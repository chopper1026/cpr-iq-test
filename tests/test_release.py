"""Offline release rehearsals: no GitHub write, production secret or OAuth is used."""

import argparse
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "scripts"))

from detector_assets import file_sha
from follow_host import (TRIPLES, discovery, engine_matches, package_coverage, package_source_hash, read_json,
                         write_json)
from release_gate import (WORKFLOW_PATH, assemble, classify, filename, label, preflight, publish, reduce, rows_for,
                          run_gh_retry, verify_old, verify_remote_release, verify_resume)
from sync_upstreams import build_commit_message, lm_changed_paths, update_cargo_dep_rev, update_rust_version


def make_remote_release(tag, files, *, draft=False, prerelease=False, published_at="2026-11-25T13:00:00Z", include_digest=True):
    assets = []
    for path in files:
        asset = {
            "name": path.name,
            "size": path.stat().st_size,
            "url": f"https://api.github.com/repos/example/plugin/releases/assets/{path.name}",
            "browser_download_url": f"https://github.com/example/plugin/releases/download/{tag}/{path.name}",
        }
        if include_digest:
            asset["digest"] = "sha256:" + file_sha(path)
        assets.append(asset)
    return {
        "id": 101,
        "tag_name": tag,
        "draft": draft,
        "prerelease": prerelease,
        "published_at": None if draft else published_at,
        "html_url": f"https://github.com/example/plugin/releases/tag/{tag}",
        "assets": assets,
    }


class FollowRelease(unittest.TestCase):
    def setUp(self):
        self.sha = "a" * 40
        self.default_version = "0.1.8"
        self.plan = {"hostTag": "v3.54.6", "hostCommit": self.sha, "candidateVersion": "0.1.9",
                     "previous": {"version": self.default_version, "assets": {}},
                     "contract": {"problems": [], "sdkChangedFiles": [], "sdkFingerprint": "ef",
                                  "host": {"manifest_schema_versions": [2], "protocol_versions": [7],
                                           "capabilities": [{"capability": "maintenance", "versions": [1]}]}}}
        self.author = {"publisher": "cpr-iq-test", "name": "account-fingerprint", "version": self.default_version,
                       "engines": {"codex-proxy-rs": "~3.54.5"}, "manifestVersion": 2,
                       "package": {"protocolVersion": 7}, "contributes": {"maintenance": {"version": 1}}}

    def status(self, triple, status, **more):
        version = self.plan["candidateVersion"] if status == "verified" else self.default_version
        return label(self.plan, triple, status, pluginVersion=version, asset=filename(version, triple), sha256="b" * 64,
                     sourceCommit=self.sha, **more)

    def make_publish_fixture(self, root, *, write=True, route="rebuild"):
        self.plan["repository"] = "example/plugin"
        paths = argparse.Namespace(plan=root / "plan.json", decision=root / "decision.json", input=root / "matrix",
                                   output=root / "out", source=self.sha, resume_run_id="", write=write)
        write_json(paths.plan, self.plan)
        write_json(paths.decision, {"route": route, "results": []})
        packages = []
        for triple in TRIPLES:
            row = self.status(triple, "verified")
            folder = paths.input / triple
            folder.mkdir(parents=True)
            package = folder / row["asset"]
            package.write_text(triple)
            row["sha256"] = file_sha(package)
            write_json(folder / "candidate.json", row)
            packages.append(package)
        return paths, packages

    def test_existing_installation_continues_without_new_assets(self):
        reuse = [self.status(t, "reuse") for t in TRIPLES]
        self.assertEqual(classify(self.plan, reuse), "reuse")
        entry, packages = assemble(self.plan, reuse, "reuse", self.sha)
        self.assertEqual(packages, [])
        self.assertEqual(entry["pluginVersion"], self.default_version)
        self.assertEqual(set(entry["platforms"]), set(TRIPLES))

    def test_only_outside_engines_rebuilds_without_changing_sdk(self):
        self.plan["hostTag"] = "v3.55.0"
        problem = package_coverage(self.author, self.plan["contract"]["host"], self.plan["hostTag"])
        self.assertEqual(problem, "engine-range")
        old = [self.status(t, "rebuild", reason=problem) for t in TRIPLES]
        self.assertEqual(classify(self.plan, old), "manifest-only")
        self.plan["contract"]["sdkChangedFiles"] = ["src/message.rs"]
        self.assertEqual(classify(self.plan, old), "rebuild")
        rows = [self.status(t, "verified") for t in TRIPLES]
        entry, _ = assemble(self.plan, rows, "rebuild", self.sha)
        self.assertEqual(entry["pluginVersion"], self.plan["candidateVersion"])

    def test_blocked_host_or_version_mismatch_is_never_a_release(self):
        row = [self.status(t, "reuse") for t in TRIPLES]
        row[2]["status"] = "block"
        self.assertEqual(classify(self.plan, row), "block")
        with self.assertRaisesRegex(ValueError, "Blocked route"):
            assemble(self.plan, row, "block", self.sha)
        row = [self.status(t, "verified") for t in TRIPLES]
        row[1]["pluginVersion"] = "9.9.9"
        with self.assertRaisesRegex(ValueError, "disagree on plugin version"):
            assemble(self.plan, row, "rebuild", self.sha)

    def test_matrix_requires_all_platforms_from_same_host_commit(self):
        with tempfile.TemporaryDirectory() as scratch:
            paths = [pathlib.Path(scratch) / f"{idx}.json" for idx in range(3)]
            for p, t in zip(paths, TRIPLES, strict=True):
                write_json(p, self.status(t, "reuse"))
            self.assertEqual(len(rows_for(self.plan, paths)), 3)
            with self.assertRaises(ValueError):
                rows_for(self.plan, paths[:2])
            other = self.status(TRIPLES[2], "reuse")
            other["hostCommit"] = "f" * 40
            write_json(paths[-1], other)
            with self.assertRaisesRegex(ValueError, "Mixed host"):
                rows_for(self.plan, paths)

    def test_engine_floor_and_minor_fence(self):
        for tag, expect in [("v3.54.4", False), ("v3.54.5", True), ("v3.54.12", True), ("v3.55.0", False),
                            ("v3.54.6-rc.1", False), ("v03.54.5", False)]:
            self.assertEqual(engine_matches("~3.54.5", tag), expect)
        self.assertFalse(engine_matches("*", "v3.54.5"))
        self.author["package"]["protocolVersion"] = 6
        self.assertEqual(package_coverage(self.author, self.plan["contract"]["host"], "v3.54.5"), "rpc-contract")

    def test_old_package_integrity_and_smoke_are_required_not_claimed(self):
        triple = TRIPLES[0]
        artifact_name = filename(self.default_version, triple)
        verification = {"pluginVersion": self.default_version,
                        "sourceCommit": self.sha, "sdkFingerprint": "ef",
                        "platforms": {triple: {"asset": artifact_name, "sha256": "b" * 64}}}
        with (patch("release_gate.release_asset", return_value=pathlib.Path("fixture.tar.gz")),
              patch("release_gate.read_json", return_value=verification),
              patch("release_gate.file_sha", return_value="b" * 64),
              patch("release_gate.package_manifest", return_value=self.author),
              patch("release_gate.smoke", return_value=True) as run):
            self.assertEqual(verify_old(self.plan, triple, pathlib.Path("dummy"))["status"], "reuse")
            run.assert_called_once()
            run.return_value = False
            self.assertEqual(verify_old(self.plan, triple, pathlib.Path("dummy"))["status"], "block")
            self.plan["contract"]["sdkChangedFiles"] = ["src/manifest.rs"]
            self.assertEqual(verify_old(self.plan, triple, pathlib.Path("dummy"))["status"], "rebuild")
            verification["platforms"][triple]["sha256"] = "a" * 64
            self.assertEqual(verify_old(self.plan, triple, pathlib.Path("dummy"))["reason"], "prior package integrity")

    def test_publish_rebuild_only_after_all_artifact_digests(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            paths, packages = self.make_publish_fixture(root)
            tag = "v" + self.plan["candidateVersion"]
            state = {"created": False}

            def fake_run(cmd, **kwargs):
                if "create" in cmd:
                    state["created"] = True

            def fake_own_releases(_repo):
                if not state["created"]:
                    return []
                files = (*packages, paths.output / "checksums.txt", paths.output / "verification.json")
                return [make_remote_release(tag, files, draft=True)]

            def fake_api(route, missing=False):
                if route == f"repos/example/plugin/releases/tags/{tag}":
                    files = (*packages, paths.output / "checksums.txt", paths.output / "verification.json")
                    return make_remote_release(tag, files, draft=False)
                return None

            with (patch("release_gate.ledger", return_value=None),
                  patch("release_gate.api", side_effect=fake_api),
                  patch("release_gate.own_releases", side_effect=fake_own_releases),
                  patch("release_gate.ref_commit", return_value=self.sha),
                  patch("release_gate.subprocess.run", side_effect=fake_run) as run,
                  patch("release_gate.git_write") as write,
                  patch.dict(os.environ, {"GITHUB_REPOSITORY": "example/plugin", "GITHUB_ACTIONS": "true",
                                          "CPR_ENABLE_PUBLISH": "true"})):
                publish(paths)
                self.assertEqual(run.call_count, 7)  # empty draft, 5 sequential uploads, then promote after remote digest check
                self.assertIn("create", run.call_args_list[0].args[0])
                for call in run.call_args_list[1:6]:
                    self.assertIn("upload", call.args[0])
                    self.assertIn("--clobber", call.args[0])
                self.assertIn("--draft=false", run.call_args_list[6].args[0])
                self.assertIn("compatibility/v3.54.6.json", write.call_args_list[-1].args[0])
                receipt = read_json(paths.output / "publish-receipt.json")
                self.assertEqual(receipt["stage"], "verified")
                self.assertEqual(len(receipt["remoteAssets"]), 5)
                run.reset_mock()
                write.reset_mock()
                packages[-1].write_text("changed")
                with self.assertRaisesRegex(ValueError, "digest mismatch"):
                    publish(paths)
                run.assert_not_called()
                write.assert_not_called()

    def test_draft_remote_digest_mismatch_blocks_promotion_and_writes_failed_receipt(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            paths, packages = self.make_publish_fixture(root)
            tag = "v" + self.plan["candidateVersion"]
            state = {"created": False}

            def fake_run(cmd, **kwargs):
                if "create" in cmd:
                    state["created"] = True

            def fake_own_releases(_repo):
                if not state["created"]:
                    return []
                files = (*packages, paths.output / "checksums.txt", paths.output / "verification.json")
                draft = make_remote_release(tag, files, draft=True)
                draft["assets"][0]["digest"] = "sha256:" + ("0" * 64)
                return [draft]

            with (patch("release_gate.ledger", return_value=None),
                  patch("release_gate.api", return_value=None),
                  patch("release_gate.own_releases", side_effect=fake_own_releases),
                  patch("release_gate.subprocess.run", side_effect=fake_run) as run,
                  patch("release_gate.git_write") as write,
                  patch.dict(os.environ, {"GITHUB_REPOSITORY": "example/plugin", "GITHUB_ACTIONS": "true",
                                          "CPR_ENABLE_PUBLISH": "true", "GITHUB_RUN_ID": "999"})):
                with self.assertRaisesRegex(ValueError, "Remote asset SHA-256 mismatch"):
                    publish(paths)
                self.assertEqual(run.call_count, 6)  # draft + 5 sequential uploads, never promoted draft to public
                self.assertTrue(all("edit" not in call.args[0] for call in run.call_args_list))
                write.assert_not_called()
                receipt = read_json(paths.output / "publish-receipt.json")
                self.assertEqual(receipt["stage"], "failed")
                self.assertEqual(receipt["lastCompletedStage"], "draft-created")
                self.assertIn("resume_run_id=999", receipt["resumeHint"])

    def test_remote_verification_falls_back_to_download_when_api_digest_missing(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            sample = root / "artifact.tar.gz"
            sample.write_bytes(b"immutable-release-bytes")
            release = make_remote_release("v0.1.9", [sample], draft=True, include_digest=False)

            def fake_download(_asset, out, expected=None):
                out.parent.mkdir(parents=True, exist_ok=True)
                out.write_bytes(sample.read_bytes())
                return out

            with patch("release_gate.download_release_asset", side_effect=fake_download) as dl:
                verified = verify_remote_release(release, {sample.name: sample}, root / "dl", expect_published=False)
                dl.assert_called_once()
                self.assertEqual(verified[sample.name]["verifiedBy"], "download")
                self.assertEqual(verified[sample.name]["sha256"], file_sha(sample))

            release["assets"][0]["size"] = 999
            with self.assertRaisesRegex(ValueError, "size mismatch"):
                verify_remote_release(release, {sample.name: sample}, root / "dl2", expect_published=False)

    def test_orphan_draft_release_blocks_duplicate_creation(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            paths, packages = self.make_publish_fixture(root)
            tag = "v" + self.plan["candidateVersion"]
            orphan = {"id": 55, "tag_name": tag, "draft": True, "prerelease": False, "assets": []}
            with (patch("release_gate.ledger", return_value=None),
                  patch("release_gate.api", return_value=None),
                  patch("release_gate.own_releases", return_value=[orphan]),
                  patch("release_gate.subprocess.run") as run,
                  patch("release_gate.git_write") as write,
                  patch.dict(os.environ, {"GITHUB_REPOSITORY": "example/plugin", "GITHUB_ACTIONS": "true",
                                          "CPR_ENABLE_PUBLISH": "true"})):
                with self.assertRaisesRegex(ValueError, "Draft or duplicate release"):
                    publish(paths)
                run.assert_not_called()
                write.assert_not_called()

    def test_idempotent_resume_reuses_identical_published_release_to_finish_ledger(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            paths, packages = self.make_publish_fixture(root)
            paths.resume_run_id = "4242"
            tag = "v" + self.plan["candidateVersion"]

            def fake_own_releases(_repo):
                files = (*packages, paths.output / "checksums.txt", paths.output / "verification.json")
                return [make_remote_release(tag, files, draft=False)]

            def fake_api(route, missing=False):
                if route == f"repos/example/plugin/git/ref/tags/{tag}":
                    return {"object": {"type": "commit", "sha": self.sha}}
                return None

            with (patch("release_gate.ledger", return_value=None),
                  patch("release_gate.api", side_effect=fake_api),
                  patch("release_gate.own_releases", side_effect=fake_own_releases),
                  patch("release_gate.ref_commit", return_value=self.sha),
                  patch("release_gate.subprocess.run") as run,
                  patch("release_gate.git_write") as write,
                  patch.dict(os.environ, {"GITHUB_REPOSITORY": "example/plugin", "GITHUB_ACTIONS": "true",
                                          "CPR_ENABLE_PUBLISH": "true"})):
                publish(paths)
                run.assert_not_called()  # did not re-create or re-upload existing identical release
                self.assertIn("compatibility/v3.54.6.json", write.call_args_list[-1].args[0])
                receipt = read_json(paths.output / "publish-receipt.json")
                self.assertTrue(receipt["reusedPublishedRelease"])
                self.assertEqual(receipt["resumedFromRunId"], "4242")
                self.assertEqual(receipt["stage"], "verified")

    def test_existing_published_release_at_different_commit_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            paths, packages = self.make_publish_fixture(root)
            tag = "v" + self.plan["candidateVersion"]
            existing = make_remote_release(tag, packages, draft=False)
            with (patch("release_gate.ledger", return_value=None),
                  patch("release_gate.api", return_value={"object": {"type": "commit", "sha": "f" * 40}}),
                  patch("release_gate.own_releases", return_value=[existing]),
                  patch("release_gate.ref_commit", return_value="f" * 40),
                  patch("release_gate.subprocess.run") as run,
                  patch("release_gate.git_write") as write,
                  patch.dict(os.environ, {"GITHUB_REPOSITORY": "example/plugin", "GITHUB_ACTIONS": "true",
                                          "CPR_ENABLE_PUBLISH": "true"})):
                with self.assertRaisesRegex(ValueError, "different commit"):
                    publish(paths)
                run.assert_not_called()
                write.assert_not_called()

    def test_reuse_only_writes_compatibility_entry_not_release(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            self.plan["repository"] = "example/plugin"
            paths = argparse.Namespace(plan=root / "plan.json", decision=root / "decision.json", input=None,
                                       output=root / "out", source=self.sha, resume_run_id="", write=True)
            write_json(paths.plan, self.plan)
            write_json(paths.decision, {"route": "reuse", "results": [self.status(t, "reuse") for t in TRIPLES]})
            prior = root / "prior.json"
            write_json(prior, {"detectorRevision": "prior-verified-detector"})
            with (patch("release_gate.ledger", return_value=None), patch("release_gate.api", return_value={}),
                  patch("release_gate.release_asset", return_value=prior), patch("release_gate.git_write") as write,
                  patch("release_gate.subprocess.run") as run,
                  patch.dict(os.environ, {"GITHUB_REPOSITORY": "example/plugin", "GITHUB_ACTIONS": "true",
                                          "CPR_ENABLE_PUBLISH": "true"})):
                publish(paths)
                run.assert_not_called()
                write.assert_called_once()
                self.assertEqual((paths.output / "verification.json").read_text().count("prior-verified-detector"), 1)

    def test_decide_short_circuits_without_old_outcomes_when_applicable(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            out = root / "decision.json"
            gh_out = root / "gh.out"
            plan_path = root / "plan.json"

            # 1) No previous package: synthesizes rebuild without old/* files.
            no_prev = {**self.plan, "previous": None}
            write_json(plan_path, no_prev)
            args = argparse.Namespace(plan=plan_path, input=root / "missing-outcomes", output=out,
                                      github_output=gh_out)
            reduce(args)
            self.assertEqual(read_json(out)["route"], "rebuild")

            # 2) Contract problems short-circuit without old/* files.
            broken = {**self.plan, "contract": {**self.plan["contract"], "problems": ["Manifest format"]}}
            write_json(plan_path, broken)
            with self.assertRaisesRegex(RuntimeError, "Validation failed"):
                reduce(args)
            self.assertEqual(read_json(out)["route"], "block")

            # 3) Previous package present with missing old/* outcomes still fails closed.
            write_json(plan_path, self.plan)
            with self.assertRaisesRegex(ValueError, "Exactly one result per native platform"):
                reduce(args)

    def test_verify_resume_enforces_workflow_branch_sha_and_upstream_jobs(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            decision = root / "decision.json"
            gh_out = root / "gh.out"
            write_json(decision, {"route": "rebuild", "results": []})
            valid_run = {"path": WORKFLOW_PATH, "head_branch": "main", "head_sha": self.sha}
            valid_jobs = [
                {"name": "discover", "conclusion": "success"},
                {"name": "host-contract", "conclusion": "success"},
                {"name": "decide", "conclusion": "success"},
                *[
                    {"name": f"rebuild (runner, {triple})", "conclusion": "success"}
                    for triple in TRIPLES
                ],
            ]

            def fake_api(route, missing=False):
                if route.endswith("/jobs?per_page=100&page=1"):
                    return {"jobs": valid_jobs}
                return valid_run

            args = argparse.Namespace(repository="example/plugin", run_id="12345", source=self.sha,
                                      default_branch="main", decision=decision, output=root / "summary.json",
                                      github_output=gh_out)
            with patch("release_gate.api", side_effect=fake_api):
                verify_resume(args)
            self.assertEqual(read_json(root / "summary.json")["needsCandidates"], True)
            self.assertIn("needs_candidates=true", gh_out.read_text())

            with patch("release_gate.api", return_value={**valid_run, "head_sha": "b" * 40}):
                with self.assertRaisesRegex(ValueError, "head_sha"):
                    verify_resume(args)

            with patch("release_gate.api", return_value={**valid_run, "head_branch": "feature/x"}):
                with self.assertRaisesRegex(ValueError, "default branch"):
                    verify_resume(args)

            with patch("release_gate.api", return_value={**valid_run, "path": ".github/workflows/checks.yml"}):
                with self.assertRaisesRegex(ValueError, "belongs to"):
                    verify_resume(args)

            with patch("release_gate.api", side_effect=lambda r, missing=False:
                       {"jobs": valid_jobs[:-1]} if "/jobs?" in r else valid_run):
                with self.assertRaisesRegex(ValueError, "missing successful 'rebuild' job"):
                    verify_resume(args)

            write_json(decision, {"route": "reuse", "results": []})
            reuse_jobs = [
                {"name": "discover", "conclusion": "success"},
                {"name": "host-contract", "conclusion": "success"},
                {"name": "decide", "conclusion": "success"},
                *[{"name": f"old (runner, {triple})", "conclusion": "success"} for triple in TRIPLES],
            ]
            with patch("release_gate.api", side_effect=lambda r, missing=False:
                       {"jobs": reuse_jobs} if "/jobs?" in r else valid_run):
                verify_resume(args)
            self.assertEqual(read_json(root / "summary.json")["needsCandidates"], False)

    def test_preflight_checks_version_alignment_and_is_read_only(self):
        with tempfile.TemporaryDirectory() as scratch:
            out = pathlib.Path(scratch) / "preflight.json"
            args = argparse.Namespace(online=False, repository="", tag="", output=out, strict=False)
            dirty_git = argparse.Namespace(returncode=0, stdout=self.sha + "\n")
            dirty_status = argparse.Namespace(returncode=0, stdout=" M README.md\n")
            with patch("release_gate.subprocess.run", side_effect=[dirty_git, dirty_status]):
                preflight(args)
                report = json.loads(out.read_text())
                self.assertEqual(report["pluginVersion"], report["cargoVersion"])
                self.assertIn("dirty-working-tree", report["blockers"])
                self.assertFalse(report["publishReady"])
            clean_git = argparse.Namespace(returncode=0, stdout=self.sha + "\n")
            clean_status = argparse.Namespace(returncode=0, stdout="")
            with patch("release_gate.subprocess.run", side_effect=[clean_git, clean_status]):
                preflight(args)
                ready = json.loads(out.read_text())
                self.assertTrue(ready["publishReady"])
                self.assertEqual(ready["blockers"], [])
            discover_args = argparse.Namespace(repository="example/plugin", tag="", work=pathlib.Path(scratch),
                                               output=out, github_output=None)
            with patch("follow_host.read_json", return_value={"version": "9.9.9", "engines": {"codex-proxy-rs": "~3.54.5"}}):
                with self.assertRaisesRegex(ValueError, "Version mismatch"):
                    discovery(discover_args)

    def test_prerelease_publishes_dev_tag_without_ledger_or_stable_enable(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            self.plan["prerelease"] = True
            self.plan["previous"] = None
            self.plan["candidateVersion"] = "0.1.0-dev.1"
            paths, packages = self.make_publish_fixture(root)
            paths.allow_prerelease = True
            for triple in TRIPLES:
                cand = paths.input / triple / "candidate.json"
                row = read_json(cand)
                row["maxGlibc"] = "GLIBC_2.34" if triple.endswith("linux-gnu") else None
                write_json(cand, row)
            tag = "v0.1.0-dev.1"
            state = {"created": False}

            def fake_run(cmd, **kwargs):
                if "create" in cmd:
                    state["created"] = True

            def fake_own_releases(_repo):
                if not state["created"]:
                    return []
                files = (*packages, paths.output / "checksums.txt", paths.output / "verification.json")
                return [make_remote_release(tag, files, draft=True, prerelease=True)]

            def fake_api(route, missing=False):
                if route == f"repos/example/plugin/releases/tags/{tag}":
                    files = (*packages, paths.output / "checksums.txt", paths.output / "verification.json")
                    return make_remote_release(tag, files, draft=False, prerelease=True)
                return None

            with (patch("release_gate.api", side_effect=fake_api),
                  patch("release_gate.own_releases", side_effect=fake_own_releases),
                  patch("release_gate.ref_commit", return_value=self.sha),
                  patch("release_gate.subprocess.run", side_effect=fake_run) as run,
                  patch("release_gate.git_write") as write,
                  patch.dict(os.environ, {"GITHUB_REPOSITORY": "example/plugin", "GITHUB_ACTIONS": "true"}, clear=False)):
                os.environ.pop("CPR_ENABLE_PUBLISH", None)
                publish(paths)
                self.assertEqual(run.call_count, 7)
                self.assertIn("--prerelease", run.call_args_list[0].args[0])
                for call in run.call_args_list[1:6]:
                    self.assertIn("upload", call.args[0])
                    self.assertIn("--clobber", call.args[0])
                self.assertIn("--latest=false", run.call_args_list[6].args[0])
                write.assert_not_called()  # prerelease never mutates cpr-compatibility
                receipt = read_json(paths.output / "publish-receipt.json")
                self.assertTrue(receipt["prerelease"])
                self.assertFalse(receipt["ledgerUpdated"])
                self.assertEqual(receipt["stage"], "verified")
                # Without explicit --allow-prerelease, prerelease write must fail closed.
                paths.allow_prerelease = False
                with self.assertRaisesRegex(ValueError, "--allow-prerelease"):
                    publish(paths)

    def test_publish_retries_transient_gh_5xx_and_avoids_duplicate_draft(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            self.plan["prerelease"] = True
            self.plan["previous"] = None
            self.plan["candidateVersion"] = "0.1.0-dev.2"
            paths, packages = self.make_publish_fixture(root)
            paths.allow_prerelease = True
            tag = "v0.1.0-dev.2"
            state = {"created": False, "create_calls": 0, "uploaded": [], "upload_flaked": False,
                     "edit_flaked": False, "promoted": False}

            def fake_run(cmd, **kwargs):
                if "create" in cmd:
                    state["create_calls"] += 1
                    if state["create_calls"] == 1:
                        raise subprocess.CalledProcessError(1, cmd, stderr="HTTP 500 (https://api.github.com/repos/example/plugin/releases)")
                    # Second attempt creates draft on server before returning 502; check_done must stop retries.
                    state["created"] = True
                    raise subprocess.CalledProcessError(1, cmd, stderr="HTTP 502: Bad Gateway")
                if "upload" in cmd:
                    asset = pathlib.Path(cmd[-2])
                    if not state["upload_flaked"]:
                        state["upload_flaked"] = True
                        raise subprocess.CalledProcessError(1, cmd, stderr="HTTP 502: Error updating policy")
                    if asset not in state["uploaded"]:
                        state["uploaded"].append(asset)
                    return None
                if "edit" in cmd:
                    if not state["edit_flaked"]:
                        state["edit_flaked"] = True
                        raise subprocess.CalledProcessError(1, cmd, stderr="HTTP 503: Service Unavailable")
                    state["promoted"] = True
                    return None

            def fake_own_releases(_repo):
                if not state["created"]:
                    return []
                return [make_remote_release(tag, state["uploaded"], draft=True, prerelease=True)]

            def fake_api(route, missing=False):
                if route == f"repos/example/plugin/releases/tags/{tag}" and state["promoted"]:
                    files = (*packages, paths.output / "checksums.txt", paths.output / "verification.json")
                    return make_remote_release(tag, files, draft=False, prerelease=True)
                return None

            with (patch("release_gate.api", side_effect=fake_api),
                  patch("release_gate.own_releases", side_effect=fake_own_releases),
                  patch("release_gate.ref_commit", return_value=self.sha),
                  patch("release_gate.subprocess.run", side_effect=fake_run),
                  patch("release_gate.time.sleep") as sleep_mock,
                  patch.dict(os.environ, {"GITHUB_REPOSITORY": "example/plugin", "GITHUB_ACTIONS": "true"})):
                publish(paths)
                self.assertEqual(state["create_calls"], 2)  # stopped via check_done without 3rd create
                self.assertEqual(len(state["uploaded"]), 5)
                self.assertEqual(sleep_mock.call_count, 3)  # 1 create + 1 upload + 1 edit retry
                receipt = read_json(paths.output / "publish-receipt.json")
                self.assertEqual(receipt["stage"], "verified")

        # Non-transient 4xx fails immediately without sleeping or retrying.
        with (patch("release_gate.subprocess.run",
                    side_effect=subprocess.CalledProcessError(1, ["gh"], stderr="HTTP 422: Validation Failed")) as run,
              patch("release_gate.time.sleep") as sleep_mock):
            with self.assertRaises(subprocess.CalledProcessError):
                run_gh_retry(["gh", "release", "create"])
            self.assertEqual(run.call_count, 1)
            sleep_mock.assert_not_called()

    def test_verify_old_triggers_rebuild_when_detector_or_package_source_updates(self):
        triple = TRIPLES[0]
        artifact_name = filename(self.default_version, triple)
        verification = {
            "pluginVersion": self.default_version,
            "sourceCommit": self.sha,
            "sdkFingerprint": "ef",
            "detectorRevision": "old-det-rev",
            "packageSourceHash": "old-src-hash",
            "platforms": {triple: {"asset": artifact_name, "sha256": "b" * 64}},
        }
        plan = {**self.plan, "detectorRevision": "new-det-rev", "packageSourceHash": "old-src-hash"}
        with (patch("release_gate.release_asset", return_value=pathlib.Path("fixture.tar.gz")),
              patch("release_gate.read_json", return_value=verification),
              patch("release_gate.file_sha", return_value="b" * 64),
              patch("release_gate.package_manifest", return_value=self.author),
              patch("release_gate.smoke", return_value=True) as run):
            res = verify_old(plan, triple, pathlib.Path("dummy"))
            self.assertEqual(res["status"], "rebuild")
            self.assertEqual(res["reason"], "detector-updated")
            run.assert_not_called()

            plan["detectorRevision"] = "old-det-rev"
            plan["packageSourceHash"] = "new-src-hash"
            res2 = verify_old(plan, triple, pathlib.Path("dummy"))
            self.assertEqual(res2["status"], "rebuild")
            self.assertEqual(res2["reason"], "source-updated")
            run.assert_not_called()

    def test_publish_monotonically_updates_ledger_for_newer_stable_version_and_blocks_downgrade(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            paths, packages = self.make_publish_fixture(root)
            tag = "v" + self.plan["candidateVersion"]  # v0.1.9 superseding v0.1.8
            existing_ledger = {
                "schemaVersion": 1,
                "hostTag": self.plan["hostTag"],
                "hostCommit": self.sha,
                "pluginVersion": "0.1.8",
                "sourceCommit": "c" * 40,
                "detectorRevision": "old-det",
                "platforms": {},
            }
            state = {"created": False}

            def fake_run(cmd, **kwargs):
                if "create" in cmd:
                    state["created"] = True

            def fake_own_releases(_repo):
                if not state["created"]:
                    return []
                files = (*packages, paths.output / "checksums.txt", paths.output / "verification.json")
                return [make_remote_release(tag, files, draft=True)]

            def fake_api(route, missing=False):
                if route == f"repos/example/plugin/releases/tags/{tag}":
                    files = (*packages, paths.output / "checksums.txt", paths.output / "verification.json")
                    return make_remote_release(tag, files, draft=False)
                if route.startswith("repos/example/plugin/contents/compatibility/"):
                    return {"sha": "blobsha123"}
                if route.startswith("repos/example/plugin/git/ref/heads/"):
                    return {"object": {"sha": self.sha}}
                return None

            with (patch("release_gate.ledger", return_value=existing_ledger),
                  patch("release_gate.api", side_effect=fake_api),
                  patch("release_gate.own_releases", side_effect=fake_own_releases),
                  patch("release_gate.ref_commit", return_value=self.sha),
                  patch("release_gate.subprocess.run", side_effect=fake_run),
                  patch("release_gate.git_write") as write,
                  patch.dict(os.environ, {"GITHUB_REPOSITORY": "example/plugin", "GITHUB_ACTIONS": "true",
                                          "CPR_ENABLE_PUBLISH": "true"})):
                publish(paths)
                ledger_call = write.call_args_list[-1]
                self.assertIn("compatibility/v3.54.6.json", ledger_call.args[0])
                self.assertEqual(ledger_call.args[1]["sha"], "blobsha123")
                verification = read_json(paths.output / "verification.json")
                self.assertEqual(verification["previousVersions"][-1]["pluginVersion"], "0.1.8")
                receipt = read_json(paths.output / "publish-receipt.json")
                self.assertTrue(receipt["ledgerSuperseded"])
                self.assertTrue(receipt["ledgerUpdated"])

            # Downgrade (0.1.9 -> 0.1.8) must fail closed.
            newer_ledger = {**existing_ledger, "pluginVersion": "0.1.10"}
            with (patch("release_gate.ledger", return_value=newer_ledger),
                  patch.dict(os.environ, {"GITHUB_REPOSITORY": "example/plugin", "GITHUB_ACTIONS": "true",
                                          "CPR_ENABLE_PUBLISH": "true"})):
                with self.assertRaisesRegex(ValueError, "downgrade"):
                    publish(paths)

    def test_lm_detector_watcher_ignores_web_commits_and_detects_data_or_sdk_changes(self):
        with patch("sync_upstreams.api", return_value={
            "status": "ahead",
            "files": [{"filename": "web/src/app.tsx"}, {"filename": "README.md"}],
        }):
            self.assertEqual(lm_changed_paths("Ikaleio/lm-detector", "1" * 40, "2" * 40), [])

        with patch("sync_upstreams.api", return_value={
            "status": "ahead",
            "files": [
                {"filename": "web/src/app.tsx"},
                {"filename": "data/unified_bank.json"},
                {"filename": "sdk/rust/src/scoring.rs"},
            ],
        }):
            self.assertEqual(
                lm_changed_paths("Ikaleio/lm-detector", "1" * 40, "2" * 40),
                ["data/unified_bank.json", "sdk/rust/src/scoring.rs"],
            )

        # Fallback to tree fingerprint comparison when compare returns diverged or >=300 files.
        def fake_tree_api(route, missing=False):
            if "/compare/" in route:
                return {"status": "diverged", "files": []}
            if "contents/data?ref=old" in route:
                return [{"name": "unified_bank.json", "sha": "aaa"}, {"name": "shared_detector.bin", "sha": "bbb"}]
            if "contents/data?ref=new" in route:
                return [{"name": "unified_bank.json", "sha": "ccc"}, {"name": "shared_detector.bin", "sha": "bbb"}]
            if "contents/sdk?ref=old" in route:
                return [{"name": "rust", "sha": "rrr"}]
            if "contents/sdk?ref=new" in route:
                return [{"name": "rust", "sha": "rrr"}]
            return []

        with patch("sync_upstreams.api", side_effect=fake_tree_api):
            self.assertEqual(lm_changed_paths("Ikaleio/lm-detector", "old", "new"), ["data/unified_bank.json"])

    def test_sync_upstreams_updates_cargo_and_toolchain_pins_and_hashes_package_source(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            cargo = root / "Cargo.toml"
            toolchain = root / "rust-toolchain.toml"
            cargo.write_text(
                '[package]\nname = "demo"\nrust-version = "1.99.1"\n'
                '[dependencies]\n'
                'gateway-plugin-sdk = { git = "https://github.com/zyycn/codex-proxy-rs.git", rev = "' + ("a" * 40) + '", features = ["io"] }\n'
                'lmfpd = { git = "https://github.com/Ikaleio/lm-detector.git", rev = "' + ("b" * 40) + '" }\n'
            )
            toolchain.write_text('[toolchain]\nchannel = "1.99.1"\n')
            update_cargo_dep_rev(cargo, "lmfpd", "b" * 40, "c" * 40)
            update_rust_version(toolchain, cargo, "1.100.0")
            self.assertIn("c" * 40, cargo.read_text())
            self.assertIn("a" * 40, cargo.read_text())
            self.assertIn('rust-version = "1.100.0"', cargo.read_text())
            self.assertIn('channel = "1.100.0"', toolchain.read_text())

            msg = build_commit_message({
                "host": {"needsSync": True, "tag": "v3.55.2", "commit": "d" * 40, "currentEngines": "~3.54.5",
                         "targetEngines": "~3.55.2", "engineNeedsUpdate": True, "sdkChanged": True,
                         "rustChanged": False, "currentRust": "1.99.1", "targetRust": "1.99.1"},
                "lmDetector": {"needsSync": True, "currentRevision": "b" * 40, "latestRevision": "c" * 40,
                               "changedPaths": ["data/unified_bank.json"]},
            })
            self.assertIn("CPR v3.55.2", msg)
            self.assertIn("lm-detector", msg)
            self.assertEqual(len(package_source_hash()), 64)


if __name__ == "__main__":
    unittest.main()
