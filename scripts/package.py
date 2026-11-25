#!/usr/bin/env python3
"""Build and package a native CPR plugin against a pinned host SDK and packager."""

import argparse
import json
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import tomllib

from detector_assets import ROOT, bank_lock, fetch_assets

TRIPLES = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin")
ID = "cpr-iq-test.account-fingerprint"


def run(*command, **kwargs):
    subprocess.run(command, check=True, **kwargs)


def rust_channel(host):
    for directory in (host / "backend", host):
        path = directory / "rust-toolchain.toml"
        if path.is_file():
            version = tomllib.loads(path.read_text())["toolchain"]["channel"]
            if re.fullmatch(r"\d+\.\d+\.\d+", version):
                return version
            break
    # Future unsupported toolchain declarations are a compatibility failure, not a silent fallback.
    raise ValueError("Host Rust toolchain must select an exact released version")


def stage_source(project, sdk_commit):
    for filename in ("Cargo.toml", "Cargo.lock", "build.rs", "plugin.json", "rust-toolchain.toml"):
        shutil.copy2(ROOT / filename, project)
    shutil.copytree(ROOT / "src", project / "src")
    shutil.copytree(ROOT / "tests", project / "tests", ignore=shutil.ignore_patterns("__pycache__"))
    (project / "data").mkdir()
    for item in ("detector.lock.json", "profiles.json", "shared_detector.bin", "unified_bank.json"):
        shutil.copy2(ROOT / "data" / item, project / "data" / item)
    file = project / "Cargo.toml"
    old = tomllib.loads(file.read_text())["dependencies"]["gateway-plugin-sdk"]["rev"]
    if sdk_commit != old:
        if file.read_text().count(old) != 1:
            raise ValueError("Ambiguous gateway-plugin-sdk pin")
        file.write_text(file.read_text().replace(old, sdk_commit))
    return sdk_commit != old


def build(args):
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=True)
    toolchain = rust_channel(args.host_source)
    fetch_assets(bank_lock(), args.offline)
    with tempfile.TemporaryDirectory(prefix="cpr-package-") as directory:
        staging = pathlib.Path(directory)
        project = staging / "project"
        project.mkdir()
        old = tomllib.loads((ROOT / "Cargo.toml").read_text())["dependencies"]["gateway-plugin-sdk"]["rev"]
        needs_lock = stage_source(project, old if args.keep_sdk else args.host_sha)
        env = {**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target" / "plugin"), "CPR_REQUIRE_BANK": "1"}
        cargo = ["cargo", "+" + toolchain]
        if needs_lock:
            run(*cargo, "update", "-p", "gateway-plugin-sdk", cwd=project, env=env)
        run(*cargo, "test", "--locked", "--target", args.target, cwd=project, env=env)
        if args.no_package:
            return
        if args.packager:
            packager = args.packager.resolve()
        else:
            cli_target = ROOT / "target" / "cpr-cli"
            run(*cargo, "build", "--locked", "--manifest-path", str(args.host_source / "backend/Cargo.toml"),
                "-p", "codex-proxy-plugin-cli", env={**env, "CARGO_TARGET_DIR": str(cli_target)})
            packager = cli_target / "debug" / "cpr-plugin"
        shutil.copytree(ROOT / "web", staging / "web")
        shutil.copytree(project / "data", staging / "data")
        for filename in ("LICENSE", "LICENSE-lm-detector", "THIRD_PARTY.md"):
            shutil.copy2(ROOT / filename, staging)
        manifest = json.loads((ROOT / "plugin.json").read_bytes())
        manifest["engines"]["codex-proxy-rs"] = "~" + args.host_tag[1:]
        version_list = [(args.version, output)]
        if args.baseline_out:
            args.baseline_out.mkdir(parents=True, exist_ok=True)
            version_list.insert(0, ("0.0.0", args.baseline_out.resolve()))
        for version, destination in version_list:
            manifest["version"] = version
            author = staging / "plugin.json"
            author.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")
            run(*cargo, "build", "--release", "--locked", "--target", args.target,
                cwd=project, env={**env, "CPR_AUTHOR_MANIFEST": str(author)})
            binary = pathlib.Path(env["CARGO_TARGET_DIR"]) / args.target / "release" / "cpr-iq-test"
            run(str(packager), "package", "--manifest", str(author), "--binary", str(binary),
                "--target", args.target, "--output-dir", str(destination))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host-source", required=True, type=pathlib.Path)
    parser.add_argument("--host-sha", required=True)
    parser.add_argument("--host-tag", required=True)
    parser.add_argument("--target", required=True, choices=TRIPLES)
    parser.add_argument("--version", default="0.1.0-dev.1")
    parser.add_argument("--out", type=pathlib.Path, default=ROOT / "dist")
    parser.add_argument("--baseline-out", type=pathlib.Path, help="also build an older version for state-upgrade smoke only")
    parser.add_argument("--offline", action="store_true", help="verify local detector assets instead of downloading")
    parser.add_argument("--no-package", action="store_true", help="just compile and run the contracts")
    parser.add_argument("--keep-sdk", action="store_true", help="for a proven manifest-only rebuild, keep the repo's SDK pin")
    parser.add_argument("--packager", type=pathlib.Path, help="use a pre-built cpr-plugin from this exact host source")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.host_sha) or not (re.fullmatch(r"v\d+\.\d+\.\d+", args.host_tag) or (args.no_package and args.host_tag == "main")):
        parser.error("host tag and commit must be pinned")
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-dev\.[1-9]\d*)?", args.version) or args.version.startswith("0.0.0"):
        parser.error("plugin version must be X.Y.Z or X.Y.Z-dev.N, newer than 0.0.0")
    args.host_source = args.host_source.resolve()
    if (args.host_source / ".git").exists():
        actual = subprocess.check_output(["git", "-C", str(args.host_source), "rev-parse", "HEAD"], text=True).strip()
        if actual != args.host_sha or subprocess.check_output(["git", "-C", str(args.host_source), "status", "--porcelain"], text=True).strip():
            parser.error("host source checkout is dirty or is not the supplied revision")
    build(args)


if __name__ == "__main__":
    main()
