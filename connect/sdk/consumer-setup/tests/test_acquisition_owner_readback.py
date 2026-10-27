"""Readback exercises actual owner publications, with no replay or tool imports."""
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

import test_python_acquisition as python_tests
import test_node_acquisition as node_tests
import test_compose_acquisition as compose_tests


def envelope(owner, family, request, job):
    return {"schema_version": 1, "kind": "toolkit.consumer_setup.acquisition_request",
            "job_id": job.name, "family": "selected-" + family, "workspace": str(owner.workspace),
            "staging_root": str(owner.staging), "arguments": copy.deepcopy(request),
            "host": {"profile_sha256": owner.profile_sha, "selected_profile_sha256": None if family == "compose" else "d" * 64,
                     "assets": {name: hashlib.sha256(raw).hexdigest() for name, raw in owner.frozen.items() if "/" not in name}}}


def snapshot(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob("*") if p.is_file() and not p.is_symlink()}


class OwnerReadbackTests(unittest.TestCase):
    def fixture(self, family):
        module = python_tests if family == "python" else node_tests
        case = module.AcquisitionTests() if family == "python" else module.NodeAcquisitionTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        owner = case.owner()
        result = case.execute(owner)
        record = envelope(owner, family, case.request, case.job)
        return module.M, case, owner, result, record

    def test_package_publications_read_back_after_external_tools_disappear(self):
        for family in ("python", "node"):
            with self.subTest(family=family):
                module, case, owner, result, record = self.fixture(family)
                # Original runtime, installer and profile paths are not needed.
                owner.profile_path.unlink()
                (case.pip if family == "python" else case.npm).unlink()
                destination = case.staging / case.job.name
                for name in ("package.json", "activation.json"):
                    (destination / name).write_text('{"later":"authored metadata"}')
                before = snapshot(case.root)
                with patch("subprocess.Popen", side_effect=AssertionError("process replay")), patch("builtins.exec", side_effect=AssertionError("tool import")):
                    observed = module.readback(case.io, record, case.job)
                self.assertEqual(observed, result)
                self.assertEqual(snapshot(case.root), before)

    def test_partial_missing_and_modified_publications_fail_without_writes(self):
        for family in ("python", "node"):
            for target in ("receipt", "artifact", "dependency", "source", "tool", "profile"):
                with self.subTest(family=family, target=target):
                    module, case, owner, result, record = self.fixture(family)
                    destination = case.staging / case.job.name
                    dependency = next(name for name in result["files"] if name.startswith("closure/") and name not in ("closure/requirements", "closure/package-json", "closure/package-lock"))
                    path = {"receipt": destination / "acquisition.json", "artifact": destination / ("artifact.whl" if family == "python" else "artifact.tgz"),
                            "dependency": destination / dependency, "source": case.job / ("report.json" if family == "python" else "project/package-lock.json"),
                            "tool": owner.assets / "worker.py", "profile": owner.assets / "profile.json"}[target]
                    if target == "receipt":
                        path.unlink()
                    else:
                        path.chmod(0o600)
                        path.write_bytes(b"changed")
                    before = snapshot(case.root)
                    with self.assertRaisesRegex(case.io.Error, family + "_acquisition_readback_unconfirmed"):
                        module.readback(case.io, record, case.job)
                    self.assertEqual(snapshot(case.root), before)

    def test_request_and_receipt_associations_cannot_cross_jobs_or_profiles(self):
        for family in ("python", "node"):
            for field in ("family", "job_id", "request", "profile", "schema", "files"):
                with self.subTest(family=family, field=field):
                    module, case, owner, result, record = self.fixture(family)
                    if field in ("family", "job_id"):
                        record[field] = "selected-compose" if field == "family" else "c" * 32
                    else:
                        path = case.staging / case.job.name / "acquisition.json"
                        value = json.loads(path.read_bytes())
                        if field == "request":
                            value["request"]["request_id"] = "c" * 32
                        elif field == "profile":
                            value["host_profile_sha256"] = "c" * 64
                        elif field == "schema":
                            value["schema_version"] = True
                        else:
                            value["files"]["closure/../escape"] = "c" * 64
                        path.write_bytes(module.wire(value))
                    with self.assertRaisesRegex(case.io.Error, family + "_acquisition_readback_unconfirmed"):
                        module.readback(case.io, record, case.job)

    def test_symlinks_special_files_and_unexpected_members_fail_closed(self):
        for family in ("python", "node"):
            for kind in ("extra_root", "extra_closure", "link", "fifo", "metadata_link", "closure_link"):
                with self.subTest(family=family, kind=kind):
                    module, case, owner, result, record = self.fixture(family)
                    destination = case.staging / case.job.name
                    if kind.startswith("extra_"):
                        path = destination / ("closure/extra" if kind == "extra_closure" else "extra")
                        path.write_bytes(b"untracked")
                    elif kind == "metadata_link":
                        (destination / "package.json").symlink_to(owner.assets / "profile.json")
                    elif kind == "closure_link":
                        (destination / "closure").rename(case.root / "original-closure")
                        (destination / "closure").symlink_to(case.root / "original-closure", target_is_directory=True)
                    else:
                        path = destination / ("artifact.whl" if family == "python" else "artifact.tgz")
                        path.unlink()
                        if kind == "link":
                            path.symlink_to(owner.assets / "worker.py")
                        else:
                            os.mkfifo(path)
                    with self.assertRaisesRegex(case.io.Error, family + "_acquisition_readback_unconfirmed"):
                        module.readback(case.io, record, case.job)

    def test_bounded_inventory_and_payload_sizes(self):
        for family in ("python", "node"):
            module, case, owner, result, record = self.fixture(family)
            with self.subTest(family=family), patch.object(module, "TOTAL_LIMIT", 1):
                with self.assertRaises(case.io.Error):
                    module.readback(case.io, record, case.job)
            artifact = case.staging / case.job.name / ("artifact.whl" if family == "python" else "artifact.tgz")
            with artifact.open("wb") as stream:
                stream.truncate(65 * 1024 * 1024)
            with self.assertRaises(case.io.Error):
                module.readback(case.io, record, case.job)

    def test_receipt_metadata_remains_bound_to_the_original_resolver_record(self):
        for family in ("python", "node"):
            with self.subTest(family=family):
                module, case, owner, result, record = self.fixture(family)
                path = case.staging / case.job.name / "acquisition.json"
                value = json.loads(path.read_bytes())
                if family == "python":
                    value["primary"]["primary"] = 1
                    value["artifacts"][0]["primary"] = 1
                else:
                    value["artifacts"][1]["name"] = "forged-dependency"
                path.write_bytes(module.wire(value))
                with self.assertRaises(case.io.Error):
                    module.readback(case.io, record, case.job)

    def compose_fixture(self):
        case = compose_tests.ComposeAcquisitionTests()
        case.setUp()
        self.addCleanup(case.tearDown)
        controller = case.controller()
        request = case.request()
        job = controller.acquire_compose(request)["job"]
        controller.join()
        final = controller.acquire_compose(request)["job"]
        self.assertEqual(final["status"], "succeeded", final)
        owner = controller.compose_acquisition
        directory = owner.workspace / "acquisitions" / job["id"]
        # The test worker's entry marker is not a production proposal artifact.
        (directory / "proposal/entered").unlink()
        module = sys.modules.get(owner.__class__.__module__)
        if module is None:
            module = python_tests.load("compose_owner_readback_module", python_tests.ROOT / "sdk/consumer-setup/compose_acquisition.py")
        return module, case, owner, directory, final["result"], envelope(owner, "compose", request, directory)

    def test_compose_final_proposal_readback_has_no_engine_dependency(self):
        module, case, owner, directory, result, record = self.compose_fixture()
        for name in ("runtime", "planner", "host_profile"):
            Path(owner.host[name]["path"]).unlink()
        owner.profile_path.unlink()
        proposal = directory / "proposal"
        (proposal / ".lock").touch()
        (proposal / ".docker").mkdir()
        (proposal / ".docker/config.json").write_bytes(b"{}\n")
        before = snapshot(case.root)
        with patch("subprocess.Popen", side_effect=AssertionError("engine or planner replay")):
            self.assertEqual(module.readback(owner.io, record, directory), result)
        self.assertEqual(snapshot(case.root), before)

    def test_compose_partial_modified_and_extra_proposals_fail(self):
        for change in ("missing_receipt", "changed_file", "missing_image", "extra", "symlink", "changed_selection", "cross_profile", "cross_job"):
            with self.subTest(change=change):
                module, case, owner, directory, result, record = self.compose_fixture()
                proposal = directory / "proposal"
                if change == "missing_receipt":
                    (proposal / "proposal.json").unlink()
                elif change == "changed_file":
                    (proposal / "inputs.json").write_bytes(b"changed")
                elif change == "missing_image":
                    (proposal / "image-acquisition.json").unlink()
                elif change == "extra":
                    (proposal / "extra").write_bytes(b"extra")
                elif change == "symlink":
                    (proposal / "prepared.json").unlink()
                    (proposal / "prepared.json").symlink_to(proposal / "inputs.json")
                elif change == "changed_selection":
                    (directory / "selection.json").write_bytes(b"changed")
                elif change == "cross_profile":
                    record["host"]["profile_sha256"] = "c" * 64
                else:
                    record["job_id"] = "c" * 32
                before = snapshot(case.root)
                with self.assertRaisesRegex(owner.io.Error, "compose_acquisition_readback_unconfirmed"):
                    module.readback(owner.io, record, directory)
                self.assertEqual(snapshot(case.root), before)

    def test_compose_plan_identity_and_completed_observation_are_bound(self):
        for member, field, value in (
            ("proposal.json", "plan_sha256", "c" * 64),
            ("prepared.json", "plan_sha256", "c" * 64),
            ("image-acquisition.json", "plan_sha256", "c" * 64),
            ("prepared.json", "status", "unconfirmed"),
            ("prepared.json", "plan", None),
            ("image-acquisition.json", "status", "pending"),
            ("image-acquisition.json", "host_profile_sha256", "c" * 64),
            ("image-acquisition.json", "selection_sha256", "c" * 64),
        ):
            with self.subTest(member=member, field=field):
                module, case, owner, directory, result, record = self.compose_fixture()
                proposal = directory / "proposal"
                path = proposal / member
                changed = json.loads(path.read_bytes())
                changed[field] = value
                path.write_text(json.dumps(changed))
                if member != "proposal.json":
                    receipt_path = proposal / "proposal.json"
                    receipt = json.loads(receipt_path.read_bytes())
                    receipt["files"][member] = hashlib.sha256(path.read_bytes()).hexdigest()
                    receipt_path.write_text(json.dumps(receipt))
                before = snapshot(case.root)
                with self.assertRaisesRegex(owner.io.Error, "compose_acquisition_readback_unconfirmed"):
                    module.readback(owner.io, record, directory)
                self.assertEqual(snapshot(case.root), before)

    def test_fresh_process_reads_each_family_without_executing_retained_tools(self):
        script = r'''
import importlib.util,json,pathlib,sys,subprocess,builtins
from types import SimpleNamespace
def load(name,path):
    spec=importlib.util.spec_from_file_location(name,path)
    value=importlib.util.module_from_spec(spec);spec.loader.exec_module(value);return value
base=pathlib.Path(sys.argv[1]);family=sys.argv[2]
backend=load('readback_backend',base/'backend.py')
owner=load('readback_owner',base/(family+'_acquisition.py'))
request=json.loads(sys.stdin.buffer.read())
io=SimpleNamespace(Error=backend.SetupError,read_file=backend.read_file,decode=backend.decode,
                   canonical_directory=backend.canonical_directory,keys=backend.keys)
def refuse(*args,**kwargs):raise AssertionError('unexpected execution')
subprocess.Popen=refuse;builtins.exec=refuse
result=owner.readback(io,request,pathlib.Path(request['workspace'])/'acquisitions'/request['job_id'])
print(json.dumps(result,sort_keys=True))
'''
        for family in ("python", "node", "compose"):
            with self.subTest(family=family):
                if family == "compose":
                    _, case, _, _, result, record = self.compose_fixture()
                else:
                    _, case, _, result, record = self.fixture(family)
                before = snapshot(case.root)
                completed = subprocess.run([sys.executable, "-I", "-B", "-c", script,
                                            str(python_tests.ROOT / "sdk/consumer-setup"), family],
                                           input=json.dumps(record).encode(), capture_output=True, timeout=15)
                self.assertEqual(completed.returncode, 0, completed.stderr.decode())
                self.assertEqual(json.loads(completed.stdout), result)
                self.assertEqual(snapshot(case.root), before)


if __name__ == "__main__":
    unittest.main()
