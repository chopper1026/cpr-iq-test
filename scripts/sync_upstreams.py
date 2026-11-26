#!/usr/bin/env python3
"""Inspect and synchronize upstream Codex Proxy (stable) and lm-detector (data + Rust SDK) pins."""

import argparse
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import tomllib
import urllib.request

from detector_assets import DATA, ROOT, bank_lock, file_sha
from follow_host import (
    LM_UPSTREAM,
    SHA,
    STABLE,
    UPSTREAM,
    api,
    contract,
    engine_matches,
    host_source,
    parse_semver,
    read_json,
    ref_commit,
    write_json,
)
from package import rust_channel

LM_WATCH_FILES = ("data/unified_bank.json", "data/shared_detector.bin")
LM_WATCH_PREFIXES = ("sdk/rust/",)
MAX_ASSET_BYTES = 64 * 1024 * 1024


def is_lm_relevant(path):
    return path in LM_WATCH_FILES or any(path.startswith(prefix) for prefix in LM_WATCH_PREFIXES)


def _lm_tree_fingerprints(repo, rev):
    fingerprints = {}
    data_entries = api(f"repos/{repo}/contents/data?ref={rev}", missing=True) or []
    if isinstance(data_entries, list):
        for item in data_entries:
            name = item.get("name")
            if f"data/{name}" in LM_WATCH_FILES:
                fingerprints[f"data/{name}"] = item.get("sha")
    sdk_entries = api(f"repos/{repo}/contents/sdk?ref={rev}", missing=True) or []
    if isinstance(sdk_entries, list):
        for item in sdk_entries:
            if item.get("name") == "rust":
                fingerprints["sdk/rust/"] = item.get("sha")
    return fingerprints


def lm_changed_paths(repo, old_rev, new_rev):
    """Return sorted detector-relevant paths changed between old_rev and new_rev."""
    if old_rev == new_rev:
        return []
    compare = api(f"repos/{repo}/compare/{old_rev}...{new_rev}", missing=True)
    if compare is not None and compare.get("status") != "diverged":
        files = compare.get("files") or []
        if len(files) < 300:
            return sorted(
                entry["filename"]
                for entry in files
                if isinstance(entry.get("filename"), str) and is_lm_relevant(entry["filename"])
            )
    # Fallback when diff is truncated (>=300 files) or history diverged: compare git tree/blob SHAs.
    old_fp = _lm_tree_fingerprints(repo, old_rev)
    new_fp = _lm_tree_fingerprints(repo, new_rev)
    return sorted(key for key in (old_fp.keys() | new_fp.keys()) if old_fp.get(key) != new_fp.get(key))


def download_lm_assets(repo, revision, data_dir=DATA):
    if not SHA.fullmatch(revision):
        raise ValueError(f"Invalid lm-detector revision: {revision}")
    data_dir.mkdir(parents=True, exist_ok=True)
    files_meta = {}
    for name in ("unified_bank.json", "shared_detector.bin"):
        destination = data_dir / name
        downloaded = None
        for url in (
            f"https://raw.githubusercontent.com/{repo}/{revision}/data/{name}",
            f"https://cdn.jsdelivr.net/gh/{repo}@{revision}/data/{name}",
        ):
            with tempfile.TemporaryDirectory(prefix="cpr-lm-sync-", dir=data_dir) as scratch:
                staging = pathlib.Path(scratch) / name
                try:
                    req = urllib.request.Request(url, headers={"User-Agent": "cpr-iq-test-ci"})
                    with urllib.request.urlopen(req, timeout=60) as response, staging.open("wb") as out:
                        total = 0
                        while chunk := response.read(1024 * 1024):
                            total += len(chunk)
                            if total > MAX_ASSET_BYTES:
                                raise ValueError("asset exceeded maximum size")
                            out.write(chunk)
                    size = staging.stat().st_size
                    if size == 0:
                        raise ValueError("empty asset")
                    if name == "unified_bank.json":
                        parsed = json.loads(staging.read_bytes())
                        if not isinstance(parsed.get("models"), list) or not parsed["models"]:
                            raise ValueError("unified_bank.json missing models list")
                    digest = file_sha(staging)
                    staging.replace(destination)
                    downloaded = {"bytes": size, "sha256": digest}
                    break
                except (OSError, ValueError) as error:
                    print(f"Warning: failed fetching {name} from {url}: {error}", file=sys.stderr)
        if downloaded is None:
            raise RuntimeError(f"Unable to download lm-detector asset {name} at {revision}")
        files_meta[name] = downloaded
    return files_meta


def update_cargo_dep_rev(cargo_path, crate_name, old_rev, new_rev):
    text = cargo_path.read_text()
    pattern = re.compile(
        rf'^({re.escape(crate_name)}\s*=\s*\{{[^\n]*?\brev\s*=\s*")({re.escape(old_rev)})("[^\n]*\}})$',
        re.MULTILINE,
    )
    matches = list(pattern.finditer(text))
    if len(matches) != 1:
        raise ValueError(f"Expected exactly 1 pin for {crate_name} at {old_rev}, found {len(matches)}")
    updated = pattern.sub(rf"\g<1>{new_rev}\g<3>", text, count=1)
    cargo_path.write_text(updated)


def update_rust_version(toolchain_path, cargo_path, new_channel):
    if not re.fullmatch(r"\d+\.\d+\.\d+", new_channel):
        raise ValueError(f"Invalid Rust toolchain version: {new_channel}")
    tc_text = toolchain_path.read_text()
    tc_new, tc_count = re.subn(r'^(channel\s*=\s*")[^"]+(")$', rf'\g<1>{new_channel}\g<2>', tc_text, flags=re.MULTILINE)
    if tc_count != 1:
        raise ValueError("Could not uniquely update channel in rust-toolchain.toml")
    toolchain_path.write_text(tc_new)

    cargo_text = cargo_path.read_text()
    cargo_new, cargo_count = re.subn(
        r'^(rust-version\s*=\s*")[^"]+(")$',
        rf'\g<1>{new_channel}\g<2>',
        cargo_text,
        flags=re.MULTILINE,
    )
    if cargo_count != 1:
        raise ValueError("Could not uniquely update rust-version in Cargo.toml")
    cargo_path.write_text(cargo_new)


def inspect_upstreams(work, host_tag="", force_lm=False):
    author = read_json(ROOT / "plugin.json")
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text())
    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    lock = bank_lock()

    # 1. Inspect Codex Proxy stable release
    if host_tag and not STABLE.fullmatch(host_tag):
        raise ValueError("Use a stable upstream CPR tag vX.Y.Z")
    rel = api(f"repos/{UPSTREAM}/releases/" + ("tags/" + host_tag if host_tag else "latest"))
    tag = rel["tag_name"]
    if not STABLE.fullmatch(tag) or rel.get("prerelease") or rel.get("draft") or not rel.get("published_at"):
        raise ValueError("Unpublished or non-stable upstream CPR release")
    host_sha = ref_commit(UPSTREAM, "tags", tag)
    current_sdk_rev = cargo["dependencies"]["gateway-plugin-sdk"]["rev"]
    current_engines = author["engines"]["codex-proxy-rs"]
    current_min = current_engines.removeprefix("~")
    host_ver_tuple = parse_semver(tag[1:])
    min_ver_tuple = parse_semver(current_min)

    tool_source = host_source(host_sha, work)
    base_source = host_source(current_sdk_rev, work)
    contract_report = contract(tool_source, base_source)
    target_rust = rust_channel(tool_source)

    engine_needs_update = (not engine_matches(current_engines, tag)) and (host_ver_tuple > min_ver_tuple)
    sdk_changed = bool(contract_report["sdkChangedFiles"])
    rust_changed = target_rust != toolchain
    host_needs_sync = (not contract_report["problems"]) and (engine_needs_update or sdk_changed or rust_changed)

    # 2. Inspect lm-detector main branch (data assets + sdk/rust)
    lm_repo = lock["repository"]
    if lm_repo != LM_UPSTREAM:
        raise ValueError(f"Unexpected detector repository: {lm_repo}")
    lm_old_rev = lock["revision"]
    lm_latest_rev = ref_commit(lm_repo, "heads", "main")
    lm_paths = lm_changed_paths(lm_repo, lm_old_rev, lm_latest_rev)
    lm_needs_sync = bool(lm_paths) or (force_lm and lm_latest_rev != lm_old_rev)

    return {
        "host": {
            "repository": UPSTREAM,
            "tag": tag,
            "commit": host_sha,
            "currentEngines": current_engines,
            "targetEngines": f"~{tag[1:]}",
            "currentSdkRev": current_sdk_rev,
            "sdkChangedFiles": contract_report["sdkChangedFiles"],
            "currentRust": toolchain,
            "targetRust": target_rust,
            "engineNeedsUpdate": engine_needs_update,
            "sdkChanged": sdk_changed,
            "rustChanged": rust_changed,
            "contractProblems": contract_report["problems"],
            "needsSync": host_needs_sync,
            "sourceDir": str(tool_source),
        },
        "lmDetector": {
            "repository": lm_repo,
            "currentRevision": lm_old_rev,
            "latestRevision": lm_latest_rev,
            "changedPaths": lm_paths,
            "needsSync": lm_needs_sync,
        },
        "needsSync": host_needs_sync or lm_needs_sync,
    }


def build_commit_message(report):
    parts = []
    if report["host"]["needsSync"]:
        parts.append(f"CPR {report['host']['tag']}")
    if report["lmDetector"]["needsSync"]:
        parts.append(f"lm-detector {report['lmDetector']['latestRevision'][:10]}")
    title = f"build(release): 自动同步上游更新（{'、'.join(parts)}）"
    lines = [title, "", "【构建与依赖】"]
    if report["host"]["needsSync"]:
        h = report["host"]
        details = []
        if h["engineNeedsUpdate"]:
            details.append(f"engines {h['currentEngines']} -> {h['targetEngines']}")
        if h["sdkChanged"]:
            details.append(f"gateway-plugin-sdk -> {h['commit'][:10]}")
        if h["rustChanged"]:
            details.append(f"Rust {h['currentRust']} -> {h['targetRust']}")
        lines.append(f"- 同步 Codex Proxy {h['tag']}（{', '.join(details)}）。")
    if report["lmDetector"]["needsSync"]:
        lm = report["lmDetector"]
        paths = ", ".join(lm["changedPaths"][:5]) or "data & sdk/rust"
        lines.append(
            f"- 同步 lm-detector {lm['currentRevision'][:10]} -> {lm['latestRevision'][:10]}（变更：{paths}）。"
        )
    lines.extend(["", "验证：CI 已通过数据锁校验、Rust 单测、clippy、宿主核心绑定契约与 Python 发版测试。"])
    return "\n".join(lines) + "\n"


def apply_sync(report, *, verify=True):
    if report["host"]["contractProblems"]:
        raise RuntimeError("Host contract problems detected; refusing upstream sync: " + "; ".join(report["host"]["contractProblems"]))
    if not report["needsSync"]:
        return False

    cargo_path = ROOT / "Cargo.toml"
    manifest_path = ROOT / "plugin.json"
    toolchain_path = ROOT / "rust-toolchain.toml"
    lock_path = DATA / "detector.lock.json"
    rust_ver = report["host"]["targetRust"] if report["host"]["rustChanged"] else report["host"]["currentRust"]
    cargo_cmd = ["cargo"]
    if os.environ.get("GITHUB_ACTIONS") == "true" or report["host"]["rustChanged"]:
        subprocess.run(
            ["rustup", "toolchain", "install", rust_ver, "--profile", "minimal", "--component", "clippy"],
            check=True,
        )
        cargo_cmd = ["cargo", f"+{rust_ver}"]

    updated_crates = []
    if report["lmDetector"]["needsSync"]:
        lm = report["lmDetector"]
        files_meta = download_lm_assets(lm["repository"], lm["latestRevision"])
        lock_doc = {
            "repository": lm["repository"],
            "revision": lm["latestRevision"],
            "files": files_meta,
        }
        lock_path.write_text(json.dumps(lock_doc, indent=2) + "\n")
        update_cargo_dep_rev(cargo_path, "lmfpd", lm["currentRevision"], lm["latestRevision"])
        updated_crates.append("lmfpd")

    if report["host"]["needsSync"]:
        h = report["host"]
        if h["engineNeedsUpdate"]:
            manifest = read_json(manifest_path)
            manifest["engines"]["codex-proxy-rs"] = h["targetEngines"]
            manifest_path.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")
        if h["rustChanged"]:
            update_rust_version(toolchain_path, cargo_path, h["targetRust"])
        if h["sdkChanged"]:
            update_cargo_dep_rev(cargo_path, "gateway-plugin-sdk", h["currentSdkRev"], h["commit"])
            updated_crates.append("gateway-plugin-sdk")

    env = {**os.environ, "CARGO_NET_GIT_FETCH_WITH_CLI": "true"}
    for crate in updated_crates:
        subprocess.run([*cargo_cmd, "update", "-p", crate], cwd=ROOT, env=env, check=True)

    if verify:
        subprocess.run([sys.executable, str(ROOT / "scripts/detector_assets.py")], cwd=ROOT, check=True)
        subprocess.run(
            [*cargo_cmd, "test", "--locked", "--all-targets"],
            cwd=ROOT,
            env={**env, "CPR_REQUIRE_BANK": "1"},
            check=True,
        )
        subprocess.run(
            [*cargo_cmd, "clippy", "--locked", "--all-targets", "--", "-D", "warnings"],
            cwd=ROOT,
            env=env,
            check=True,
        )
        subprocess.run(
            ["bash", str(ROOT / "scripts/check_bound_host.sh"), report["host"]["sourceDir"], rust_ver],
            cwd=ROOT,
            env=env,
            check=True,
        )
        subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test_*.py"], cwd=ROOT, check=True)
    return True


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--work", type=pathlib.Path, required=True)
    p.add_argument("--host-tag", default="")
    p.add_argument("--apply", action="store_true", help="Write updated pins and run full verification")
    p.add_argument("--no-verify", action="store_true", help="Skip test/clippy verification after applying (for debugging)")
    p.add_argument("--force-lm", action="store_true", help="Update lm-detector even if only non-watched files changed")
    p.add_argument("--output", type=pathlib.Path)
    p.add_argument("--commit-message-out", type=pathlib.Path)
    p.add_argument("--github-output", type=pathlib.Path)
    args = p.parse_args()

    report = inspect_upstreams(args.work, host_tag=args.host_tag, force_lm=args.force_lm)
    applied = False
    commit_msg = ""
    if report["needsSync"]:
        commit_msg = build_commit_message(report)
    if args.apply and report["needsSync"]:
        applied = apply_sync(report, verify=not args.no_verify)
    report["applied"] = applied
    if args.commit_message_out and commit_msg:
        args.commit_message_out.parent.mkdir(parents=True, exist_ok=True)
        args.commit_message_out.write_text(commit_msg)
    if args.output:
        write_json(args.output, report)
    if args.github_output:
        with args.github_output.open("a") as f:
            print(
                f"needs_sync={str(report['needsSync']).lower()}\n"
                f"applied={str(applied).lower()}\n"
                f"host_needs_sync={str(report['host']['needsSync']).lower()}\n"
                f"lm_needs_sync={str(report['lmDetector']['needsSync']).lower()}\n"
                f"host_tag={report['host']['tag']}\n"
                f"lm_revision={report['lmDetector']['latestRevision']}\n"
                f"contract_ok={str(not report['host']['contractProblems']).lower()}",
                file=f,
            )
    print(json.dumps(report, indent=2, ensure_ascii=False))
    if report["host"]["contractProblems"]:
        raise RuntimeError("Upstream CPR contract check failed: " + "; ".join(report["host"]["contractProblems"]))


if __name__ == "__main__":
    main()
