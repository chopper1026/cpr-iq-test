#!/usr/bin/env python3
"""Model the three release paths, then gate any remote mutation on all three platforms."""

import argparse
import base64
import json
import os
import pathlib
import subprocess
import sys

from detector_assets import ROOT, bank_lock, verified_profiles, file_sha
from follow_host import (DIGEST, ID, LEDGER_BRANCH, SHA, STABLE, TRIPLES, api, host_source, ledger,
                         package_coverage, package_manifest, prepare_host, read_json, release_asset, write_json)


def filename(version, triple):
    return f"{ID}-{version}-{triple}.tar.gz"


def label(plan, triple, status, **data):
    return {"hostTag": plan["hostTag"], "hostCommit": plan["hostCommit"], "triple": triple, "status": status, **data}


def smoke(plan, triple, work, package, profiles=(), baseline=None):
    host = prepare_host(plan, triple, work)
    cmd = [sys.executable, str(ROOT / "scripts/smoke_host.py"), "--host-dir", str(host),
           "--host-tag", plan["hostTag"], "--host-sha", plan["hostCommit"], "--package", str(package),
           "--log", str(work / "smoke.log")]
    for profile in profiles:
        cmd.extend(("--required-profile", profile))
    if baseline:
        cmd.extend(("--baseline", str(baseline)))
    return subprocess.run(cmd, check=False).returncode == 0


def verify_old(plan, triple, work):
    previous = plan.get("previous")
    if plan["contract"]["problems"]:
        return label(plan, triple, "block", reason="host-contract")
    if not previous:
        return label(plan, triple, "rebuild", reason="no-published-package")
    assets = previous["assets"]
    version = previous["version"]
    report = read_json(release_asset(assets, "verification.json", work / "previous"))
    if report["pluginVersion"] != version or not report["validatedProfiles"] or not SHA.fullmatch(report["sourceCommit"]):
        return label(plan, triple, "block", reason="missing or invalid prior validation report")
    name = filename(version, triple)
    package = release_asset(assets, name, work / "previous")
    digest = file_sha(package)
    saved = report["platforms"][triple]
    manifest = package_manifest(package)
    if saved["sha256"] != digest or saved["asset"] != name or manifest["version"] != version:
        return label(plan, triple, "block", reason="prior package integrity")
    problem = package_coverage(manifest, plan["contract"]["host"], plan["hostTag"])
    if problem:
        return label(plan, triple, "rebuild" if problem != "wrong plugin identity" else "block", reason=problem)
    if not smoke(plan, triple, work, package, report["validatedProfiles"]):
        changed_sdk = bool(plan["contract"]["sdkChangedFiles"]) or report.get("sdkFingerprint") != plan["contract"]["sdkFingerprint"]
        return label(plan, triple, "rebuild" if changed_sdk else "block", reason="prior-package-smoke-failed")
    return label(plan, triple, "reuse", pluginVersion=version, asset=name, sha256=digest,
                 validatedProfiles=report["validatedProfiles"], sourceCommit=report["sourceCommit"])


def build(plan, triple, work, output, manifest_only=False):
    if plan["contract"]["problems"]:
        raise ValueError("Refusing build for unknown or unsupported contracts")
    source = host_source(plan["hostCommit"], work)
    version = plan["candidateVersion"]
    baseline = work / "baseline"
    cmd = [sys.executable, str(ROOT / "scripts/package.py"), "--host-source", str(source), "--host-tag", plan["hostTag"],
           "--host-sha", plan["hostCommit"], "--target", triple, "--version", version, "--out", str(output),
           "--baseline-out", str(baseline)]
    if manifest_only:
        if plan["contract"]["sdkChangedFiles"]:
            raise ValueError("Manifest-only rebuild may not ignore SDK changes")
        cmd.append("--keep-sdk")
    subprocess.run(cmd, check=True)
    package = output / filename(version, triple)
    manifest = package_manifest(package)
    problem = package_coverage(manifest, plan["contract"]["host"], plan["hostTag"])
    if problem or manifest["package"]["protocolVersion"] != plan["contract"]["candidateProtocol"]:
        raise ValueError(f"New package does not match its host contract: {problem}")
    profiles = verified_profiles()
    if not smoke(plan, triple, work, package, profiles, baseline / filename("0.0.0", triple)):
        raise RuntimeError("Candidate failed install/management/state upgrade/restart smoke; release blocked")
    return label(plan, triple, "verified", pluginVersion=version, asset=package.name, sha256=file_sha(package), validatedProfiles=profiles)


def rows_for(plan, files):
    rows = [read_json(path) for path in files]
    if {r.get("triple") for r in rows} != set(TRIPLES) or len(rows) != len(TRIPLES):
        raise ValueError("Exactly one result per native platform is required")
    if any((r.get("hostTag"), r.get("hostCommit")) != (plan["hostTag"], plan["hostCommit"]) for r in rows):
        raise ValueError("Mixed host tag or commit in verification results")
    return rows


def classify(plan, rows):
    statuses = {r["status"] for r in rows}
    if plan["contract"]["problems"] or not statuses or not statuses <= {"reuse", "rebuild"}:
        return "block"
    if statuses == {"reuse"}:
        return "reuse"
    if all(r.get("reason") == "engine-range" or r["status"] == "reuse" for r in rows) and not plan["contract"]["sdkChangedFiles"]:
        return "manifest-only"
    return "rebuild"


def reduce(args):
    plan = read_json(args.plan)
    rows = rows_for(plan, sorted(args.input.rglob("outcome.json")))
    route = classify(plan, rows)
    write_json(args.output, {"route":route,"results":rows})
    print(f"Decision: {route}\n{json.dumps(rows, indent=2)}")
    if args.github_output:
        with args.github_output.open("a") as f:
            print(f"route={route}", file=f)
    if route == "block":
        raise RuntimeError("Validation failed; automatic publication is blocked")


def assemble(plan, rows, route, source_commit, candidate_dir=None):
    if not SHA.fullmatch(source_commit):
        raise ValueError("Plugin build source is not pinned")
    if route == "reuse":
        version = plan["previous"]["version"]
        if classify(plan, rows) != "reuse" or len({row.get("sourceCommit") for row in rows}) != 1:
            raise ValueError("Not all existing packages passed smoke")
        source_commit = rows[0]["sourceCommit"]
    elif route in {"manifest-only", "rebuild"}:
        version = plan["candidateVersion"]
        if any(row["status"] != "verified" for row in rows):
            raise ValueError("Not all rebuilt packages passed verification")
    else:
        raise ValueError("Blocked route cannot be published")
    profiles = rows[0]["validatedProfiles"]
    if not profiles or any(row["validatedProfiles"] != profiles or row["pluginVersion"] != version for row in rows):
        raise ValueError("Production requires the same nonempty set of authorized model profiles across platforms")
    assets = {}
    packages = []
    for row in rows:
        triple, name, digest = row["triple"], row["asset"], row["sha256"]
        if name != filename(version, triple) or not DIGEST.fullmatch(digest):
            raise ValueError("Invalid asset name or digest")
        if candidate_dir:
            files = list(candidate_dir.rglob(name))
            if len(files) != 1 or file_sha(files[0]) != digest:
                raise ValueError(f"CI artifact digest mismatch: {name}")
            packages.append(files[0])
        assets[triple] = {"asset": name, "sha256": digest}
    entry = {"schemaVersion":1,"hostTag":plan["hostTag"],"hostCommit":plan["hostCommit"],"pluginId":ID,
             "pluginVersion":version, "sourceCommit":source_commit, "platforms":assets, "validatedProfiles":profiles,
             "detectorRevision": bank_lock()["revision"], "sdkFingerprint":plan["contract"]["sdkFingerprint"]}
    return entry, packages


def git_write(route, data):
    result = subprocess.run(["gh", "api", "--method", "POST" if "/git/refs" in route else "PUT", route, "--input", "-"],
                            input=json.dumps(data), text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError(f"Could not update compatibility ledger: {result.stderr.strip()}")


def publish(args):
    plan = read_json(args.plan)
    decision = read_json(args.decision)
    route = decision["route"]
    if route not in {"reuse", "rebuild", "manifest-only"}:
        raise ValueError(f"Refusing remote action for decision {route}")
    repo = plan["repository"]
    if repo != os.environ.get("GITHUB_REPOSITORY") or not SHA.fullmatch(args.source):
        raise ValueError("Publication must target the current GitHub repository at an exact source SHA")
    old = ledger(repo, plan["hostTag"])
    if old is not None and (route != "reuse" or old["hostCommit"] != plan["hostCommit"]):
        raise ValueError("Refusing to replace an existing compatibility entry")
    if route == "reuse":
        rows = decision["results"]
        if len(rows) != len(TRIPLES) or {r["triple"] for r in rows} != set(TRIPLES) or any(r["hostCommit"] != plan["hostCommit"] or r["hostTag"] != plan["hostTag"] for r in rows):
            raise ValueError("Incomplete reuse verification")
        entry, packages = assemble(plan, rows, route, args.source)
        # Reuse the actual published detector metadata, not the data pin of the current checkout.
        previous = plan["previous"]
        prior = read_json(release_asset(previous["assets"], "verification.json", args.output / "previous"))
        entry["detectorRevision"] = prior["detectorRevision"]
    else:
        candidates = args.input
        if candidates is None or not verified_profiles():
            raise ValueError("No authorized validation evidence or candidate packages: production release blocked")
        rows = rows_for(plan, sorted(candidates.rglob("candidate.json")))
        entry, packages = assemble(plan, rows, route, args.source, candidates)
        if entry["validatedProfiles"] != verified_profiles():
            raise ValueError("Candidate did not validate this checkout's qualified profiles")
    write_json(args.output / "verification.json", entry)
    if not args.write:
        print("Dry run: no release or compatibility ledger was changed")
        return
    if os.environ.get("CPR_ENABLE_PUBLISH") != "true" or os.environ.get("GITHUB_ACTIONS") != "true":
        raise ValueError("Remote publication is disabled")
    if old is not None:
        if old["platforms"] != entry["platforms"] or old["pluginVersion"] != entry["pluginVersion"]:
            raise ValueError("Recorded verified packages changed")
        print("Verified existing immutable compatibility entry; nothing to change")
        return
    if packages:
        tag = "v" + entry["pluginVersion"]
        if not STABLE.fullmatch(tag) or api(f"repos/{repo}/git/ref/tags/{tag}", missing=True) or api(f"repos/{repo}/releases/tags/{tag}", missing=True):
            raise ValueError("Plugin version is already reserved; refusing to overwrite")
        checksums = args.output / "checksums.txt"
        checksums.write_text("".join(f"{entry['platforms'][triple]['sha256']}  {filename(entry['pluginVersion'], triple)}\n" for triple in TRIPLES))
        command = ["gh", "release", "--repo", repo]
        note = f"Native plugin verified on CPR {entry['hostTag']} ({entry['hostCommit']}). Install only after checking verification.json and checksums.txt. No automatic production upgrade."
        subprocess.run([*command, "create", tag, "--draft", "--target", entry["sourceCommit"], "--title", tag,
                        "--notes", note, *map(str, (*packages, checksums, args.output / "verification.json"))], check=True)
        subprocess.run([*command, "edit", tag, "--draft=false", "--latest"], check=True)
    branch = f"repos/{repo}/git/ref/heads/{LEDGER_BRANCH}"
    if api(branch, missing=True) is None:
        git_write(f"repos/{repo}/git/refs", {"ref": "refs/heads/" + LEDGER_BRANCH, "sha": args.source})
    git_write(f"repos/{repo}/contents/compatibility/{entry['hostTag']}.json", {
        "message": f"Record verified compatibility with CPR {entry['hostTag']}", "branch": LEDGER_BRANCH,
        "content":base64.b64encode((args.output / "verification.json").read_bytes()).decode(),
    })


def main():
    p = argparse.ArgumentParser(description=__doc__)
    subs = p.add_subparsers(dest="command", required=True)
    for cmd in ("old", "build", "decide", "publish"):
        parser = subs.add_parser(cmd)
        parser.add_argument("--plan", type=pathlib.Path, required=True)
        parser.add_argument("--output", type=pathlib.Path, required=True)
        if cmd in {"old", "build"}:
            parser.add_argument("--triple", choices=TRIPLES, required=True)
            parser.add_argument("--work", type=pathlib.Path, required=True)
        if cmd == "build":
            parser.add_argument("--manifest-only", action="store_true")
        if cmd == "decide":
            parser.add_argument("--input", type=pathlib.Path, required=True)
            parser.add_argument("--github-output", type=pathlib.Path)
        if cmd == "publish":
            parser.add_argument("--input", type=pathlib.Path)
            parser.add_argument("--decision", type=pathlib.Path, required=True)
            parser.add_argument("--source", required=True)
            parser.add_argument("--write", action="store_true")
    args = p.parse_args()
    if args.command in {"old", "build"}:
        plan = read_json(args.plan)
        if args.command == "old":
            row = verify_old(plan, args.triple, args.work)
            name = "outcome.json"
        else:
            row = build(plan, args.triple, args.work, args.output, args.manifest_only)
            name = "candidate.json"
        write_json(args.output / name, row)
        print(json.dumps(row, indent=2))
    else:
        (reduce if args.command == "decide" else publish)(args)


if __name__ == "__main__":
    main()
