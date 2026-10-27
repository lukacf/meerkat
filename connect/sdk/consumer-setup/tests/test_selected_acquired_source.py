"""Receipt-bound source expansion with real acquisition/readback owners.

Registry I/O and selected validator/install outputs are controlled interfaces.
No selected package, registry process or network request is executed here.
"""
import copy
import json
from pathlib import Path
import shutil
import sys
import unittest
from unittest.mock import patch

import test_acquisition_recovery as acquisition
from test_selected_setup import SelectedRunner, adapter, backend


class AcquiredSourceFixture:
    def acquired(self, kind="python", extra_wheels=0, *, software=False):
        case, controller, owner = acquisition.AcquisitionRecoveryTests.package_controller(self, kind)
        for index in range(extra_wheels):
            case.add_wheel("dependency_" + str(index) + "_" + "x" * 80)
        controller._acquire(kind, owner, case.request)
        controller.join()
        self.assertEqual(controller.job["status"], "succeeded", controller.job)
        result = copy.deepcopy(controller.job["result"])
        controller.runner = SelectedRunner(controller.workspace)
        source = {"kind": "artifact", "entrypoint": {
            "executable": "outside_server/__main__.py" if kind == "python" else "index.js",
            "arguments": ["--explicit", "literal value"]}}
        activation = {"configuration": {"owner": "host", "reference": "chosen"},
                      "policy": {"owner": "host", "revision": "chosen"},
                      "components": [{"id": "server", "source": source, "runtime": kind,
                                      "readiness": {"agent_authored": True}}],
                      "unknown_for_canonical_owner": ["preserve", 7]}
        plan = {"schema_version": 1, "package": {"publisher": "fixture", "name": "chosen", "version": "1"},
                "package_content_digest": "1" * 64, "manifest_digest": "2" * 64, "strategy": "selected",
                "component": "server", "export": "service", "artifact_digest": result["primary"]["sha256"],
                "closure": {name[8:]: sha for name, sha in result["files"].items() if name.startswith("closure/")},
                "entrypoint": copy.deepcopy(source["entrypoint"]), "mcp_name": "chosen-server"}
        if kind == "python":
            plan.update(artifact_filename=result["primary"]["filename"], module="outside_server.__main__")
        if software:
            # This fixture controls the metadata owner; no host facts are invented.
            activation = {"document_kind": "local_software_declaration",
                          "components": [{"id": "server", "runtime": kind, "source": source}]}
            plan["document_kind"] = "local_software_declaration"
            plan["declaration_digest"] = plan.pop("manifest_digest")
        request = {"profile": kind, "staged_id": controller.job["id"], "component": "server",
                   "package_json": backend.encoded({"fixture_plan": plan}).decode(),
                   ("declaration_json" if software else "activation_json"): backend.encoded(activation).decode(),
                   "acquired_source": {"job_id": controller.job["id"], "receipt_sha256": result["receipt_sha256"]}}
        return case, controller, request, result

    def recover_selected(self, case, original, job):
        profile = case.root / "selected.json"
        result = backend.Controller(None, None, case.root / "recovered-selected", (), sys.executable,
            str(case.node) if hasattr(case, "node") else None, sys.executable if hasattr(case, "node") else None,
            runner_factory=SelectedRunner, selected_install_profile=profile,
            selected_install_profile_sha256=backend.digest(profile.read_bytes()),
            recover_workspace=original.workspace, recover_request=job["id"],
            recover_request_sha256=job["recovery_request"]["sha256"])
        self.addCleanup(result.join)
        return result

    def assert_refused(self, controller, request):
        calls = len(controller.runner.calls)
        with self.assertRaises(backend.SetupError):
            controller.selected_check(request)
        self.assertEqual(controller.validations, {})
        self.assertEqual(len(controller.runner.calls), calls)
        self.assertFalse(controller.operation_lock.locked())


class AcquiredSourceTests(AcquiredSourceFixture, unittest.TestCase):
    def test_acquired_software_declaration_keeps_exact_source_through_install_and_recovery(self):
        for kind in ("python", "node"):
            with self.subTest(kind=kind):
                case, c, request, acquired = self.acquired(kind, software=True)
                before = acquisition.inventory(case.staging)
                checked = c.selected_check(request)
                path, effect, plan = backend.selected_proposal(c.workspace, checked["validation_id"],
                    checked["effect_sha256"], c.selected_binding)
                document = backend.decode((path / "declaration.json").read_bytes())
                expected = backend.decode(request["declaration_json"].encode())
                expected["components"][0]["source"].update(location=acquired["primary"]["url"],
                    content_digest=acquired["primary"]["sha256"],
                    dependencies={"kind": "pinned_closure", "artifacts": plan["closure"]})
                self.assertEqual(document, expected)
                self.assertEqual(effect["request"]["document_kind"], "local_software_declaration")
                self.assertEqual(effect["request"]["selection_sha256"], backend.digest((path / "declaration.json").read_bytes()))
                self.assertEqual(acquisition.inventory(case.staging), before)
                self.assertFalse((path / "activation.json").exists())
                job = c.selected_install({key: checked[key] for key in ("validation_id", "effect_sha256")})["job"]
                c.join()
                self.assertEqual(c.jobs[job["id"]]["status"], "succeeded", c.jobs[job["id"]])
                shutil.rmtree(case.staging)
                recovered = self.recover_selected(case, c, job)
                result = recovered.reconcile({"job_id": job["id"]})["observation"]
                self.assertEqual(result["status"], "matched")
                self.assertEqual(result["result"]["acquired_source"], request["acquired_source"])
                self.assertEqual(result["result"]["document_kind"], "local_software_declaration")
                self.assertNotIn("manifest_digest", result["result"])
                self.assertTrue(all("--verify-existing" in argv for argv, _ in recovered.runner.calls))
                with self.assertRaises(backend.SetupError):
                    recovered.selected_check(request)

    def test_real_receipts_expand_both_profiles_without_semantic_or_staging_changes(self):
        for kind in ("python", "node"):
            with self.subTest(kind=kind):
                case, c, request, acquired = self.acquired(kind)
                before = acquisition.inventory(case.staging)
                job_before = copy.deepcopy(c.job)
                module = c.acquisition_modules[kind]
                with patch.object(module, "readback", wraps=module.readback) as reader:
                    checked = c.selected_check(request)
                    self.assertEqual(reader.call_count, 2)
                path, effect, plan = backend.selected_proposal(c.workspace, checked["validation_id"],
                    checked["effect_sha256"], c.selected_binding)
                expanded = backend.decode((path / "activation.json").read_bytes())
                expected = backend.decode(request["activation_json"].encode())
                expected["components"][0]["source"].update(location=acquired["primary"]["url"],
                    content_digest=acquired["primary"]["sha256"],
                    dependencies={"kind": "pinned_closure", "artifacts": plan["closure"]})
                self.assertEqual(expanded, expected)
                self.assertEqual((path / "package.json").read_bytes(), request["package_json"].encode())
                self.assertEqual(effect["request"]["acquired_source"], request["acquired_source"])
                self.assertEqual(checked["acquired_source"], request["acquired_source"])
                self.assertEqual(checked["selection_sha256"], backend.digest((path / "activation.json").read_bytes()))
                self.assertEqual(c.job, job_before)
                self.assertEqual(acquisition.inventory(case.staging), before)
                self.assertFalse(Path(checked["destination"]).exists())
                c.job = None  # The reference targets the retained job, not latest-job projection.
                self.assertEqual(c.selected_check(copy.deepcopy(request)), checked)
                self.assertEqual(len(c.runner.calls), 1)

    def test_large_real_closure_exceeds_inline_bound_only_after_materialization(self):
        _, c, request, _ = self.acquired(extra_wheels=220)
        # The controlled plan is fixture data, not agent-supplied package meaning.
        # Keep it outside the inline request for this body-size regression.
        plan = json.loads(request["package_json"])["fixture_plan"]
        request["package_json"] = "{}"
        with patch.object(c.runner, "run", return_value=(0, backend.encoded(plan))):
            checked = c.selected_check(request)
        raw = (c.workspace / "selections" / checked["validation_id"] / "activation.json").read_bytes()
        self.assertLess(len(backend.encoded(request)), 4096)
        self.assertGreater(len(raw), backend.SELECTED_INLINE_DOCUMENT)
        self.assertLess(len(raw), backend.SELECTED_DOCUMENT)
        self.assertEqual(len(json.loads(raw)["components"][0]["source"]["dependencies"]["artifacts"]), 222)

    def test_unknown_wrong_receipt_family_and_unsuccessful_jobs_refuse(self):
        _, c, request, _ = self.acquired()
        for changes in ({"staged_id": "0" * 32}, {"profile": "node"},
                        {"acquired_source": {"job_id": "0" * 32, "receipt_sha256": "0" * 64}},
                        {"acquired_source": {**request["acquired_source"], "receipt_sha256": "0" * 64}}):
            self.assert_refused(c, {**request, **changes})
        job = c.jobs[request["staged_id"]]
        for status in ("running", "unconfirmed", "cancelled"):
            job["status"] = status
            self.assert_refused(c, request)
        job["status"] = "succeeded"
        job["recovered"] = True
        self.assert_refused(c, request)
        job.pop("recovered")
        _, other, _, _ = self.acquired()
        self.assert_refused(other, request)

    def test_reference_is_inline_only_strict_and_not_mcpb_or_recovery(self):
        _, c, request, _ = self.acquired()
        bad_requests = [{k: v for k, v in request.items() if not k.endswith("_json")},
                        {**request, "package_sha256": "0" * 64},
                        {**request, "profile": "mcpb", "configuration_json": "{}"}]
        for ref in (None, [], {}, {"job_id": True, "receipt_sha256": "0" * 64},
                    {**request["acquired_source"], "path": "/private"},
                    {**request["acquired_source"], "receipt_sha256": "x" * 64}):
            bad_requests.append({**request, "acquired_source": ref})
        for bad in bad_requests:
            self.assert_refused(c, bad)
        c.recovery_only = True
        self.assert_refused(c, request)

    def test_mixed_source_missing_or_duplicate_component_and_duplicate_json_refuse(self):
        _, c, request, _ = self.acquired()
        activation = json.loads(request["activation_json"])
        variants = []
        for key, value in (("location", "https://example.invalid/pkg"), ("content_digest", "0" * 64),
                           ("dependencies", {"kind": "self_contained"}), ("unexpected", True),
                           ("kind", "remote_service")):
            changed = copy.deepcopy(activation)
            changed["components"][0]["source"][key] = value
            variants.append(changed)
        changed = copy.deepcopy(activation)
        del changed["components"][0]["source"]["entrypoint"]
        variants.extend((changed, {**activation, "components": []},
                         {**activation, "components": activation["components"] * 2},
                         {**activation, "components": [{**activation["components"][0], "id": "other"}]},
                         {**activation, "components": "server"}))
        for changed in variants:
            self.assert_refused(c, {**request, "activation_json": json.dumps(changed)})
        self.assert_refused(c, {**request, "activation_json": '{"components":[],"components":[]}'})
        self.assert_refused(c, {**request, "activation_json": "x" * (backend.SELECTED_INLINE_DOCUMENT + 1)})

    def test_changed_retained_and_host_inputs_refuse_before_validator(self):
        for target in ("artifact", "closure", "receipt", "request", "selected_profile", "acquisition_profile", "tool"):
            with self.subTest(target=target):
                case, c, request, result = self.acquired()
                stage = case.staging / request["staged_id"]
                paths = {"artifact": stage / "artifact.whl", "closure": stage / next(
                    name for name in result["files"] if name.startswith("closure/")),
                    "receipt": stage / "acquisition.json",
                    "request": c.workspace / "requests" / (request["staged_id"] + ".json"),
                    "selected_profile": c.workspace / "selected-host-profile.json",
                    "acquisition_profile": case.profile_path,
                    "tool": c.workspace / "selected-tools/python"}
                path = paths[target]
                path.write_bytes(path.read_bytes() + b"\n")
                self.assert_refused(c, request)

    def test_mutation_during_canonical_validation_never_publishes_proposal(self):
        for target in ("artifact", "receipt", "request", "selected_profile", "acquisition_profile", "tool"):
            with self.subTest(target=target):
                case, c, request, _ = self.acquired("node")
                stage = case.staging / request["staged_id"]
                path = {"artifact": stage / "artifact.tgz", "receipt": stage / "acquisition.json",
                        "request": c.workspace / "acquisitions" / request["staged_id"] / "request.json",
                        "selected_profile": c.workspace / "selected-host-profile.json",
                        "acquisition_profile": case.profile_path,
                        "tool": c.workspace / "selected-tools/node"}[target]
                original = c.runner.run
                def changed(*args, **kwargs):
                    result = original(*args, **kwargs)
                    path.write_bytes(path.read_bytes() + b"\n")
                    return result
                with patch.object(c.runner, "run", side_effect=changed):
                    with self.assertRaises(backend.SetupError):
                        c.selected_check(request)
                self.assertEqual(c.validations, {})
                self.assertEqual(list((c.workspace / "selections").glob("*/effect.json")), [])
                self.assertFalse(c.operation_lock.locked())

    def test_validator_refusal_remains_authoritative(self):
        _, c, request, _ = self.acquired()
        with patch.object(c.runner, "run", return_value=(2, b"private rejection")):
            with self.assertRaises(backend.SetupError) as caught:
                c.selected_check(request)
        self.assertNotIn("private", str(caught.exception))
        self.assertEqual(c.validations, {})

    def test_install_retry_and_recovery_use_frozen_selection_without_old_acquisition(self):
        for kind, software in (("python", False), ("node", False), ("python", True), ("node", True)):
            with self.subTest(kind=kind, software=software):
                case, c, request, _ = self.acquired(kind, software=software)
                checked = c.selected_check(request)
                shutil.rmtree(case.staging)
                shutil.rmtree(c.workspace / "acquisitions")
                c.runner.nonzero = True
                job = c.selected_install({key: checked[key] for key in ("validation_id", "effect_sha256")})["job"]
                c.join()
                self.assertEqual(c.jobs[job["id"]]["status"], "unconfirmed")
                fresh = c.selected_retry({"job_id": job["id"]})
                self.assertEqual(fresh["acquired_source"], request["acquired_source"])
                self.assertEqual(fresh["selection_sha256"], checked["selection_sha256"])
                if software:
                    self.assertEqual(fresh["document_kind"], "local_software_declaration")
                    self.assertEqual(fresh["declaration_digest"], checked["declaration_digest"])
                    self.assertNotIn("manifest_digest", fresh)
                    self.assertEqual((c.workspace / "selections" / fresh["validation_id"] / "declaration.json").read_bytes(),
                                     (c.workspace / "selections" / checked["validation_id"] / "declaration.json").read_bytes())
                self.assertEqual(fresh, c.selected_retry({"job_id": job["id"]}))
                c.runner.nonzero = False
                job = c.selected_install({key: fresh[key] for key in ("validation_id", "effect_sha256")})["job"]
                c.join()
                self.assertEqual(c.jobs[job["id"]]["status"], "succeeded", c.jobs[job["id"]])
                self.assertEqual(c.jobs[job["id"]]["result"]["acquired_source"], request["acquired_source"])
                recovered = self.recover_selected(case, c, c.jobs[job["id"]])
                observation = recovered.reconcile({"job_id": job["id"]})["observation"]
                self.assertEqual(observation["status"], "matched", observation)
                self.assertEqual(observation["result"]["acquired_source"], request["acquired_source"])
                with self.assertRaises(backend.SetupError):
                    recovered.selected_check(request)

    def test_exact_reference_is_effect_bound(self):
        _, c, request, _ = self.acquired()
        checked = c.selected_check(request)
        path = c.workspace / "selections" / checked["validation_id"] / "effect.json"
        value = json.loads(path.read_bytes())
        value["request"]["acquired_source"]["receipt_sha256"] = "0" * 64
        path.write_bytes(backend.encoded(value))
        with self.assertRaisesRegex(backend.SetupError, "selected_proposal_changed"):
            c.selected_install({key: checked[key] for key in ("validation_id", "effect_sha256")})


class AcquiredSourceMcpTests(AcquiredSourceFixture, unittest.IsolatedAsyncioTestCase):
    async def test_real_mcp_schema_accepts_inline_reference_and_refuses_hash_or_mcpb(self):
        from mcp import Client
        _, c, request, _ = self.acquired("node")
        owner = adapter.SetupAdapter(c)
        try:
            async with Client(adapter.create_server(owner)) as client:
                tools = (await client.list_tools()).tools
                schema = next(item for item in tools if item.name == "setup_selected_check").input_schema
                self.assertIn("acquired_source", schema["properties"])
                accepted = await client.call_tool("setup_selected_check", request)
                self.assertFalse(accepted.is_error, accepted)
                self.assertEqual(accepted.structured_content["acquired_source"], request["acquired_source"])
                for bad in ({**request, "package_sha256": "0" * 64},
                            {**request, "profile": "mcpb", "configuration_json": "{}"},
                            {**request, "acquired_source": {**request["acquired_source"], "extra": True}}):
                    result = await client.call_tool("setup_selected_check", bad)
                    self.assertTrue(result.is_error, result)
        finally:
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
