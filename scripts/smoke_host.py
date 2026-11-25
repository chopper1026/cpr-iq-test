#!/usr/bin/env python3
"""Install and exercise a package on an isolated, real host binary. No upstream OAuth calls."""

import argparse
import base64
from contextlib import ExitStack
import http.cookiejar
import json
import os
import pathlib
import secrets
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid
import zlib

from detector_assets import file_sha


def timed_wait(label, ready, limit=80):
    start = time.monotonic()
    last = None
    while time.monotonic() < start + limit:
        try:
            result = ready()
            if result:
                return result
        except (OSError, RuntimeError) as error:
            last = error
        time.sleep(0.25)
    raise RuntimeError(f"Timed out waiting for {label}" + (f": {last}" if last else ""))


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def stop(proc):
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(20)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(5)


def launch(stack, log, argv, **opts):
    with log.open("ab") as file:
        proc = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=file, stderr=subprocess.STDOUT, **opts)
    stack.callback(stop, proc)
    return proc


def database(stack, root, use_docker, db_password, redis_password):
    if use_docker:
        def container(image, port, *args, environment=()):
            name = f"cpr-smoke-{uuid.uuid4().hex[:12]}"
            run("docker", "run", "--detach", "--rm", "--name", name, "-p", f"127.0.0.1::{port}",
                *environment, image, *args, stdout=subprocess.DEVNULL)
            stack.callback(subprocess.run, ("docker", "rm", "-f", name), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            bound = run("docker", "port", name, str(port), capture_output=True, text=True).stdout.strip()
            return name, int(bound.rsplit(":", 1)[1])

        db, port = container("postgres:17-alpine", 5432, environment=("-e", f"POSTGRES_PASSWORD={db_password}"))
        _, redis_port = container("redis:7-alpine", 6379, "redis-server", "--requirepass", redis_password)
        psql = ["docker", "exec", "-i", "-e", f"PGPASSWORD={db_password}", db, "psql", "-h", "127.0.0.1", "-U", "postgres", "-d", "postgres"]
    else:
        initdb = shutil.which("initdb")
        if initdb is None:
            raise RuntimeError("initdb, psql, and redis-server must be on PATH (or pass --docker-services)")
        port, redis_port = free_port(), free_port()
        if port == redis_port:
            redis_port = free_port()
        pwfile = root / "db.password"
        pwfile.write_text(db_password + "\n")
        pwfile.chmod(0o600)
        with (root / "db.log").open("wb") as log:
            run(initdb, "-D", str(root / "pg"), "-U", "postgres", "-A", "scram-sha-256", f"--pwfile={pwfile}", stdout=log, stderr=subprocess.STDOUT)
        pwfile.unlink()
        launch(stack, root / "db.log", ["postgres", "-D", str(root / "pg"), "-h", "127.0.0.1", "-p", str(port),
                                       "-k", str(root)])
        launch(stack, root / "redis.log", ["redis-server", "--bind", "127.0.0.1", "--port", str(redis_port),
                                          "--save", "", "--appendonly", "no", "--requirepass", redis_password])
        psql = ["psql", "-h", "127.0.0.1", "-p", str(port), "-U", "postgres", "-d", "postgres"]

    def sql(query, retry=False):
        result = subprocess.run([*psql, "-XqtA", "-w", "-v", "ON_ERROR_STOP=1"], input=query,
                                text=True, capture_output=True, env={**os.environ, "PGPASSWORD": db_password})
        if result.returncode != 0:
            if retry:
                return None
            raise RuntimeError(f"Isolated database fixture failed: {result.stderr.strip()[:500]}")
        return result.stdout.strip()

    timed_wait("the isolated database", lambda: sql("SELECT 1;", retry=True))
    timed_wait("the isolated Redis", lambda: port_open(redis_port))
    return port, redis_port, sql


def port_open(port):
    with socket.create_connection(("127.0.0.1", port), timeout=1):
        return True


class API:
    def __init__(self, port):
        self.origin = f"http://127.0.0.1:{port}"
        self.client = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

    def call(self, method, path, body=None, expected=None):
        headers = {"Origin": self.origin}
        if isinstance(body, bytes):
            headers["Content-Type"] = "application/octet-stream"
        elif body is not None:
            body = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(self.origin + path, data=body, headers=headers, method=method)
        try:
            response = self.client.open(request, timeout=30)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            raw = response.read()
            if (expected is not None and response.code != expected) or (expected is None and not 200 <= response.code < 300):
                raise RuntimeError(f"{method} {path}: HTTP {response.code}: {raw[:500].decode(errors='replace')}")
            if response.headers.get_content_type() == "application/json":
                return json.loads(raw)
            return raw


def upload(api, package):
    digest = file_sha(package)
    prefix = "/api/admin/plugins/artifacts"
    content = package.read_bytes()
    verified = api.call("POST", prefix + "/upload/verify", content)
    assert verified["data"]["metadata"]["sha256"] == digest, verified
    installed = api.call("POST", prefix + "/upload?sha256=" + digest, content)["data"]
    # Release variants may either accept on installation or keep a separate trust confirmation.
    if not installed["artifact"].get("acceptedAt"):
        api.call("POST", prefix + "/accept", {"sha256": digest})
    return digest


def extension(api, digest):
    items = api.call("GET", "/api/admin/plugins/extensions")["data"]
    return next((item for item in items if item["target"]["artifactSha256"] == digest), None)


def prefix_for(ext):
    target = ext["target"]
    return f'/api/admin/plugins/extensions/{target["instanceId"]}/{target["artifactSha256"]}/{target["revision"]}'


def seed_history(sql, instance_id, hours):
    # A DB fixture is applied only to this newly initialized smoke database, never to a deployment.
    uuid.UUID(instance_id)
    scope = f"g.instance_id='{instance_id}' AND g.namespace='iq-test' AND g.status='active'"
    raw = sql(f"SELECT r.value_json->>'document' FROM plugin_state_records r JOIN plugin_state_generations g ON r.generation_id=g.id WHERE {scope};")
    state = json.loads(zlib.decompress(base64.b64decode(raw)))
    assert not state["queue"] and state["settings"]["hours"] == hours
    now_ms = int(time.time() * 1000)
    # Seed both a new-format entry with real email and a legacy v0.1.0-dev.1 profileId/calibration entry.
    modern = {
        "runId": "m-smokemodern",
        "accountId": "acct_smoke_modern",
        "accountEmail": "smoke@example.com",
        "accountName": "Smoke Account",
        "model": "gpt-6.2-sol",
        "effort": "high",
        "scheduled": False,
        "endedAt": now_ms,
        "pluginVersion": "fixture",
        "firstRound": None,
        "outcome": {
            "verdict": "match",
            "reason": None,
            "lastRound": {
                "ranking": {"candidates": [{"id": "gpt-6.2-sol", "label": "gpt-6.2-sol", "probabilityBps": 9100}]},
                "stamp": "smoke-stamp",
                "detectorRevision": "fixture",
                "topMarginBps": 8200,
            },
        },
    }
    legacy = {
        "runId": "m-smokelegacy",
        "accountId": "acct_smoke_legacy",
        "profileId": "gpt6-sol-low",
        "mode": "calibration",
        "scheduled": True,
        "endedAt": now_ms - 1000,
        "pluginVersion": "0.1.0-dev.1",
        "firstRound": None,
        "outcome": {"verdict": "calibration", "reason": None, "lastRound": None},
    }
    state["history"] = [modern, legacy]
    value = json.dumps({"document": base64.b64encode(zlib.compress(json.dumps(state).encode())).decode()}, separators=(",", ":"))
    # Value contains only base64 and fixed JSON syntax; instance ID is a verified UUID.
    sql(f"BEGIN; UPDATE plugin_state_records r SET value_json='{value}'::jsonb, value_bytes={len(value)}"
        f" FROM plugin_state_generations g WHERE g.id=r.generation_id AND {scope};"
        f" UPDATE plugin_state_generations g SET total_bytes={len(value)} WHERE {scope}; COMMIT;")
    return modern, legacy


def smoke(args):
    bin_path = args.host_dir / "codex-proxy-rs"
    web_root = args.host_dir / "web" / "dist"
    assert web_root.joinpath("index.html").is_file()
    release_manifest = json.loads((args.host_dir / "plugins/official/plugin-release-manifest.json").read_bytes())
    assert release_manifest["gateway_version"] == args.host_tag[1:] and release_manifest["gateway_git_sha"] == args.host_sha
    assert bin_path.is_file() and os.access(bin_path, os.X_OK)
    with tempfile.TemporaryDirectory(prefix="cpr-smoke-", dir="/tmp") as temp, ExitStack() as stack:
        root = pathlib.Path(temp)
        (root / "deploy").mkdir()
        log = root / "host.log"
        db_password, redis_password, admin_password = (secrets.token_hex(24) for _ in range(3))
        db, redis, sql = database(stack, root, args.docker_services, db_password, redis_password)
        port = free_port()
        username = "smoke@cpr.local"
        cfg = root / "deploy/config.yaml"
        cfg.write_text(json.dumps({
            "schema_version": 1,
            "host": {"timezone": "Asia/Shanghai", "worker_shutdown_timeout_seconds": 10, "listen": {"host": "127.0.0.1", "port": port}, "runtime_data_dir": str(root / "data"),
                     "logging": {"level": "info", "stdout": True, "request_dump": False, "file": {"enabled": False, "directory": str(root / "logs"), "retention_days": 7, "max_file_size_mb": 20}}},
            "store": {"database": {"url": f"postgres://postgres@127.0.0.1:{db}/postgres", "password": db_password},
                      "redis": {"url": f"redis://127.0.0.1:{redis}/", "password": redis_password}},
            "admin": {"default_username": username, "default_password": admin_password, "session_ttl_minutes": 60, "session_absolute_ttl_minutes": 120},
            "client": {"session_ttl_minutes": 60},
            "api": {"asset_directory": str(web_root), "cors_allowed_origins": [], "request_timeout_seconds": None, "request_id_header": "x-request-id"},
        }))
        cfg.chmod(0o600)
        # Disallow inherited CPR overrides on a workstation; never contact or use any installed host configuration.
        env = {key: value for key, value in os.environ.items() if not key.startswith("CPR_") and key not in {"GH_TOKEN", "GITHUB_TOKEN"}}
        host = launch(stack, log, [str(bin_path)], cwd=root, env=env)
        api = API(port)

        def health():
            if host.poll() is not None:
                raise AssertionError(f"Isolated host exited early ({host.returncode})")
            api.call("GET", "/healthz")
            return True

        def login():
            timed_wait("host health", health)
            api.call("POST", "/api/auth/login", {"mode": "admin", "username": username, "password": admin_password})

        def verify_seeded_runs(url):
            all_runs = api.call("GET", url + "/api/runs?page=1&pageSize=10")
            assert all_runs["pagination"]["total"] == 2 and len(all_runs["items"]) == 2, all_runs
            filtered = api.call("GET", url + "/api/runs?search=smoke%40example.com&source=manual&verdict=match&page=1&pageSize=1")
            assert filtered["pagination"]["total"] == 1 and filtered["items"][0]["accountEmail"] == "smoke@example.com", filtered
            migrated = api.call("GET", url + "/api/runs?source=scheduled&verdict=ranked")
            assert migrated["pagination"]["total"] == 1, migrated
            assert migrated["items"][0]["model"] == "gpt-6-sol" and migrated["items"][0]["effort"] == "low", migrated

        try:
            login()
            base_digest = upload(api, args.baseline or args.package)
            ext = timed_wait("management registration", lambda: extension(api, base_digest), 22)
            base_target = ext["target"]
            base_url = prefix_for(ext)
            assert any(page["entry"] == "web/index.html" for page in ext["pages"])
            html = api.call("GET", base_url + "/resources/web/index.html")
            assert b"app.js" in html and b"tab-records" in html
            assert api.call("GET", base_url + "/resources/web/app.js")
            assert api.call("GET", base_url + "/resources/web/style.css")
            catalog = api.call("GET", base_url + "/api/catalog")
            assert catalog["detectorReady"] and not catalog["accounts"] and not catalog["keys"] and catalog["models"] == [], catalog
            runs = api.call("GET", base_url + "/api/runs?page=1&pageSize=20")
            assert not runs["items"] and runs["pagination"]["total"] == 0, runs
            assert api.call("POST", base_url + "/api/runs/cancel", {}) == {"cancelled": 0}
            default = api.call("GET", base_url + "/api/schedule")["settings"]
            assert default == {"enabled": False, "hours": 24, "target": None}, default
            settings = {**default, "hours": 12}
            edited = api.call("PUT", base_url + "/api/schedule", {"settings": settings, "previous": default})
            assert edited["settings"] == settings and edited["nextDueAt"] is None
            api.call("POST", base_url + "/api/runs", {"runId": "m-invalidsmoke", "target": {
                "clientKeyId": "key-fixture", "model": "", "effort": "low", "accountIds": ["no-account"]}}, expected=400)
            if args.baseline:
                seed_history(sql, base_target["instanceId"], 12)
                verify_seeded_runs(base_url)
                digest = upload(api, args.package)
                api.call("POST", "/api/admin/plugins/instances/switch-version", {"id": base_target["instanceId"],
                    "target": {"artifactSha256": digest, "expectedRevision": base_target["revision"]}})
                ext = timed_wait("switched management registration", lambda: extension(api, digest), 22)
                assert ext["target"]["instanceId"] == base_target["instanceId"]
                new_url = prefix_for(ext)
                candidate_catalog = api.call("GET", new_url + "/api/catalog")
                assert candidate_catalog["detectorReady"]
                assert api.call("GET", new_url + "/api/schedule")["settings"] == settings
                verify_seeded_runs(new_url)
                stop(host)
                host = launch(stack, log, [str(bin_path)], cwd=root, env=env)
                login()
                ext = timed_wait("management registration after restart", lambda: extension(api, digest), 22)
                now_url = prefix_for(ext)
                assert api.call("GET", now_url + "/api/schedule")["settings"] == settings
                verify_seeded_runs(now_url)
            print(f"Smoke OK: {args.host_tag}, {args.package.name}; no upstream account or model was called")
        finally:
            if args.log:
                args.log.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(log, args.log)
            if sys.exc_info()[0] is not None:
                print(log.read_text(errors="replace")[-8000:], file=sys.stderr)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--host-dir", type=pathlib.Path, required=True)
    p.add_argument("--host-tag", required=True)
    p.add_argument("--host-sha", required=True)
    p.add_argument("--package", type=pathlib.Path, required=True)
    p.add_argument("--baseline", type=pathlib.Path)
    p.add_argument("--docker-services", action="store_true")
    p.add_argument("--log", type=pathlib.Path)
    args = p.parse_args()
    args.host_dir = args.host_dir.resolve()
    smoke(args)


if __name__ == "__main__":
    main()
