#!/usr/bin/env python3
"""Read-only host discovery, contract checks, and download verification for CI."""

import argparse
import base64
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request

from detector_assets import ROOT, file_sha
from package import ID, TRIPLES, rust_channel

UPSTREAM = "zyycn/codex-proxy-rs"
LEDGER_BRANCH = "cpr-compatibility"
SDK = pathlib.Path("backend/crates/gateway-plugin/sdk")
RUNTIME = SDK.parent / "runtime"
RELEASE_MANIFEST = pathlib.Path("plugins/official/plugin-release-manifest.json")
HOST_PLATFORM = dict(zip(TRIPLES, ("linux_amd64", "linux_arm64", "darwin_arm64"), strict=True))
STABLE = re.compile(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)\Z")
SHA = re.compile(r"[a-f0-9]{40}\Z")
DIGEST = re.compile(r"[a-f0-9]{64}\Z")
CALLBACKS = {
    "host.auth.list", "host.auth.get_runtime", "host.keys.list", "host.data.access.check",
    "host.model.execute_stream", "host.model.stream_read", "host.model.stream_close",
    "host.state.get", "host.state.put", "plugin.reconcile", "management.handle", "management.register",
}


def read_json(path):
    return json.loads(pathlib.Path(path).read_bytes())


def write_json(path, obj):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=2, sort_keys=True) + "\n")


def api(path, missing=False):
    url = "https://api.github.com/" + path.lstrip("/")
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "cpr-iq-test-ci"}
    if token := os.environ.get("GH_TOKEN"):
        headers["Authorization"] = "Bearer " + token
    error = None
    for attempt in range(3):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=35) as response:
                return json.load(response)
        except urllib.error.HTTPError as e:
            if e.code == 404 and missing:
                return None
            if e.code < 500 and e.code != 429:
                raise
            error = e
        except (OSError, ValueError) as e:
            error = e
        time.sleep(1 + attempt * 2)
    raise RuntimeError(f"GitHub API read failed: {path}: {error}")


def ref_commit(repo, kind, name):
    obj = api(f"repos/{repo}/git/ref/{kind}/{urllib.parse.quote(name, safe='')}")["object"]
    for _ in range(6):
        if obj["type"] == "commit" and SHA.fullmatch(obj["sha"]):
            return obj["sha"]
        if obj["type"] != "tag" or not SHA.fullmatch(obj["sha"]):
            break
        obj = api(f"repos/{repo}/git/tags/{obj['sha']}")["object"]
    raise ValueError(f"Ref is not a commit: {repo}/{kind}/{name}")


def download(url, output, expected=None, size_limit=140 * 1024 * 1024):
    if not url.startswith(("https://github.com/", "https://codeload.github.com/")):
        raise ValueError(f"Unexpected download location: {url}")
    if expected is not None and not DIGEST.fullmatch(expected):
        raise ValueError("Unusable SHA-256 for artifact")
    if output.is_file() and expected and file_sha(output) == expected:
        return output
    output.parent.mkdir(parents=True, exist_ok=True)
    failure = None
    for attempt in range(3):
        try:
            with tempfile.NamedTemporaryFile(dir=output.parent, delete=False) as handle:
                temporary = pathlib.Path(handle.name)
                try:
                    with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent":"cpr-iq-test-ci"}), timeout=60) as response:
                        remaining = size_limit
                        while buf := response.read(1024 * 1024):
                            remaining -= len(buf)
                            if remaining < 0:
                                raise ValueError("Download exceeded size limit")
                            handle.write(buf)
                    handle.flush()
                    if expected and file_sha(temporary) != expected:
                        raise ValueError("SHA-256 mismatch")
                    temporary.replace(output)
                    return output
                finally:
                    temporary.unlink(missing_ok=True)
        except (OSError, ValueError) as error:
            failure = error
            time.sleep(1 + attempt * 2)
    raise RuntimeError(f"Download failed: {url}: {failure}")


def archive_unpack(archive, dest):
    dest.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, "r:gz") as tar:
        members = tar.getmembers()
        if sum(entry.size for entry in members) > 750 * 1024 * 1024:
            raise ValueError("Archive expanded size exceeded limit")
        tar.extractall(dest, members=members, filter="data")


def host_source(commit, work):
    if not SHA.fullmatch(commit):
        raise ValueError("Host source commit not pinned")
    dest = work / "source" / f"codex-proxy-rs-{commit}"
    if (dest / ".iq-test-extracted").is_file():
        return dest
    url = f"https://codeload.github.com/{UPSTREAM}/tar.gz/{commit}"
    source_tar = download(url, work / f"source-{commit}.tar.gz", size_limit=45 * 1024 * 1024)
    archive_unpack(source_tar, dest.parent)
    if not (dest / SDK / "Cargo.toml").is_file():
        raise ValueError("Host SDK layout changed; update the adapter")
    (dest / ".iq-test-extracted").touch()
    return dest


def slim_assets(assets):
    return {asset["name"]: {"url": asset["browser_download_url"], "sha256": (asset.get("digest") or "").removeprefix("sha256:")}
            for asset in assets}


def release_asset(assets, name, work):
    asset = assets[name]
    digest = asset["sha256"]
    if not DIGEST.fullmatch(digest):
        if name == "checksums.txt":
            raise ValueError("Checksum asset must have a GitHub SHA-256 digest")
        checksums = release_asset(assets, "checksums.txt", work).read_text().splitlines()
        matches = [parts[0] for line in checksums if len(parts := line.split()) == 2 and parts[1].lstrip("*") == name]
        if len(matches) != 1:
            raise ValueError(f"No unique checksum for {name}")
        digest = matches[0]
    return download(asset["url"], work / name, digest)


def prepare_host(plan, triple, work):
    version = plan["hostTag"][1:]
    asset = f"codex-proxy-rs_{version}_{HOST_PLATFORM[triple]}.tar.gz"
    archive = release_asset(plan["hostAssets"], asset, work / "host-assets")
    dest = work / "native" / "codex-proxy-rs"
    archive_unpack(archive, dest.parent)
    manifest = read_json(dest / RELEASE_MANIFEST)
    if manifest["gateway_git_sha"] != plan["hostCommit"] or manifest["gateway_version"] != version:
        raise ValueError("Host release binary and locked tag commit disagree")
    if manifest["plugin_host"] != plan["contract"]["host"]:
        raise ValueError("Host source contract and packaged declarations disagree")
    return dest


def sdk_files(source):
    directory = source / SDK
    paths = sorted([*directory.joinpath("src").rglob("*.rs"), directory / "Cargo.toml"])
    return {str(path.relative_to(directory)): file_sha(path) for path in paths}


def int_const(file, name):
    result = re.search(rf"pub const {name}: u32 = (\d+);", file.read_text())
    if result is None:
        raise ValueError(f"Unable to identify {name}: adapt contract reader")
    return int(result[1])


def contract(source, base):
    current = read_json(source / RUNTIME / "plugin-host-compatibility.json")
    deprecated = read_json(source / RUNTIME / "plugin-api-deprecations.json")
    author = read_json(ROOT / "plugin.json")
    used = {name: item.get("version", 1) for name, item in author["contributes"].items()}
    host = {item["capability"]: item["versions"] for item in current["capabilities"]}
    protocol = int_const(source / SDK / "src/message.rs", "PROTOCOL_VERSION")
    manifest = int_const(source / SDK / "src/manifest.rs", "MANIFEST_VERSION")
    changes = []
    a, b = sdk_files(source), sdk_files(base)
    for path in sorted(a.keys() | b.keys()):
        if a.get(path) != b.get(path):
            changes.append(path)
    strings = "\n".join(path.read_text() for path in (source / SDK / "src").rglob("*.rs"))
    missing_callbacks = sorted(method for method in CALLBACKS if f'"{method}"' not in strings)
    used_deprecations = [item for item in deprecated if used.get(item["capability"]) == item["version"]]
    problems = []
    if current["schema_version"] != 2 or manifest != author["manifestVersion"] or manifest not in current["manifest_schema_versions"]:
        problems.append("Manifest format")
    if protocol not in current["protocol_versions"]:
        problems.append("Host does not accept the bundled SDK protocol")
    if any(version not in host.get(name, []) for name, version in used.items()):
        problems.append("Management/maintenance capability contract")
    if missing_callbacks:
        problems.append("Missing callback contracts: " + ", ".join(missing_callbacks))
    if any(row["remaining_releases"] <= 0 for row in used_deprecations):
        problems.append("Used capability is at the end of its deprecation window")
    return {"sdkChangedFiles": changes, "sdkFingerprint": hashlib.sha256(json.dumps(a, sort_keys=True).encode()).hexdigest(),
            "host": current, "candidateProtocol": protocol, "usedCapabilities": used, "deprecations": used_deprecations, "problems": problems}


def package_manifest(archive):
    with tarfile.open(archive, "r:gz") as tar:
        members = [item for item in tar if item.name in ("plugin.json", "./plugin.json")]
        if len(members) != 1 or not members[0].isfile() or members[0].size > 65_536:
            raise ValueError("Invalid packaged manifest size or path")
        return json.load(tar.extractfile(members[0]))


def engine_matches(requirement, tag):
    # All our builds use a tilde and all three components. Unknown syntax must not be interpreted as compatible.
    lower = STABLE.fullmatch("v" + requirement[1:]) if requirement.startswith("~") else None
    current = STABLE.fullmatch(tag)
    return bool(lower and current and lower.groups()[:2] == current.groups()[:2] and int(lower[3]) <= int(current[3]))


def package_coverage(manifest, host, tag):
    if manifest["publisher"] + "." + manifest["name"] != ID:
        return "wrong plugin identity"
    if not engine_matches(manifest["engines"]["codex-proxy-rs"], tag):
        return "engine-range"
    if manifest["manifestVersion"] not in host["manifest_schema_versions"]:
        return "manifest-contract"
    if manifest["package"]["protocolVersion"] not in host["protocol_versions"]:
        return "rpc-contract"
    caps = {item["capability"]: item["versions"] for item in host["capabilities"]}
    if any(contribution["version"] not in caps.get(name, []) for name, contribution in manifest["contributes"].items()):
        return "capability-contract"
    return None


def ledger(repo, tag):
    obj = api(f"repos/{repo}/contents/compatibility/{tag}.json?ref={LEDGER_BRANCH}", missing=True)
    if obj is None:
        return None
    return json.loads(base64.b64decode(obj["content"]))


def own_releases(repo):
    all_releases = []
    for page in range(1, 41):
        items = api(f"repos/{repo}/releases?per_page=100&page={page}")
        all_releases.extend(items)
        if len(items) < 100:
            return all_releases
    raise ValueError("Release namespace scan exceeded limit")


def discovery(args):
    repo = args.repository
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
        raise ValueError("A valid plugin GitHub repository is required")
    manual = bool(args.tag)
    if manual and not STABLE.fullmatch(args.tag):
        raise ValueError("Use a stable upstream tag vX.Y.Z")
    release = api(f"repos/{UPSTREAM}/releases/" + ("tags/" + args.tag if manual else "latest"))
    tag = release["tag_name"]
    if not STABLE.fullmatch(tag) or release["prerelease"] or release["draft"] or not release["published_at"]:
        raise ValueError("Unpublished or non-stable upstream release")
    sha = ref_commit(UPSTREAM, "tags", tag)
    known = ledger(repo, tag)
    if known and known["hostCommit"] != sha:
        raise ValueError("Published upstream tag moved: stop rather than overwriting compatibility evidence")
    result = {"hostTag": tag, "hostCommit": sha, "hostAssets": slim_assets(release["assets"]), "repository": repo,
              "skip": bool(known and not manual)}
    if not result["skip"]:
        records = []
        series = read_json(ROOT / "plugin.json")["version"].rsplit(".",1)[0]
        for prior in own_releases(repo):
            prior_tag = prior["tag_name"]
            if STABLE.fullmatch(prior_tag) and prior_tag.startswith(f"v{series}."):
                records.append((int(prior_tag.rsplit(".",1)[1]), prior))
        available = [item for _, item in sorted(records, key=lambda p:p[0], reverse=True) if not item["draft"] and not item["prerelease"]
                     and "verification.json" in slim_assets(item["assets"])]
        if known:
            known_tag = "v" + known["pluginVersion"]
            available = [rel for rel in available if rel["tag_name"] == known_tag]
            if not available:
                raise ValueError("Recorded verified plugin release is missing; do not silently rewrite the compatibility table")
        tool_source = host_source(sha, args.work)
        base_pin = tomllib.loads((ROOT / "Cargo.toml").read_text())["dependencies"]["gateway-plugin-sdk"]["rev"]
        base = host_source(base_pin, args.work)
        result["contract"] = contract(tool_source, base)
        result["rust"] = rust_channel(tool_source)
        result["previous"] = None if not available else {
            "version": available[0]["tag_name"][1:], "assets": slim_assets(available[0]["assets"]),
        }
        result["candidateVersion"] = series + "." + str(1 + max((patch for patch, _ in records), default=-1))
        result["minimumHost"] = read_json(ROOT / "plugin.json")["engines"]["codex-proxy-rs"].removeprefix("~")
        # A target older than the original validated SDK baseline should require a deliberate port.
        minimum = tuple(map(int, result["minimumHost"].split(".")))
        if tuple(map(int, STABLE.fullmatch(tag).groups())) < minimum:
            raise ValueError("Host target is below the supported baseline")
    write_json(args.output, result)
    if args.github_output:
        with args.github_output.open("a") as f:
            print(f"skip={str(result['skip']).lower()}\nhost_tag={tag}\nhost_sha={sha}", file=f)
            if "rust" in result:
                print(f"rust={result['rust']}", file=f)
    print(f"Host {tag} at {sha}: " + ("already verified" if result["skip"] else json.dumps(result["contract"], ensure_ascii=False)))


def main_preflight(args):
    sha = ref_commit(UPSTREAM, "heads", "main")
    source = host_source(sha, args.work)
    base_ref = tomllib.loads((ROOT / "Cargo.toml").read_text())["dependencies"]["gateway-plugin-sdk"]["rev"]
    report = {"hostCommit": sha, **contract(source, host_source(base_ref, args.work))}
    write_json(args.output, report)
    print(json.dumps(report, indent=2, ensure_ascii=False))
    if report["problems"]:
        raise ValueError("Host main contract has changed")
    if args.compile:
        channel = rust_channel(source)
        subprocess.run(["rustup", "toolchain", "install", channel, "--profile", "minimal"], check=True)
        subprocess.run([sys.executable, str(ROOT / "scripts/package.py"), "--host-source", str(source),
                        "--host-sha", sha, "--host-tag", "main", "--target", "x86_64-unknown-linux-gnu", "--no-package"], check=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)
    discover = sub.add_parser("discover")
    discover.add_argument("--repository", required=True)
    discover.add_argument("--tag", default="")
    discover.add_argument("--github-output", type=pathlib.Path)
    nightly = sub.add_parser("main")
    nightly.add_argument("--compile", action="store_true")
    for command in (discover, nightly):
        command.add_argument("--work", type=pathlib.Path, required=True)
        command.add_argument("--output", type=pathlib.Path, required=True)
    args = p.parse_args()
    (discovery if args.command == "discover" else main_preflight)(args)


if __name__ == "__main__":
    main()
