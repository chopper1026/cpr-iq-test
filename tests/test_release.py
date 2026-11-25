"""Offline release rehearsals: no GitHub write, production secret or OAuth is used."""

import argparse
import os
import pathlib
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "scripts"))

from detector_assets import file_sha, profile_stamp
from follow_host import TRIPLES, package_coverage, write_json, engine_matches
from release_gate import assemble, classify, filename, label, publish, rows_for, verify_old


class FollowRelease(unittest.TestCase):
    def setUp(self):
        self.sha = "a" * 40
        self.default_version = "0.1.8"
        self.plan = {"hostTag": "v3.54.6", "hostCommit": self.sha, "candidateVersion": "0.1.9",
                     "previous": {"version": self.default_version, "assets": {}},
                     "contract": {"problems": [], "sdkChangedFiles": [], "sdkFingerprint": "ef",
                                  "host": {"manifest_schema_versions": [2], "protocol_versions": [7],
                                           "capabilities": [{"capability":"maintenance","versions":[1]}]}}}
        self.author = {"publisher":"cpr-iq-test","name":"account-fingerprint","version":self.default_version,
                       "engines":{"codex-proxy-rs":"~3.54.5"},"manifestVersion": 2,
                       "package":{"protocolVersion":7}, "contributes":{"maintenance":{"version":1}}}

    def status(self, triple, status, **more):
        version = self.plan["candidateVersion"] if status == "verified" else self.default_version
        return label(self.plan, triple, status, pluginVersion=version, asset=filename(version, triple), sha256="b"*64,
                     validatedProfiles=["fixture-approved"], sourceCommit=self.sha, **more)

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
        rows = [self.status(t,"verified") for t in TRIPLES]
        entry, _ = assemble(self.plan, rows, "rebuild", self.sha)
        self.assertEqual(entry["pluginVersion"], self.plan["candidateVersion"])

    def test_blocked_host_or_unverified_profile_is_never_a_release(self):
        row = [self.status(t, "reuse") for t in TRIPLES]
        row[2]["status"] = "block"
        self.assertEqual(classify(self.plan, row), "block")
        with self.assertRaisesRegex(ValueError,"Blocked route"):
            assemble(self.plan, row, "block", self.sha)
        row = [self.status(t, "verified") for t in TRIPLES]
        for item in row:
            item["validatedProfiles"] = []
        with self.assertRaisesRegex(ValueError,"nonempty set"):
            assemble(self.plan, row, "rebuild", self.sha)

    def test_matrix_requires_all_platforms_from_same_host_commit(self):
        with tempfile.TemporaryDirectory() as scratch:
            paths = [pathlib.Path(scratch) / f"{idx}.json" for idx in range(3)]
            for p, t in zip(paths, TRIPLES, strict=True):
                write_json(p, self.status(t,"reuse"))
            self.assertEqual(len(rows_for(self.plan, paths)), 3)
            with self.assertRaises(ValueError):
                rows_for(self.plan, paths[:2])
            other = self.status(TRIPLES[2],"reuse")
            other["hostCommit"] = "f"*40
            write_json(paths[-1], other)
            with self.assertRaisesRegex(ValueError,"Mixed host"):
                rows_for(self.plan, paths)

    def test_engine_floor_and_minor_fence(self):
        for tag, expect in [("v3.54.4", False),("v3.54.5", True),("v3.54.12", True),("v3.55.0", False),
                            ("v3.54.6-rc.1",False),("v03.54.5",False)]:
            self.assertEqual(engine_matches("~3.54.5",tag),expect)
        self.assertFalse(engine_matches("*","v3.54.5"))
        self.author["package"]["protocolVersion"] = 6
        self.assertEqual(package_coverage(self.author,self.plan["contract"]["host"],"v3.54.5"),"rpc-contract")

    def test_old_package_integrity_and_smoke_are_required_not_claimed(self):
        triple = TRIPLES[0]
        artifact_name = filename(self.default_version, triple)
        verification = {"pluginVersion":self.default_version,"validatedProfiles":["fixture-approved"],
                        "sourceCommit":self.sha,"sdkFingerprint":"ef",
                        "platforms":{triple:{"asset":artifact_name,"sha256":"b"*64}}}
        with (patch("release_gate.release_asset", return_value=pathlib.Path("fixture.tar.gz")),
              patch("release_gate.read_json",return_value=verification),
              patch("release_gate.file_sha",return_value="b"*64),
              patch("release_gate.package_manifest",return_value=self.author),
              patch("release_gate.smoke",return_value=True) as run):
            self.assertEqual(verify_old(self.plan,triple,pathlib.Path("dummy"))["status"],"reuse")
            run.assert_called_once()
            run.return_value = False
            self.assertEqual(verify_old(self.plan,triple,pathlib.Path("dummy"))["status"],"block")
            self.plan["contract"]["sdkChangedFiles"] = ["src/manifest.rs"]
            self.assertEqual(verify_old(self.plan,triple,pathlib.Path("dummy"))["status"],"rebuild")
            verification["platforms"][triple]["sha256"] = "a"*64
            self.assertEqual(verify_old(self.plan,triple,pathlib.Path("dummy"))["reason"],"prior package integrity")

    def test_publish_rebuild_only_after_all_artifact_digests_and_evidence(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            self.plan["repository"] = "example/plugin"
            paths = argparse.Namespace(plan=root/"plan.json", decision=root/"decision.json", input=root/"matrix",
                                       output=root/"out", source=self.sha, write=True)
            write_json(paths.plan, self.plan)
            write_json(paths.decision, {"route":"rebuild","results":[]})
            for triple in TRIPLES:
                row = self.status(triple,"verified")
                folder = paths.input/triple
                folder.mkdir(parents=True)
                package = folder/row["asset"]
                package.write_text(triple)
                row["sha256"] = file_sha(package)
                write_json(folder/"candidate.json",row)
            with (patch("release_gate.ledger",return_value=None), patch("release_gate.api",return_value=None),
                  patch("release_gate.verified_profiles",return_value=["fixture-approved"]) as evidence,
                  patch("release_gate.subprocess.run") as run, patch("release_gate.git_write") as write,
                  patch.dict(os.environ,{"GITHUB_REPOSITORY":"example/plugin","GITHUB_ACTIONS":"true","CPR_ENABLE_PUBLISH":"true"})):
                publish(paths)
                self.assertEqual(run.call_count,2)  # draft with all three assets, then promote
                self.assertIn("create",run.call_args_list[0].args[0])
                self.assertIn("--draft=false",run.call_args_list[1].args[0])
                self.assertIn("compatibility/v3.54.6.json",write.call_args_list[-1].args[0])
                run.reset_mock(); write.reset_mock()
                package.write_text("changed")
                with self.assertRaisesRegex(ValueError,"digest mismatch"):
                    publish(paths)
                evidence.return_value = []
                with self.assertRaisesRegex(ValueError,"No authorized validation"):
                    publish(paths)
                run.assert_not_called()
                write.assert_not_called()

    def test_reuse_only_writes_compatibility_entry_not_release(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            self.plan["repository"] = "example/plugin"
            paths = argparse.Namespace(plan=root/"plan.json", decision=root/"decision.json", input=None,
                                       output=root/"out", source=self.sha, write=True)
            write_json(paths.plan,self.plan)
            write_json(paths.decision,{"route":"reuse","results":[self.status(t,"reuse") for t in TRIPLES]})
            prior = root/"prior.json"
            write_json(prior,{"detectorRevision":"prior-verified-detector"})
            with (patch("release_gate.ledger",return_value=None), patch("release_gate.api",return_value={}),
                  patch("release_gate.release_asset",return_value=prior), patch("release_gate.git_write") as write,
                  patch("release_gate.subprocess.run") as run,
                  patch.dict(os.environ,{"GITHUB_REPOSITORY":"example/plugin","GITHUB_ACTIONS":"true","CPR_ENABLE_PUBLISH":"true"})):
                publish(paths)
                run.assert_not_called()
                write.assert_called_once()
                self.assertEqual((paths.output/"verification.json").read_text().count("prior-verified-detector"),1)

    def test_validation_stamp_changes_on_effort_change(self):
        model = {"id":"example","model":"model","effort":"low","labelId":"reference"}
        self.assertNotEqual(profile_stamp(model),profile_stamp({**model,"effort":"high"}))


if __name__ == "__main__":
    unittest.main()
