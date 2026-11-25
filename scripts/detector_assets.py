#!/usr/bin/env python3
"""Fetch the two pinned assets at build time, never from a running plugin."""

import argparse
import hashlib
import json
import pathlib
import sys
import tempfile
import tomllib
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
DATA = ROOT / "data"
POLICY = "responses-openai-strict-rank-v1"


def file_sha(path):
    with pathlib.Path(path).open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def bank_lock():
    lock = json.loads((DATA / "detector.lock.json").read_bytes())
    pin = tomllib.loads((ROOT / "Cargo.toml").read_text())["dependencies"]["lmfpd"]
    if pin["rev"] != lock["revision"] or pin["git"] != f'https://github.com/{lock["repository"]}.git':
        raise ValueError("Detector SDK and data pins disagree")
    if set(lock["files"]) != {"unified_bank.json", "shared_detector.bin"}:
        raise ValueError("Both detector assets must be locked")
    return lock


def profile_stamp(profile):
    fields = [POLICY, *(profile[name] for name in ("id", "model", "effort", "labelId"))]
    return hashlib.sha256((DATA / "detector.lock.json").read_bytes() + b"\x00" + "\x00".join(fields).encode()).hexdigest()


def verified_profiles():
    profiles = json.loads((DATA / "profiles.json").read_bytes())
    result = []
    for profile in profiles:
        evidence = profile["validation"]
        if evidence is None:
            continue
        if not (
            evidence["profileStamp"] == profile_stamp(profile)
            and evidence["hostTag"].startswith("v")
            and evidence["evidence"].strip()
            and isinstance(evidence["positiveRounds"], int)
            and 0 < evidence["positiveRounds"] <= 65_535
            and isinstance(evidence["minimumMarginBps"], int)
            and 0 < evidence["minimumMarginBps"] <= 10_000
        ):
            raise ValueError(f'Validation evidence is invalid or stale for {profile["id"]}')
        result.append(profile["id"])
    return result


def good(path, details):
    return path.is_file() and path.stat().st_size == details["bytes"] and file_sha(path) == details["sha256"]


def fetch_assets(lock, offline=False):
    for name, details in lock["files"].items():
        destination = DATA / name
        if good(destination, details):
            continue
        if offline:
            raise ValueError(f"Missing or corrupt locked asset: {name}")
        repo, revision = lock["repository"], lock["revision"]
        for url in (
            f"https://raw.githubusercontent.com/{repo}/{revision}/data/{name}",
            f"https://cdn.jsdelivr.net/gh/{repo}@{revision}/data/{name}",
        ):
            with tempfile.TemporaryDirectory(prefix="cpr-assets-", dir=DATA) as scratch:
                staging = pathlib.Path(scratch) / name
                try:
                    with urllib.request.urlopen(url, timeout=45) as response, staging.open("wb") as out:
                        bytes_left = details["bytes"] + 1
                        while chunk := response.read(min(1024 * 1024, bytes_left)):
                            out.write(chunk)
                            bytes_left -= len(chunk)
                    if not good(staging, details):
                        raise ValueError("digest or length mismatch")
                    staging.replace(destination)
                    break
                except (OSError, ValueError) as error:
                    print(f"Could not verify {name} at {url}: {error}", file=sys.stderr)
        else:
            raise ValueError(f"Unable to obtain pinned asset: {name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify-only", action="store_true")
    parser.add_argument("--profiles", action="store_true", help="show profile stamps without fetching data")
    parser.add_argument("--require-validated", action="store_true", help="fail if there is no verified profile")
    args = parser.parse_args()
    lock = bank_lock()
    valid = verified_profiles()
    if args.require_validated and not valid:
        parser.exit(1, "No authorized validation evidence: production release blocked.\n")
    if args.profiles:
        print(json.dumps([{"id": p["id"], "profileStamp": profile_stamp(p), "validated": p["id"] in valid}
                          for p in json.loads((DATA / "profiles.json").read_bytes())], indent=2))
    else:
        fetch_assets(lock, args.verify_only)
        print(f"Verified detector pair at {lock['revision']}")


if __name__ == "__main__":
    main()
