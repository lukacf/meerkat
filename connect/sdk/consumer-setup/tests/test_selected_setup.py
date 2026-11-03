"""Selected setup composition with inert installer/validator owners and the real MCP SDK.

These tests establish custody and projection, not package validity or runtime readiness.
"""
import asyncio
import copy
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("selected_setup_adapter", ROOT / "mcp_adapter.py")
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)
backend = adapter.backend


class SelectedRunner:
    """Emulate only canonical owner outputs; never execute package bytes."""
    def __init__(self, workspace):
        self.workspace = workspace
        self.calls = []
        self.block = None
        self.entered = threading.Event()
        self.nonzero = False
        self.bad_proof = None
        self.receipt_padding = 0

    def run(self, argv, capture=False):
        self.calls.append((list(argv), capture))
        def arg(name):
            return argv[argv.index(name) + 1]
        if "--package" in argv:
            return 0, backend.encoded(backend.decode(Path(arg("--package")).read_bytes())["fixture_plan"])
        profile = "python" if "--selected-python" in argv else "node"
        source = Path(arg("--selected-" + profile))
        effect = backend.decode((source / "effect.json").read_bytes())
        plan = backend.decode((source / "plan.json").read_bytes())
        destination = Path(arg("--destination"))
        command = ([str(destination / "runtime/bin/python"), "-I", "-B", "-m", plan["module"]]
                   if profile == "python" else [arg("--node"), str(destination / "node_modules/fixture/index.js")])
        command += plan["entrypoint"]["arguments"]
        mcp = {"mcpServers": {plan["mcp_name"]: {"command": command[0], "args": command[1:]}}}
        if "--verify-existing" not in argv:
            self.entered.set()
            if self.block:
                self.block.wait(10)
            destination.mkdir()
            (destination / "ready").mkdir()
            receipt = {"schema_version": 1, "scope": "observed_software_installation_only",
                       "inputs": effect["files"], "command": command, "destination": str(destination),
                       "mcp_configuration": mcp, "selection_sha256": arg("--selection-sha256"),
                       **({"document_kind": "local_software_declaration", "declaration_digest": plan["declaration_digest"]}
                          if "document_kind" in plan else {"manifest_digest": plan["manifest_digest"]}), "padding": "x" * self.receipt_padding}
            (destination / "ready/installed.json").write_bytes(backend.encoded(receipt))
            (destination / "ready/mcp.json").write_bytes(backend.encoded(mcp))
            return (2 if self.nonzero else 0), b"PRIVATE_CHILD_OUTPUT"
        receipt_raw = (destination / "ready/installed.json").read_bytes()
        receipt = backend.decode(receipt_raw)
        if receipt["command"] != command or backend.decode((destination / "ready/mcp.json").read_bytes()) != mcp:
            return 2, b"PRIVATE_CHILD_OUTPUT"
        proof = {"schema_version": 1, "scope": backend.READBACK_SCOPE, "verified": True,
                 "selection_kind": profile, "destination": str(destination),
                 "selection_sha256": arg("--selection-sha256"), "component": arg("--component"),
                 **({"document_kind": "local_software_declaration", "declaration_digest": plan["declaration_digest"]}
                          if "document_kind" in plan else {"manifest_digest": plan["manifest_digest"]}), "command": command,
                 "selected_runtime_path": arg("--" + profile), "retained_payloads": {"fixture": True},
                 "historical_receipt": {"sha256": backend.digest(receipt_raw), "recorded_scope": receipt["scope"]}}
        if self.bad_proof:
            proof.update(self.bad_proof)
        return 0, backend.encoded(proof)


def software_form(staging, request):
    """Controlled metadata-owner output; actual declaration validity is Rust-owned."""
    request = copy.deepcopy(request)
    source = staging / request["staged_id"]
    package = backend.decode((source / "package.json").read_bytes())
    plan = package["fixture_plan"]
    plan["document_kind"] = "local_software_declaration"
    plan["declaration_digest"] = plan.pop("manifest_digest")
    (source / "package.json").write_bytes(backend.encoded(package))
    (source / "activation.json").unlink()
    (source / "declaration.json").write_bytes(backend.encoded({
        "document_kind": "local_software_declaration", "fixture": "canonical semantics tested in Rust"}))
    request.update(document_kind="local_software_declaration",
        package_sha256=backend.digest((source / "package.json").read_bytes()),
        selection_sha256=backend.digest((source / "declaration.json").read_bytes()))
    return request


class SelectedFixture:
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.staging = self.root / "staging"
        self.staging.mkdir()
        self.controllers = []
        self.sequence = 0
        self.tools = {}
        for name in ("installer", "python", "node"):
            path = self.root / (name + ".tool")
            path.write_bytes(("inert host " + name + "\n").encode())
            path.chmod(0o700)
            self.tools[name] = {"path": str(path), "sha256": backend.digest(path.read_bytes())}
        self.profile = {"schema_version": 1, "scope": backend.SELECTED_SCOPE, "staging_root": str(self.staging),
                        "installer": self.tools["installer"], "validators": {k: self.tools[k] for k in ("python", "node")}}
        self.profile_path = self.root / "host.json"
        self.write_profile()

    def write_profile(self):
        self.profile_path.write_bytes(backend.encoded(self.profile))
        self.profile_sha = backend.digest(self.profile_path.read_bytes())

    def tearDown(self):
        for controller in self.controllers:
            if controller.runner.block:
                controller.runner.block.set()
            controller.join()
        self.temp.cleanup()

    def controller(self, **changes):
        self.sequence += 1
        args = dict(bundle=None, contents_sha256=None, families=(), workspace=self.root / ("work-" + str(self.sequence)),
                    python=sys.executable, node=sys.executable, npm_cli=sys.executable, runner_factory=SelectedRunner,
                    selected_install_profile=self.profile_path, selected_install_profile_sha256=self.profile_sha)
        args.update(changes)
        result = backend.Controller(**args)
        self.controllers.append(result)
        return result

    def stage(self, profile="python", name="choice", mutate=None):
        source = self.staging / name
        source.mkdir()
        (source / "closure").mkdir()
        artifact = b"inert archive bytes"
        closure = b"inert closure bytes"
        plan = {"schema_version": 1, "package": {"publisher": "fixture", "name": "chosen", "version": "1"},
                "package_content_digest": "1" * 64, "manifest_digest": "2" * 64, "strategy": "selected",
                "component": "server", "export": "service", "artifact_digest": backend.digest(artifact),
                "closure": {"dependency": backend.digest(closure)},
                "entrypoint": {"executable": "python" if profile == "python" else "node", "arguments": ["--mode", "safe"]},
                "mcp_name": "chosen-server"}
        if profile == "python":
            plan.update(artifact_filename="fixture-1-py3-none-any.whl", module="fixture_server")
        if mutate:
            mutate(plan)
        (source / "package.json").write_bytes(backend.encoded({"fixture_plan": plan}))
        (source / "activation.json").write_bytes(backend.encoded({"fixture": "host admitted software installation only"}))
        (source / ("artifact.whl" if profile == "python" else "artifact.tgz")).write_bytes(artifact)
        (source / "closure/dependency").write_bytes(closure)
        return {"profile": profile, "staged_id": name, "component": "server",
                "package_sha256": backend.digest((source / "package.json").read_bytes()),
                "selection_sha256": backend.digest((source / "activation.json").read_bytes())}

    def software_stage(self, profile="python", name="choice", mutate=None):
        return software_form(self.staging, self.stage(profile, name, mutate))

    def selection(self, checked):
        return {k: checked[k] for k in ("validation_id", "effect_sha256")}

    def installed(self, profile="python"):
        controller = self.controller()
        checked = controller.selected_check(self.stage(profile))
        selected = self.selection(checked)
        job = controller.selected_install(selected)["job"]
        controller.join()
        self.assertEqual(controller.jobs[job["id"]]["status"], "succeeded", controller.jobs[job["id"]])
        return controller, checked, job

    def recover(self, original, job):
        return self.controller(recover_workspace=original.workspace, recover_request=job["id"],
                               recover_request_sha256=job["recovery_request"]["sha256"])


class SelectedTests(SelectedFixture, unittest.TestCase):
    def test_python_console_plan_retains_selector_without_installation(self):
        c = self.controller()
        for software in (False, True):
            with self.subTest(software=software):
                stage = self.software_stage if software else self.stage
                request = stage(name="console-" + str(software), mutate=lambda plan:
                    plan["entrypoint"].update(python_console_script="fixture-server"))
                checked = c.selected_check(request)
                _, _, plan = c.selected_command({"root": c.workspace, **self.selection(checked)}, verify=False)
                self.assertEqual(plan["entrypoint"]["python_console_script"], "fixture-server")
                self.assertEqual(c.selected_check(request), checked)
                self.assertIsNone(c.job)
        self.assertTrue(all("--package" in argv for argv, _ in c.runner.calls))

    def test_console_plan_selector_malformed_or_non_python_refuses(self):
        c = self.controller()
        for index, (profile, script) in enumerate([
                ("python", None), ("python", 1), ("python", ""), ("python", "../server"),
                ("python", "server\n"), ("python", "serv\u00e9r"), ("node", "fixture-server")]):
            with self.subTest(profile=profile, index=index), \
                    self.assertRaisesRegex(backend.SetupError, "selected_validation_unavailable"):
                c.selected_check(self.stage(profile, "bad-console-" + str(index), mutate=lambda plan:
                    plan["entrypoint"].update(python_console_script=script)))
        self.assertIsNone(c.job)

    def test_software_forms_install_reconcile_and_recover_exact_identity_without_replay(self):
        for profile in ("python", "node"):
            with self.subTest(profile=profile):
                c = self.controller()
                request = self.software_stage(profile, "software-" + profile,
                    mutate=lambda plan: plan.update(declared_service={"authentication": {"kind": "none"},
                        "operation_tools": {"declared/read": "actual_read"}}))
                checked = c.selected_check(request)
                self.assertEqual(checked["document_kind"], "local_software_declaration")
                self.assertEqual(checked["declaration_digest"], "2" * 64)
                self.assertNotIn("manifest_digest", checked)
                self.assertIn("declaration.json", checked["files"])
                self.assertNotIn("activation.json", checked["files"])
                self.assertIn("--declaration", c.runner.calls[0][0])
                self.assertNotIn("--selection", c.runner.calls[0][0])
                shutil.rmtree(self.staging / request["staged_id"])
                job = c.selected_install(self.selection(checked))["job"]
                c.join()
                self.assertEqual(c.jobs[job["id"]]["status"], "succeeded", c.jobs[job["id"]])
                self.assertEqual(c.selected_install(self.selection(checked))["job"]["id"], job["id"])
                value = c.reconcile({"job_id": job["id"]})["observation"]
                self.assertEqual(value["status"], "matched")
                self.assertEqual(value["result"]["selection_kind"], profile)
                self.assertEqual(value["result"]["document_kind"], request["document_kind"])
                self.assertEqual(value["result"]["declaration_digest"], checked["declaration_digest"])
                self.assertNotIn("manifest_digest", value["result"])
                install_calls = [argv for argv, _ in c.runner.calls if "--selected-" + profile in argv]
                self.assertTrue(all(argv[argv.index("--document-kind") + 1] == request["document_kind"] for argv in install_calls))
                self.assertEqual(sum("--verify-existing" not in argv for argv in install_calls), 1)
                recovered = self.recover(c, job)
                self.assertTrue(recovered.recovery_only)
                self.assertEqual(recovered.reconcile({"job_id": job["id"]})["observation"]["status"], "matched")
                with self.assertRaises(backend.SetupError):
                    recovered.selected_retry({"job_id": job["id"]})
                self.assertTrue(all("--verify-existing" in argv for argv, _ in recovered.runner.calls))
                recovered.runner.bad_proof = {"manifest_digest": checked["declaration_digest"]}
                self.assertNotEqual(recovered.reconcile({"job_id": job["id"]})["observation"]["status"], "matched")

    def test_selected_document_kind_and_canonical_identity_cannot_cross_forms(self):
        for profile in ("python", "node"):
            request = self.software_stage(profile, "invalid-" + profile)
            c = self.controller()
            for kind in (None, "activation", "remote_declaration"):
                changed = dict(request)
                if kind is None:
                    changed.pop("document_kind")
                else:
                    changed["document_kind"] = kind
                with self.subTest(profile=profile, kind=kind), self.assertRaises(backend.SetupError):
                    c.selected_check(changed)
            self.assertEqual(c.runner.calls, [])
            package_path = self.staging / request["staged_id"] / "package.json"
            original = backend.decode(package_path.read_bytes())
            for change in ({"manifest_digest": "2" * 64}, {"document_kind": "remote_declaration"},
                           {"declaration_digest": None}):
                package = copy.deepcopy(original)
                package["fixture_plan"].update(change)
                package_path.write_bytes(backend.encoded(package))
                with self.subTest(profile=profile, change=change), self.assertRaises(backend.SetupError):
                    c.selected_check({**request, "package_sha256": backend.digest(package_path.read_bytes())})
            self.assertFalse(any("--selected-" + profile in argv for argv, _ in c.runner.calls))
            self.assertEqual(c.jobs, {})

    def test_declared_service_survives_exact_selected_install_and_reconciliation(self):
        declaration = {"authentication": {"kind": "none"},
                       "operation_tools": {"declared/read": "actual_read_items"}}
        for profile in ("python", "node"):
            with self.subTest(profile=profile):
                c = self.controller()
                checked = c.selected_check(self.stage(profile, profile,
                    mutate=lambda plan: plan.update(declared_service=copy.deepcopy(declaration))))
                self.assertEqual(checked["declared_service"], declaration)
                checked["declared_service"]["authentication"]["kind"] = "mutated-client-copy"
                # The controller retains the canonical plan, not the returned object.
                job = c.selected_install(self.selection(checked))["job"]
                c.join()
                self.assertEqual(c.jobs[job["id"]]["status"], "succeeded")
                self.assertEqual(c.jobs[job["id"]]["result"]["declared_service"], declaration)
                read = c.reconcile({"job_id": job["id"]})["observation"]
                self.assertEqual(read["status"], "matched")
                self.assertEqual(read["result"]["declared_service"], declaration)
                read["result"]["declared_service"]["operation_tools"]["declared/read"] = "changed"
                next_read = c.reconcile({"job_id": job["id"]})["observation"]
                self.assertEqual(next_read["result"]["declared_service"], declaration)

    def test_secret_slots_survive_selected_custody_without_changing_launch(self):
        declaration = {"authentication": {"kind": "binding", "owner": "process",
            "requirement": "api-key", "credential_audience": "service"},
            "operation_tools": {"read": "actual_read"}, "environment_secret_slots": ["API_KEY", "TOKEN"]}
        for profile in ("python", "node"):
            for software in (False, True):
                with self.subTest(profile=profile, software=software):
                    c = self.controller()
                    stage = self.software_stage if software else self.stage
                    checked = c.selected_check(stage(profile, "secret-" + profile + str(software),
                        mutate=lambda plan: plan.update(declared_service=copy.deepcopy(declaration))))
                    checked["declared_service"]["environment_secret_slots"][0] = "CHANGED_CLIENT_COPY"
                    job = c.selected_install(self.selection(checked))["job"]; c.join()
                    self.assertEqual(c.jobs[job["id"]]["status"], "succeeded")
                    read = c.reconcile({"job_id": job["id"]})["observation"]["result"]
                    self.assertEqual(read["declared_service"], declaration)
                    launch = next(iter(read["mcp_configuration"]["mcpServers"].values()))
                    self.assertEqual(set(launch), {"command", "args"})
                    read["declared_service"]["environment_secret_slots"].append("OTHER")
                    self.assertEqual(c.reconcile({"job_id": job["id"]})["observation"]["result"]["declared_service"], declaration)
                    recovered = self.recover(c, job)
                    self.assertEqual(recovered.reconcile({"job_id": job["id"]})["observation"]["result"]["declared_service"], declaration)
                    self.assertTrue(all("--verify-existing" in argv for argv, _ in recovered.runner.calls))

    def test_changed_secret_slot_plan_cannot_reuse_the_install_decision(self):
        declaration = {"authentication": {"kind": "binding", "owner": "process", "requirement": "key", "credential_audience": "service"},
                       "operation_tools": {"read": "actual_read"}, "environment_secret_slots": ["TOKEN"]}
        for profile in ("python", "node"):
            for software in (False, True):
                c = self.controller()
                stage = self.software_stage if software else self.stage
                checked = c.selected_check(stage(profile, "changed-secret-" + profile + str(software),
                    mutate=lambda plan: plan.update(declared_service=copy.deepcopy(declaration))))
                plan_path = c.workspace / "selections" / checked["validation_id"] / "plan.json"
                plan = backend.decode(plan_path.read_bytes())
                plan["declared_service"]["environment_secret_slots"] = ["DIFFERENT_TOKEN"]
                plan_path.write_bytes(backend.encoded(plan))
                with self.assertRaises(backend.SetupError):
                    c.selected_install(self.selection(checked))
                self.assertFalse(any("--selected-" + profile in argv for argv, _ in c.runner.calls))
                self.assertIsNone(c.job)

    def test_secret_slots_reject_invalid_canonical_projection_and_no_auth(self):
        good = {"authentication": {"kind": "binding", "owner": "process", "requirement": "key", "credential_audience": "service"},
                "operation_tools": {"read": "actual_read"}, "environment_secret_slots": ["API_KEY", "TOKEN"]}
        variants = [dict(good, environment_secret_slots=value) for value in
                    (None, [], ["TOKEN", "TOKEN"], ["TOKEN", "API_KEY"], ["bad-name"], [7], ["x" * 129])]
        variants.append(dict(good, authentication={"kind": "none"}))
        for profile in ("python", "node"):
            c = self.controller()
            for index, declaration in enumerate(variants):
                with self.subTest(profile=profile, index=index), self.assertRaises(backend.SetupError):
                    c.selected_check(self.stage(profile, "secret-invalid-" + profile + "-" + str(index),
                        mutate=lambda plan: plan.update(declared_service=copy.deepcopy(declaration))))
            self.assertIsNone(c.job)

    def test_absent_and_binding_declarations_do_not_block_selected_software(self):
        declaration = {"authentication": {"kind": "binding", "owner": "process",
            "requirement": "api-key", "credential_audience": "service"},
            "operation_tools": {"read": "actual_read"}}
        for index, value in enumerate((None, declaration)):
            c = self.controller()
            def mutate(plan):
                if value is not None:
                    plan["declared_service"] = copy.deepcopy(value)
            checked = c.selected_check(self.stage(name="auth-" + str(index), mutate=mutate))
            self.assertEqual(checked.get("declared_service"), value)
            job = c.selected_install(self.selection(checked))["job"]; c.join()
            self.assertEqual(c.jobs[job["id"]]["status"], "succeeded")
            self.assertEqual(c.reconcile({"job_id": job["id"]})["observation"]["result"].get("declared_service"), value)

    def test_malformed_declared_service_is_not_promoted_or_inferred(self):
        good = {"authentication": {"kind": "none"}, "operation_tools": {"read": "actual_read"}}
        variants = [None, {}, dict(good, unexpected=True), dict(good, authentication={"kind": "none", "owner": "hidden"}),
                    dict(good, authentication={"kind": "binding"}), dict(good, operation_tools={}),
                    dict(good, operation_tools={"read": "same", "other": "same"}),
                    dict(good, operation_tools={"read": "bad\nname"}), dict(good, operation_tools={"read": []})]
        c = self.controller()
        for index, declaration in enumerate(variants):
            with self.subTest(index=index), self.assertRaises(backend.SetupError):
                c.selected_check(self.stage(name="decl-" + str(index),
                    mutate=lambda plan: plan.update(declared_service=declaration)))
        self.assertIsNone(c.job)

    def test_selected_only_check_is_noninstalling_idempotent_and_bound(self):
        c = self.controller()
        request = self.stage()
        first = c.selected_check(request)
        self.assertEqual(first, c.selected_check(request))
        self.assertEqual(c.state()["families"], [])
        self.assertFalse(c.state()["catalog"]["available"])
        self.assertIsNone(c.job)
        self.assertEqual(len(c.runner.calls), 1)
        self.assertEqual(c.runner.calls[0][0][1], "--package")
        self.assertEqual(first["scope"], backend.SELECTED_PROPOSAL)
        self.assertEqual(len(first["files"]), 4)
        self.assertFalse(Path(first["destination"]).exists())

    def test_both_profiles_install_frozen_bytes_after_staging_removed(self):
        for profile in ("python", "node"):
            with self.subTest(profile=profile):
                c = self.controller()
                checked = c.selected_check(self.stage(profile, profile))
                shutil.rmtree(self.staging / profile)
                job = c.selected_install(self.selection(checked))["job"]
                c.join()
                self.assertEqual(c.jobs[job["id"]]["status"], "succeeded")
                replay = c.selected_install(self.selection(checked))
                self.assertEqual(replay["job"]["id"], job["id"])
                self.assertEqual(sum("--destination" in a and "--verify-existing" not in a for a, _ in c.runner.calls), 1)
                self.assertEqual(c.jobs[job["id"]]["result"]["selection_kind"], profile)
                self.assertEqual(c.reconcile({"job_id": job["id"]})["observation"]["status"], "matched")

    def test_changed_stage_refuses_recheck_but_not_checked_install(self):
        c = self.controller(); request = self.stage(); checked = c.selected_check(request)
        (self.staging / "choice/closure/dependency").write_bytes(b"replacement")
        with self.assertRaises(backend.SetupError):
            c.selected_check(request)
        c.selected_install(self.selection(checked)); c.join()
        self.assertEqual(c.job["status"], "succeeded")

    def test_tampered_proposal_or_frozen_tool_refuses_before_job(self):
        for target in ("effect", "artifact", "tool"):
            with self.subTest(target=target):
                c = self.controller(); checked = c.selected_check(self.stage(name=target))
                path = (c.workspace / "selected-tools/python" if target == "tool" else
                        c.workspace / "selections" / checked["validation_id"] / ("effect.json" if target == "effect" else "artifact.whl"))
                path.write_bytes(b"tampered")
                with self.assertRaises(backend.SetupError):
                    c.selected_install(self.selection(checked))
                self.assertIsNone(c.job)
                self.assertEqual(len(c.runner.calls), 1)

    def test_links_fifo_and_path_requests_are_refused(self):
        c = self.controller(); request = self.stage()
        for name in ("../choice", "/choice", "a/b", ".", ".."):
            with self.assertRaises(backend.SetupError):
                c.selected_check(dict(request, staged_id=name))
        for target in ("package.json", "artifact.whl", "closure/dependency"):
            path = self.staging / "choice" / target
            data = path.read_bytes(); path.unlink(); path.symlink_to(self.root / "installer.tool")
            with self.assertRaises(backend.SetupError):
                c.selected_check(request)
            path.unlink(); path.write_bytes(data)
        path = self.staging / "choice/artifact.whl"; path.unlink(); os.mkfifo(path)
        with self.assertRaises(backend.SetupError):
            c.selected_check(request)
        self.assertIsNone(c.job)

    def test_intermediate_proposal_symlink_refused(self):
        c = self.controller(); checked = c.selected_check(self.stage())
        old = c.workspace / "selections"; moved = self.root / "moved"; old.rename(moved); old.symlink_to(moved, target_is_directory=True)
        with self.assertRaises(backend.SetupError):
            c.selected_install(self.selection(checked))
        self.assertIsNone(c.job)

    def test_malformed_plan_and_host_binding_fail_closed(self):
        c = self.controller()
        mutations = [lambda p: p.update(schema_version=True), lambda p: p.update(component="other"),
                     lambda p: p.update(closure={"../escape": "a" * 64}), lambda p: p.update(unknown=True),
                     lambda p: p.update(closure={}), lambda p: p.update(artifact_digest="wrong")]
        for i, mutate in enumerate(mutations):
            with self.assertRaises(backend.SetupError):
                c.selected_check(self.stage(name="bad" + str(i), mutate=mutate))
        self.assertIsNone(c.job)
        self.profile["scope"] = "activate_and_grant"; self.write_profile()
        with self.assertRaises(backend.SetupError):
            self.controller()

    def test_enabled_profile_and_exact_effect_required(self):
        self.profile["validators"].pop("node"); self.write_profile()
        c = self.controller()
        with self.assertRaises(backend.SetupError):
            c.selected_check(self.stage("node", "node"))
        checked = c.selected_check(self.stage())
        with self.assertRaises(backend.SetupError):
            c.selected_install(dict(self.selection(checked), effect_sha256="0" * 64))
        with self.assertRaises(backend.SetupError):
            c.install(dict(validation_id=checked["validation_id"], configuration_sha256="0" * 64,
                           family="agno", profile="x", operations=[], values={}))
        self.assertIsNone(c.job)

    def test_partial_nonzero_never_retries_and_reconciles_separately(self):
        c = self.controller(); checked = c.selected_check(self.stage()); c.runner.nonzero = True
        first = c.selected_install(self.selection(checked))["job"]; c.join()
        self.assertEqual(c.job["status"], "unconfirmed")
        self.assertEqual(c.selected_install(self.selection(checked))["job"]["id"], first["id"])
        self.assertEqual(c.reconcile({"job_id": first["id"]})["observation"]["status"], "matched")
        self.assertEqual(c.job["status"], "unconfirmed")
        self.assertEqual(sum("--destination" in a and "--verify-existing" not in a for a, _ in c.runner.calls), 1)

    def test_thread_start_failure_retains_one_request_no_install(self):
        c = self.controller(); checked = c.selected_check(self.stage())
        with patch.object(backend.threading.Thread, "start", side_effect=RuntimeError("PRIVATE")):
            job = c.selected_install(self.selection(checked))["job"]
        self.assertEqual(job["status"], "unconfirmed")
        self.assertIn("recovery_request", job)
        self.assertEqual(c.selected_install(self.selection(checked))["job"]["id"], job["id"])
        self.assertEqual(len(c.runner.calls), 1)
        self.assertTrue((c.workspace / "requests" / (job["id"] + ".json")).is_file())

    def test_recovery_uses_original_intent_and_unknown_history(self):
        c, checked, job = self.installed()
        shutil.rmtree(self.staging)
        recovered = self.recover(c, job)
        self.assertTrue(recovered.recovery_only)
        self.assertEqual(recovered.job["error"], "historical_outcome_unknown")
        self.assertEqual(recovered.reconcile({"job_id": job["id"]})["observation"]["status"], "matched")
        self.assertEqual(recovered.job["status"], "unconfirmed")
        with self.assertRaises(backend.SetupError):
            recovered.selected_install(self.selection(checked))
        self.assertEqual(len(recovered.runner.calls), 1)
        (c.workspace / "selections" / checked["validation_id"] / "artifact.whl").write_bytes(b"tampered")
        self.assertEqual(recovered.reconcile({"job_id": job["id"]})["observation"]["status"], "unconfirmed")

    def test_receipt_64mib_bound_and_forged_proof_rejected(self):
        c = self.controller(); c.runner.receipt_padding = 2 * 1024 * 1024
        checked = c.selected_check(self.stage()); c.selected_install(self.selection(checked)); c.join()
        self.assertEqual(c.job["status"], "succeeded")
        for changes in ({"selection_kind": "node"}, {"component": "other"}, {"verified": 1}, {"schema_version": True},
                        {"historical_receipt": {"sha256": "0" * 64}}, {"command": []}):
            c.runner.bad_proof = changes
            self.assertEqual(c.reconcile({"job_id": c.job["id"]})["observation"]["status"], "unconfirmed")


class SelectedMcpTests(SelectedFixture, unittest.IsolatedAsyncioTestCase):
    async def test_actual_mcp_selected_only_schema_check_install_and_recovery(self):
        from mcp import Client
        c = self.controller(); request = self.stage(); owner = adapter.SetupAdapter(c)
        try:
            async with Client(adapter.create_server(owner)) as client:
                tools = (await client.list_tools()).tools
                by_name = {t.name: t for t in tools}
                self.assertIn("setup_selected_check", by_name)
                self.assertFalse(by_name["setup_selected_check"].annotations.read_only_hint)
                self.assertNotIn("path", by_name["setup_selected_check"].input_schema["properties"])
                bad = await client.call_tool("setup_selected_check", dict(request, path="/forbidden"))
                self.assertTrue(bad.is_error)
                result = await client.call_tool("setup_selected_check", request)
                self.assertFalse(result.is_error, result)
                checked = result.structured_content
                installed = await client.call_tool("setup_selected_install", self.selection(checked))
                self.assertFalse(installed.is_error, installed)
                await asyncio.to_thread(c.join)
                replay = await client.call_tool("setup_selected_install", self.selection(checked))
                self.assertEqual(installed.structured_content["job"]["id"], replay.structured_content["job"]["id"])
                job = replay.structured_content["job"]
                self.assertEqual(job["status"], "succeeded")
        finally:
            await owner.aclose()
        recovered = self.recover(c, job); owner = adapter.SetupAdapter(recovered)
        try:
            async with Client(adapter.create_server(owner)) as client:
                self.assertEqual({t.name for t in (await client.list_tools()).tools}, {"setup_recovery_manifest", "setup_state", "setup_reconcile"})
                result = await client.call_tool("setup_reconcile", {"job_id": job["id"]})
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content["observation"]["status"], "matched")
        finally:
            await owner.aclose()

    async def test_cancelled_dispatch_retains_install_and_replay(self):
        c = self.controller(); checked = c.selected_check(self.stage()); owner = adapter.SetupAdapter(c)
        c.runner.block = threading.Event()
        task = asyncio.create_task(owner.dispatch("/api/selected_install", self.selection(checked)))
        await asyncio.to_thread(c.runner.entered.wait, 5)
        task.cancel("PRIVATE_CANCEL")
        await asyncio.sleep(0)
        self.assertFalse(task.done(), "entered installation remains joined")
        self.assertNotIn("stop", c.job)
        c.runner.block.set()
        with self.assertRaises(asyncio.CancelledError):
            await task
        await asyncio.to_thread(c.join)
        replay = await owner.dispatch("/api/selected_install", self.selection(checked))
        self.assertEqual(replay["job"]["status"], "succeeded")
        self.assertEqual(sum("--destination" in a and "--verify-existing" not in a for a, _ in c.runner.calls), 1)
        await owner.aclose()


if __name__ == "__main__":
    unittest.main()
