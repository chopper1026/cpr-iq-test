#!/usr/bin/env python3
"""Model the three release paths, then gate any remote mutation on all three platforms."""

import argparse
import base64
import json
import os
import pathlib
import re
import subprocess
import sys
import time
import tomllib

from detector_assets import DATA, ROOT, bank_lock, file_sha, good
from follow_host import (DIGEST, ID, LEDGER_BRANCH, PRERELEASE, SHA, STABLE, TRIPLES, UPSTREAM, api,
                         download_release_asset, engine_matches, host_source, ledger, own_releases, package_coverage,
                         package_manifest, prepare_host, read_json, ref_commit, release_asset, write_json)

WORKFLOW_PATH = ".github/workflows/follow-host.yml"


def filename(version, triple):
    return f"{ID}-{version}-{triple}.tar.gz"


def label(plan, triple, status, **data):
    return {"hostTag": plan["hostTag"], "hostCommit": plan["hostCommit"], "triple": triple, "status": status, **data}


def smoke(plan, triple, work, package, baseline=None):
    host = prepare_host(plan, triple, work)
    cmd = [sys.executable, str(ROOT / "scripts/smoke_host.py"), "--host-dir", str(host),
           "--host-tag", plan["hostTag"], "--host-sha", plan["hostCommit"], "--package", str(package),
           "--log", str(work / "smoke.log")]
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
    if report["pluginVersion"] != version or not SHA.fullmatch(report["sourceCommit"]):
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
    if not smoke(plan, triple, work, package):
        changed_sdk = bool(plan["contract"]["sdkChangedFiles"]) or report.get("sdkFingerprint") != plan["contract"]["sdkFingerprint"]
        return label(plan, triple, "rebuild" if changed_sdk else "block", reason="prior-package-smoke-failed")
    return label(plan, triple, "reuse", pluginVersion=version, asset=name, sha256=digest,
                 sourceCommit=report["sourceCommit"])


def inspect_glibc_versions(archive):
    import tarfile, tempfile
    with tarfile.open(archive, "r:gz") as tar:
        member = next((m for m in tar if m.name in ("bin/plugin", "./bin/plugin")), None)
        if member is None:
            return []
        with tempfile.TemporaryDirectory(prefix="cpr-glibc-") as scratch:
            extracted = pathlib.Path(scratch) / "plugin"
            extracted.write_bytes(tar.extractfile(member).read())
            result = subprocess.run(["readelf", "-V", str(extracted)], text=True, capture_output=True)
            if result.returncode != 0:
                return []
            versions = sorted(
                set(re.findall(r"GLIBC_(\d+(?:\.\d+)+)", result.stdout)),
                key=lambda v: tuple(int(part) for part in v.split(".")),
            )
            return [f"GLIBC_{v}" for v in versions]


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
    if not smoke(plan, triple, work, package, baseline / filename("0.0.0", triple)):
        raise RuntimeError("Candidate failed install/management/state upgrade/restart smoke; release blocked")
    extra = {}
    if triple.endswith("linux-gnu"):
        glibc = inspect_glibc_versions(package)
        if glibc:
            extra["maxGlibc"] = glibc[-1]
            extra["glibcVersions"] = glibc
            print(f"Linux binary glibc requirements ({triple}): max={glibc[-1]} ({', '.join(glibc)})")
    return label(plan, triple, "verified", pluginVersion=version, asset=package.name, sha256=file_sha(package),
                 **extra)


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
    files = sorted(args.input.rglob("outcome.json")) if args.input and args.input.exists() else []
    if not files and plan["contract"]["problems"]:
        rows = [label(plan, triple, "block", reason="host-contract") for triple in TRIPLES]
    elif not files and plan.get("previous") is None:
        rows = [label(plan, triple, "rebuild", reason="no-published-package") for triple in TRIPLES]
    else:
        rows = rows_for(plan, files)
    route = classify(plan, rows)
    payload = {"route": route, "results": rows}
    write_json(args.output, payload)
    print(f"Decision: {route}\n{json.dumps(rows, indent=2)}")
    if args.github_output:
        with args.github_output.open("a") as f:
            print(f"route={route}", file=f)
    if route == "block":
        raise RuntimeError("Validation failed; automatic publication is blocked")


def assemble(plan, rows, route, source_commit, candidate_dir=None):
    if not SHA.fullmatch(source_commit):
        raise ValueError("Plugin build source is not pinned")
    prerelease = bool(plan.get("prerelease"))
    if route == "reuse":
        if prerelease:
            raise ValueError("Prerelease must build fresh candidates instead of reusing a stable release")
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
    if any(row["pluginVersion"] != version for row in rows):
        raise ValueError("Platforms disagree on plugin version")
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
        meta = {"asset": name, "sha256": digest}
        if row.get("maxGlibc"):
            meta["maxGlibc"] = row["maxGlibc"]
        assets[triple] = meta
    entry = {"schemaVersion": 1, "hostTag": plan["hostTag"], "hostCommit": plan["hostCommit"], "pluginId": ID,
             "pluginVersion": version, "sourceCommit": source_commit, "platforms": assets,
             "detectorRevision": bank_lock()["revision"], "sdkFingerprint": plan["contract"]["sdkFingerprint"]}
    if prerelease:
        entry["prerelease"] = True
    return entry, packages


TRANSIENT_GH_TOKENS = (
    "http 500",
    "http 502",
    "http 503",
    "http 504",
    "http 429",
    "server error",
    "bad gateway",
    "service unavailable",
    "gateway timeout",
    "unexpected eof",
    "unexpected end of json",
    "connection reset",
    "tls handshake timeout",
    "error updating policy",
)


def is_transient_gh_error(text, *, allow_not_found=False):
    lowered = (text or "").lower()
    if any(token in lowered for token in TRANSIENT_GH_TOKENS):
        return True
    if allow_not_found and ("release not found" in lowered or "http 404" in lowered):
        return True
    return False


def run_gh_retry(cmd, *, attempts=6, check_done=None, allow_not_found=False):
    for attempt in range(attempts):
        try:
            return subprocess.run(cmd, check=True, text=True, capture_output=True)
        except subprocess.CalledProcessError as error:
            detail = f"{error.stderr or ''}\n{error.stdout or ''}\n{error}"
            if check_done is not None:
                try:
                    if check_done():
                        return None
                except Exception:
                    pass
            if attempt + 1 >= attempts or not is_transient_gh_error(detail, allow_not_found=allow_not_found):
                if error.stderr:
                    sys.stderr.write(error.stderr)
                if error.stdout:
                    sys.stdout.write(error.stdout)
                raise
            time.sleep(min(2 * (attempt + 1), 10))


def git_write(route, data):
    last_err = ""
    for attempt in range(5):
        result = subprocess.run(["gh", "api", "--method", "POST" if "/git/refs" in route else "PUT", route, "--input", "-"],
                                input=json.dumps(data), text=True, capture_output=True)
        if result.returncode == 0:
            return
        last_err = result.stderr.strip()
        if attempt + 1 >= 5 or not is_transient_gh_error(last_err):
            raise RuntimeError(f"Could not update compatibility ledger: {last_err}")
        time.sleep(min(2 * (attempt + 1), 10))
    raise RuntimeError(f"Could not update compatibility ledger: {last_err}")


def verify_remote_release(release, expected_files, work, expect_published, expect_prerelease=False):
    if bool(release.get("prerelease")) != expect_prerelease:
        raise ValueError(f"Release prerelease flag mismatch: expected {expect_prerelease}, got {release.get('prerelease')}")
    if expect_published:
        if release.get("draft") or not release.get("published_at"):
            raise ValueError("Expected a published release, but release is still draft or unpublished")
    else:
        if not release.get("draft"):
            raise ValueError("Expected a draft release prior to promotion")
    remote_assets = {asset["name"]: asset for asset in release.get("assets", [])}
    missing = sorted(set(expected_files) - set(remote_assets))
    extra = sorted(set(remote_assets) - set(expected_files))
    if missing or extra:
        raise ValueError(f"Remote release asset set mismatch (missing={missing}, extra={extra})")
    verified = {}
    for name, local_path in sorted(expected_files.items()):
        asset = remote_assets[name]
        expected_size = local_path.stat().st_size
        expected_sha = file_sha(local_path)
        if asset.get("size") != expected_size:
            raise ValueError(f"Remote asset size mismatch for {name}: expected {expected_size}, got {asset.get('size')}")
        raw_digest = (asset.get("digest") or "").removeprefix("sha256:")
        if DIGEST.fullmatch(raw_digest):
            if raw_digest != expected_sha:
                raise ValueError(f"Remote asset SHA-256 mismatch for {name}")
            method = "api-digest"
        else:
            downloaded = download_release_asset(asset, work / name, expected=expected_sha)
            if downloaded.stat().st_size != expected_size or file_sha(downloaded) != expected_sha:
                raise ValueError(f"Downloaded remote asset mismatch for {name}")
            method = "download"
        verified[name] = {"size": expected_size, "sha256": expected_sha, "verifiedBy": method}
    return verified


def publish(args):
    plan = read_json(args.plan)
    decision = read_json(args.decision)
    route = decision["route"]
    prerelease = bool(plan.get("prerelease"))
    if route not in {"reuse", "rebuild", "manifest-only"}:
        raise ValueError(f"Refusing remote action for decision {route}")
    repo = plan["repository"]
    if repo != os.environ.get("GITHUB_REPOSITORY") or not SHA.fullmatch(args.source):
        raise ValueError("Publication must target the current GitHub repository at an exact source SHA")
    old = None if prerelease else ledger(repo, plan["hostTag"])
    if old is not None and old["hostCommit"] != plan["hostCommit"]:
        raise ValueError("Refusing to replace an existing compatibility entry")
    if route == "reuse":
        if prerelease:
            raise ValueError("Prerelease does not support reuse route")
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
        if candidates is None:
            raise ValueError("No candidate packages supplied: release blocked")
        rows = rows_for(plan, sorted(candidates.rglob("candidate.json")))
        entry, packages = assemble(plan, rows, route, args.source, candidates)
    if old is not None and (
        old["platforms"] != entry["platforms"]
        or old["pluginVersion"] != entry["pluginVersion"]
        or old.get("sourceCommit") != entry["sourceCommit"]
    ):
        raise ValueError("Refusing to replace an existing compatibility entry")
    verification_file = args.output / "verification.json"
    write_json(verification_file, entry)
    receipt = {
        "schemaVersion": 1,
        "stage": "prepared",
        "dryRun": not args.write,
        "prerelease": prerelease,
        "route": route,
        "repository": repo,
        "hostTag": entry["hostTag"],
        "hostCommit": entry["hostCommit"],
        "pluginVersion": entry["pluginVersion"],
        "sourceCommit": entry["sourceCommit"],
        "releaseTag": ("v" + entry["pluginVersion"]) if packages else None,
        "releaseUrl": None,
        "ledgerBranch": None if prerelease else LEDGER_BRANCH,
        "ledgerPath": None if prerelease else f"compatibility/{entry['hostTag']}.json",
        "ledgerUpdated": False,
        "remoteAssets": {},
        "resumedFromRunId": getattr(args, "resume_run_id", None) or None,
    }
    if not args.write:
        receipt["stage"] = "dry-run-verified"
        write_json(args.output / "publish-receipt.json", receipt)
        print("Dry run: no release or compatibility ledger was changed")
        return
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise ValueError("Remote publication is disabled")
    if prerelease:
        if not getattr(args, "allow_prerelease", False):
            raise ValueError("Prerelease publication requires explicit --allow-prerelease authorization")
    elif os.environ.get("CPR_ENABLE_PUBLISH") != "true":
        raise ValueError("Remote publication is disabled")
    if old is not None:
        if old["platforms"] != entry["platforms"] or old["pluginVersion"] != entry["pluginVersion"]:
            raise ValueError("Recorded verified packages changed")
        receipt["stage"] = "verified"
        receipt["note"] = "existing immutable compatibility entry verified"
        write_json(args.output / "publish-receipt.json", receipt)
        print("Verified existing immutable compatibility entry; nothing to change")
        return
    last_stage = "prepared"
    try:
        if packages:
            tag = "v" + entry["pluginVersion"]
            tag_pattern = PRERELEASE if prerelease else STABLE
            if not tag_pattern.fullmatch(tag):
                raise ValueError(f"Plugin version tag {tag} is invalid for {'prerelease' if prerelease else 'stable'} publication")
            checksums = args.output / "checksums.txt"
            checksums.write_text("".join(f"{entry['platforms'][triple]['sha256']}  {filename(entry['pluginVersion'], triple)}\n" for triple in TRIPLES))
            expected_files = {path.name: path for path in (*packages, checksums, verification_file)}
            existing_tag = api(f"repos/{repo}/git/ref/tags/{tag}", missing=True)
            matching_releases = [rel for rel in own_releases(repo) if rel.get("tag_name") == tag]
            drafts = [rel for rel in matching_releases if rel.get("draft")]
            if drafts or len(matching_releases) > 1:
                raise ValueError(f"Draft or duplicate release already exists for {tag}; inspect or remove it before retrying")
            if matching_releases:
                published = matching_releases[0]
                if not existing_tag or ref_commit(repo, "tags", tag) != entry["sourceCommit"]:
                    raise ValueError(f"Plugin version {tag} is already reserved at a different commit; refusing to overwrite")
                verified_assets = verify_remote_release(
                    published, expected_files, args.output / "remote-verify",
                    expect_published=True, expect_prerelease=prerelease,
                )
                receipt["remoteAssets"] = verified_assets
                receipt["releaseUrl"] = published.get("html_url")
                receipt["reusedPublishedRelease"] = True
                last_stage = "published"
                receipt["stage"] = last_stage
            else:
                if existing_tag:
                    raise ValueError(f"Plugin version {tag} tag is already reserved without a matching release; refusing to overwrite")
                command = ["gh", "release", "--repo", repo]
                if prerelease:
                    note = (
                        f"Development preview prerelease verified on CPR {entry['hostTag']} ({entry['hostCommit']}). "
                        "Does not occupy a stable version or update cpr-compatibility. "
                        "Install only after checking verification.json and checksums.txt."
                    )
                    create_flags = ["--draft", "--prerelease"]
                    promote_flags = ["--draft=false", "--prerelease", "--latest=false"]
                else:
                    note = f"Native plugin verified on CPR {entry['hostTag']} ({entry['hostCommit']}). Install only after checking verification.json and checksums.txt. No automatic production upgrade."
                    create_flags = ["--draft"]
                    promote_flags = ["--draft=false", "--latest"]
                run_gh_retry(
                    [*command, "create", tag, *create_flags, "--target", entry["sourceCommit"], "--title", tag,
                     "--notes", note],
                    check_done=lambda: len([rel for rel in own_releases(repo) if rel.get("tag_name") == tag and rel.get("draft")]) == 1,
                )
                last_stage = "draft-created"
                receipt["stage"] = last_stage
                for asset_path in (*packages, checksums, verification_file):
                    run_gh_retry([*command, "upload", tag, str(asset_path), "--clobber"], allow_not_found=True)
                created_drafts = [rel for rel in own_releases(repo) if rel.get("tag_name") == tag and rel.get("draft")]
                if len(created_drafts) != 1:
                    raise RuntimeError(f"Expected exactly one draft release for {tag} after upload, found {len(created_drafts)}")
                draft = created_drafts[0]
                receipt["releaseUrl"] = draft.get("html_url")
                receipt["remoteAssets"] = verify_remote_release(
                    draft, expected_files, args.output / "remote-verify",
                    expect_published=False, expect_prerelease=prerelease,
                )
                last_stage = "draft-verified"
                receipt["stage"] = last_stage
                run_gh_retry(
                    [*command, "edit", tag, *promote_flags],
                    allow_not_found=True,
                    check_done=lambda: (
                        (rel := api(f"repos/{repo}/releases/tags/{tag}", missing=True)) is not None
                        and not rel.get("draft")
                    ),
                )
                published = api(f"repos/{repo}/releases/tags/{tag}")
                if (
                    published.get("draft")
                    or bool(published.get("prerelease")) != prerelease
                    or not published.get("published_at")
                    or ref_commit(repo, "tags", tag) != entry["sourceCommit"]
                ):
                    raise RuntimeError(f"Post-promotion release verification failed for {tag}")
                receipt["releaseUrl"] = published.get("html_url")
                last_stage = "published"
                receipt["stage"] = last_stage
        if not prerelease:
            branch = f"repos/{repo}/git/ref/heads/{LEDGER_BRANCH}"
            if api(branch, missing=True) is None:
                git_write(f"repos/{repo}/git/refs", {"ref": "refs/heads/" + LEDGER_BRANCH, "sha": args.source})
            git_write(f"repos/{repo}/contents/compatibility/{entry['hostTag']}.json", {
                "message": f"Record verified compatibility with CPR {entry['hostTag']}", "branch": LEDGER_BRANCH,
                "content": base64.b64encode(verification_file.read_bytes()).decode(),
            })
            receipt["ledgerUpdated"] = True
        last_stage = "verified"
        receipt["stage"] = last_stage
    except Exception as error:
        receipt["stage"] = "failed"
        receipt["lastCompletedStage"] = last_stage
        receipt["error"] = str(error)
        run_id = os.environ.get("GITHUB_RUN_ID")
        if run_id:
            receipt["resumeHint"] = f"workflow_dispatch follow-host.yml with resume_run_id={run_id}"
        write_json(args.output / "publish-receipt.json", receipt)
        raise
    write_json(args.output / "publish-receipt.json", receipt)


def list_run_jobs(repo, run_id):
    jobs = []
    for page in range(1, 11):
        data = api(f"repos/{repo}/actions/runs/{run_id}/jobs?per_page=100&page={page}")
        batch = data.get("jobs", [])
        jobs.extend(batch)
        if len(batch) < 100:
            return jobs
    raise ValueError("Workflow run job listing exceeded limit")


def verify_resume(args):
    repo = args.repository
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
        raise ValueError("A valid GitHub repository is required")
    if not re.fullmatch(r"[1-9]\d*", str(args.run_id)):
        raise ValueError("resume_run_id must be a positive integer")
    if not SHA.fullmatch(args.source):
        raise ValueError("Source commit must be a pinned 40-char SHA")
    run = api(f"repos/{repo}/actions/runs/{args.run_id}")
    if run.get("path") != WORKFLOW_PATH:
        raise ValueError(f"Run {args.run_id} belongs to {run.get('path')!r}, not {WORKFLOW_PATH}")
    if run.get("head_branch") != args.default_branch:
        raise ValueError(f"Run {args.run_id} was not executed on default branch {args.default_branch}")
    if run.get("head_sha") != args.source:
        raise ValueError(f"Run {args.run_id} head_sha {run.get('head_sha')} does not match current commit {args.source}")
    jobs = list_run_jobs(repo, args.run_id)
    by_name = {job["name"]: job for job in jobs}
    for required in ("discover", "host-contract", "decide"):
        job = by_name.get(required)
        if job is None or job.get("conclusion") != "success":
            raise ValueError(f"Resumed run {args.run_id} missing successful '{required}' job")
    route = None
    needs_candidates = False
    if args.decision:
        decision = read_json(args.decision)
        route = decision.get("route")
        if route not in {"reuse", "rebuild", "manifest-only"}:
            raise ValueError(f"Resumed run decision route {route!r} cannot be published")
        if route == "reuse":
            for triple in TRIPLES:
                matched = [job for job in jobs if job["name"].startswith("old") and triple in job["name"] and job.get("conclusion") == "success"]
                if len(matched) != 1:
                    raise ValueError(f"Resumed reuse run missing successful 'old' job for {triple}")
        else:
            needs_candidates = True
            for triple in TRIPLES:
                matched = [job for job in jobs if job["name"].startswith("rebuild") and triple in job["name"] and job.get("conclusion") == "success"]
                if len(matched) != 1:
                    raise ValueError(f"Resumed rebuild run missing successful 'rebuild' job for {triple}")
    summary = {
        "runId": str(args.run_id),
        "repository": repo,
        "headBranch": run["head_branch"],
        "headSha": run["head_sha"],
        "route": route,
        "needsCandidates": needs_candidates,
    }
    if args.output:
        write_json(args.output, summary)
    if args.github_output:
        with args.github_output.open("a") as f:
            if route:
                print(f"route={route}", file=f)
            print(f"needs_candidates={str(needs_candidates).lower()}", file=f)
    print(json.dumps(summary, indent=2))


def preflight(args):
    author = read_json(ROOT / "plugin.json")
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text())
    lock = bank_lock()
    local_assets = {name: good(DATA / name, details) for name, details in lock["files"].items()}
    blockers = []
    if cargo["package"]["version"] != author["version"]:
        blockers.append(f"version-mismatch: plugin.json={author['version']} Cargo.toml={cargo['package']['version']}")
    git_info = {"available": False, "headSha": None, "dirtyFiles": []}
    if (ROOT / ".git").exists():
        head = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True, capture_output=True)
        status = subprocess.run(["git", "-C", str(ROOT), "status", "--porcelain"], text=True, capture_output=True)
        if head.returncode == 0:
            git_info["available"] = True
            git_info["headSha"] = head.stdout.strip()
            git_info["dirtyFiles"] = [line for line in status.stdout.splitlines() if line.strip()]
            if git_info["dirtyFiles"]:
                blockers.append("dirty-working-tree")
    report = {
        "pluginId": ID,
        "pluginVersion": author["version"],
        "cargoVersion": cargo["package"]["version"],
        "engines": author["engines"]["codex-proxy-rs"],
        "sdkPin": cargo["dependencies"]["gateway-plugin-sdk"]["rev"],
        "detectorRevision": lock["revision"],
        "localAssetsReady": all(local_assets.values()),
        "localAssets": local_assets,
        "git": git_info,
        "publishReady": len(blockers) == 0,
        "blockers": blockers,
    }
    if args.online:
        tag_arg = args.tag or ""
        if tag_arg and not STABLE.fullmatch(tag_arg):
            raise ValueError("Use a stable upstream tag vX.Y.Z")
        rel = api(f"repos/{UPSTREAM}/releases/" + ("tags/" + tag_arg if tag_arg else "latest"))
        tag = rel["tag_name"]
        sha = ref_commit(UPSTREAM, "tags", tag)
        upstream_info = {
            "hostTag": tag,
            "hostCommit": sha,
            "engineCoversUpstream": engine_matches(author["engines"]["codex-proxy-rs"], tag),
        }
        if args.repository:
            known = ledger(args.repository, tag)
            releases = own_releases(args.repository)
            series = author["version"].rsplit(".", 1)[0]
            series_patches = [int(r["tag_name"].rsplit(".", 1)[1]) for r in releases
                              if STABLE.fullmatch(r["tag_name"]) and r["tag_name"].startswith(f"v{series}.")]
            upstream_info["ledgerRecorded"] = known is not None
            upstream_info["ledgerCommitMatches"] = (known["hostCommit"] == sha) if known else None
            upstream_info["draftReleases"] = [r["tag_name"] for r in releases if r.get("draft")]
            upstream_info["nextCandidateVersion"] = f"{series}.{1 + max(series_patches, default=-1)}"
        report["upstream"] = upstream_info
    if args.output:
        write_json(args.output, report)
    print(json.dumps(report, indent=2, ensure_ascii=False))
    if args.strict and blockers:
        raise RuntimeError("Preflight blocked: " + ", ".join(blockers))


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
            parser.add_argument("--input", type=pathlib.Path)
            parser.add_argument("--github-output", type=pathlib.Path)
        if cmd == "publish":
            parser.add_argument("--input", type=pathlib.Path)
            parser.add_argument("--decision", type=pathlib.Path, required=True)
            parser.add_argument("--source", required=True)
            parser.add_argument("--resume-run-id", default="")
            parser.add_argument("--allow-prerelease", action="store_true")
            parser.add_argument("--write", action="store_true")
    resume = subs.add_parser("verify-resume")
    resume.add_argument("--repository", required=True)
    resume.add_argument("--run-id", required=True)
    resume.add_argument("--source", required=True)
    resume.add_argument("--default-branch", required=True)
    resume.add_argument("--decision", type=pathlib.Path)
    resume.add_argument("--output", type=pathlib.Path)
    resume.add_argument("--github-output", type=pathlib.Path)
    pre = subs.add_parser("preflight")
    pre.add_argument("--online", action="store_true")
    pre.add_argument("--repository", default="")
    pre.add_argument("--tag", default="")
    pre.add_argument("--output", type=pathlib.Path)
    pre.add_argument("--strict", action="store_true")
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
    elif args.command == "decide":
        reduce(args)
    elif args.command == "publish":
        publish(args)
    elif args.command == "verify-resume":
        verify_resume(args)
    else:
        preflight(args)


if __name__ == "__main__":
    main()
