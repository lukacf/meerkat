"""Inert assembler contract tests, not native or product acceptance.

All executables, resources and accepted-looking records below are fabricated
test inputs in temporary directories. Nothing packaged here can be run or
distributed. Production validators, extraction and archive writing remain real.
"""
from __future__ import annotations

import contextlib
import base64
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import runpy
import struct
import sys
import tarfile
import tempfile
import tomllib
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ASSEMBLER = load("connect_product_assembler", ROOT / "scripts/assemble-connect-product.py")
PREPARE = runpy.run_path(str(ASSEMBLER.PREPARE))
PACKAGER = runpy.run_path(str(ASSEMBLER.RESOURCE_PACKAGER))
VERIFIER = load("connect_gateway_verifier", ASSEMBLER.GATEWAY_VERIFIER)


def encoded(value):
    return (json.dumps(value, indent=2) + "\n").encode()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def binary_bytes(target, resource_digest, *, elf_kind=3):
    if target == "macos-arm64":
        header = struct.pack("<IIIIIIII", 0xFEEDFACF, 0x0100000C, 0, 2, 0, 0, 0, 0)
    else:
        header = bytearray(64)
        header[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<HHIQ", header, 16, elf_kind, 62, 1, 0x1000)
        struct.pack_into("<H", header, 52, 64)
    return bytes(header) + b"INERT - NEVER EXECUTE\n" + resource_digest.encode()


class InertInputs:
    def __init__(self, root, target, *, manifest_target=None):
        self.root, self.target = root, target
        self.destination = root / "assembled-inert"
        self.prefix = "meerkat-connect-" + target
        package = root / "component" / self.prefix
        resources = package / "libexec/meerkat-connect"
        self.package, self.resources = package, resources
        self.explicit_kind = None
        resources.mkdir(parents=True)
        (package / "bin").mkdir()
        paths = {"prepare": "prepare.py", "python": "runtime/python/bin/python3.11",
                 "node": "runtime/node/bin/node", "npm_cli": "runtime/node/lib/npm.js",
                 "starter": "starter/start.py", "archive": "archives/inert-library.tar.gz"}
        for relative in paths.values():
            path = resources / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"INERT RESOURCE - NEVER EXECUTE\n")
            path.chmod(0o555 if relative == paths["python"] else 0o444)
        (resources / "runtime/node/bin/node-link").symlink_to("node")
        for path in resources.rglob("*"):
            if path.is_dir() and not path.is_symlink():
                path.chmod(0o555)
        manifest = {"schema_version": 1, "kind": "meerkat_connect_resources",
                    "target": manifest_target or target, **paths,
                    "entries": PREPARE["tree_entries"](resources)}
        raw = encoded(manifest)
        (resources / "resources.json").write_bytes(raw)
        (resources / "resources.json").chmod(0o444)
        resources.chmod(0o555)
        self.resource_sha = sha(raw)
        self.original_entries = manifest["entries"]
        self.component = root / "inert-resources.tar.gz"
        with tarfile.open(self.component, "w:gz") as archive:
            archive.add(package, arcname=self.prefix)
        self.selected = {**ASSEMBLER.TARGETS[target], "component": self.component,
                         "component_sha256": ASSEMBLER.digest(self.component),
                         "component_size": self.component.stat().st_size,
                         "resources_sha256": self.resource_sha}
        self.binary = root / "inert-cli"
        self.binary.write_bytes(binary_bytes(target, self.resource_sha))
        self.test_binary = root / "inert-test"
        self.test_binary.write_bytes(binary_bytes(target, self.resource_sha) + b"inert test")
        self.installed_test = root / "inert-copied-test"
        self.installed_test.write_bytes(self.test_binary.read_bytes())
        for name in ASSEMBLER.LICENSES:
            (root / name).write_bytes(b"INERT LICENSE TEST PAYLOAD\n")
        self.licenses = {name: ASSEMBLER.digest(root / name) for name in ASSEMBLER.LICENSES}
        self.source = root / "SYNTHETIC-source.json"
        self.source.write_bytes(encoded({"synthetic": True, "not_a_build": True}))
        self.evidence = root / "SYNTHETIC-evidence.txt"
        self.evidence.write_text("NOT NATIVE ACCEPTANCE - fabricated contract test\n")
        candidate = {"target": target, "source_manifest_sha256": ASSEMBLER.digest(self.source),
                     "resources_sha256": self.resource_sha,
                     "companion_cli_binary": self.identity(self.binary),
                     "test_executable": self.identity(self.test_binary)}
        self.composition = root / "SYNTHETIC-composition.json"
        self.composition_value = {
            "schema_version": 1, "kind": "meerkat_connect_installed_composition",
            "scope": "installed_product_native_catalog_preparation_and_help", "status": "passed",
            "candidate": copy.deepcopy(candidate),
            "test_executable": {"path": str(self.installed_test), **self.identity(self.installed_test)},
            "execution": {"exit_code": 0, "argv": [str(self.installed_test), "--exact", "--ignored",
                          "installed_product_native_catalog_preparation_and_help"]},
            "evidence": [self.reference(self.evidence)]}
        self.composition.write_bytes(encoded(self.composition_value))
        self.shutdown = root / "SYNTHETIC-shutdown.json"
        self.shutdown_value = {
            "schema_version": 1, "kind": "meerkat_connect_native_shutdown_review",
            "scope": "active_setup_worker_shutdown", "disposition": "accepted",
            "candidate": copy.deepcopy(candidate), "evidence": [self.reference(self.evidence)]}
        self.shutdown.write_bytes(encoded(self.shutdown_value))
        self.record = root / "SYNTHETIC-build.json"
        self.record_value = {
            "schema_version": 1, "target": target, "resources_sha256": self.resource_sha,
            "binary": {"path": str(self.binary), **self.identity(self.binary)},
            "cargo": {"argv": ["NEVER-EXECUTED-SYNTHETIC-BUILD"], "exit_code": 0,
                      "env": {"MEERKAT_CONNECT_RESOURCES_SHA256": self.resource_sha}},
            "source_manifest": self.reference(self.source),
            "test_executable": {"path": str(self.test_binary), **self.identity(self.test_binary)},
            "installed_composition": self.reference(self.composition),
            "native_shutdown_review": self.reference(self.shutdown)}
        self.record.write_bytes(encoded(self.record_value))

    @staticmethod
    def identity(path):
        return {"sha256": ASSEMBLER.digest(path), "size": path.stat().st_size}

    @staticmethod
    def reference(path):
        return {"path": str(path), "sha256": ASSEMBLER.digest(path)}

    def update_record(self):
        self.record_value["installed_composition"] = self.reference(self.composition)
        self.record_value["native_shutdown_review"] = self.reference(self.shutdown)
        self.record.write_bytes(encoded(self.record_value))

    def repin_resource_record(self):
        self.resource_value["archive_sha256"] = ASSEMBLER.digest(self.component)
        self.resource_value["archive_bytes"] = self.component.stat().st_size
        self.resource_record.write_bytes(encoded(self.resource_value))
        self.record_value["resource_build_record_sha256"] = ASSEMBLER.digest(self.resource_record)
        self.update_record()

    def select_explicit(self, kind="meerkat-connect", *, starter_readme=None):
        # Synthetic resources now bind the current verifier and a full inert
        # starter closure. No produced wheel, binary or native receipt is used.
        for directory, _, files in os.walk(self.resources, followlinks=False):
            Path(directory).chmod(0o755)
            for name in files:
                path = Path(directory) / name
                if not path.is_symlink():
                    path.chmod(0o644)
        (self.resources / "prepare.py").write_bytes(ASSEMBLER.PREPARE.read_bytes())
        starter = self.resources / "starter"
        for name in ASSEMBLER.STARTER_FILES:
            (starter / name).write_bytes(b"INERT STARTER - NEVER EXECUTE\n")
        if starter_readme is not None:
            (starter / "README.md").write_bytes(starter_readme)
        for directory, _, files in os.walk(self.resources, followlinks=False):
            for name in files:
                path = Path(directory) / name
                if not path.is_symlink():
                    path.chmod(0o444)
            if Path(directory) != self.resources:
                Path(directory).chmod(0o555)
        manifest = json.loads((self.resources / "resources.json").read_bytes())
        manifest["entries"] = PREPARE["tree_entries"](self.resources)
        raw = encoded(manifest)
        (self.resources / "resources.json").chmod(0o644)
        (self.resources / "resources.json").write_bytes(raw)
        (self.resources / "resources.json").chmod(0o444)
        self.resources.chmod(0o555)
        self.resource_sha = sha(raw)
        self.original_entries = manifest["entries"]
        build_root = self.root / "resource-build"
        build_root.mkdir()
        self.component = build_root / ("meerkat-connect-resources-" + self.target + ".tar.gz")
        with tarfile.open(self.component, "w:gz") as archive:
            archive.add(self.package, arcname=self.prefix)
        self.resource_record = build_root / "build.json"
        library = self.resources / manifest["archive"]
        self.resource_value = {
            "schema_version": 1, "target": self.target,
            "scope": self.target.replace("-", "_") + "_product_resources_only_native_cli_supplied_separately",
            "resources": str(build_root / self.prefix / "libexec/meerkat-connect"),
            "resources_sha256": self.resource_sha, "entries": len(manifest["entries"]),
            "archive": str(self.component), "archive_sha256": ASSEMBLER.digest(self.component),
            "archive_bytes": self.component.stat().st_size,
            "prepare_source_sha256": ASSEMBLER.digest(ASSEMBLER.PREPARE),
            "packager_source_sha256": ASSEMBLER.digest(ASSEMBLER.RESOURCE_PACKAGER),
            "input_selection": {"kind": "explicit_pinned_connect_resource_inputs",
                "library": {"path": str(library), "sha256": ASSEMBLER.digest(library), "bytes": library.stat().st_size},
                "starter": {"path": str(starter), "manifest_sha256": ASSEMBLER.digest(starter / "release-manifest.json"),
                    "files": {name: {"sha256": ASSEMBLER.digest(starter / name), "bytes": (starter / name).stat().st_size}
                              for name in ASSEMBLER.STARTER_FILES}}}}
        self.resource_record.write_bytes(encoded(self.resource_value))
        self.explicit_kind = kind
        self.binary.write_bytes(binary_bytes(self.target, self.resource_sha))
        self.test_binary.write_bytes(binary_bytes(self.target, self.resource_sha) + b"inert test")
        self.installed_test.write_bytes(self.test_binary.read_bytes())
        self.record_value.update(binary_kind=kind, resources_sha256=self.resource_sha,
            resource_build_record_sha256=ASSEMBLER.digest(self.resource_record),
            binary={"path": str(self.binary), **self.identity(self.binary)},
            test_executable={"path": str(self.test_binary), **self.identity(self.test_binary)})
        self.record_value["cargo"]["argv"] = ["NEVER-EXECUTED-SYNTHETIC-BUILD", "--bin", kind]
        self.record_value["cargo"]["env"]["MEERKAT_CONNECT_RESOURCES_SHA256"] = self.resource_sha
        candidate = {"target": self.target, "source_manifest_sha256": ASSEMBLER.digest(self.source),
            "resources_sha256": self.resource_sha, "binary_kind": kind, "binary": self.identity(self.binary),
            "resource_build_record_sha256": ASSEMBLER.digest(self.resource_record),
            "test_executable": self.identity(self.test_binary)}
        self.composition_value["candidate"] = copy.deepcopy(candidate)
        self.composition_value["test_executable"] = {"path": str(self.installed_test), **self.identity(self.installed_test)}
        self.shutdown_value["candidate"] = copy.deepcopy(candidate)
        self.composition.write_bytes(encoded(self.composition_value))
        self.shutdown.write_bytes(encoded(self.shutdown_value))
        self.update_record()

    def assemble(self, *, destination=None, default_target=False, stage_only=False, **overrides):
        with mock.patch.object(ASSEMBLER, "TARGETS", {**ASSEMBLER.TARGETS, self.target: self.selected}), \
                mock.patch.object(ASSEMBLER, "ROOT", self.root), \
                mock.patch.object(ASSEMBLER, "PREPARE_SHA", ASSEMBLER.digest(ASSEMBLER.PREPARE)), \
                mock.patch.object(ASSEMBLER, "RESOURCE_PACKAGER_SHA", ASSEMBLER.digest(ASSEMBLER.RESOURCE_PACKAGER)), \
                mock.patch.object(ASSEMBLER, "LICENSES", self.licenses):
            kwargs = {} if default_target else {"target": self.target}
            if self.explicit_kind is not None:
                kwargs.update(resource_record_path=self.resource_record,
                              resource_record_sha=ASSEMBLER.digest(self.resource_record),
                              binary_kind=self.explicit_kind)
            kwargs.update(stage_only=stage_only, **overrides)
            return ASSEMBLER.assemble(self.binary, ASSEMBLER.digest(self.binary), self.record,
                                      destination or self.destination, **kwargs)


@contextlib.contextmanager
def inputs(target="macos-arm64", **kwargs):
    with tempfile.TemporaryDirectory(prefix="inert-connect-assembly-") as scratch:
        root = Path(scratch).resolve()
        try:
            yield InertInputs(root, target, **kwargs)
        finally:
            # Only our temporary fixture; read-only resource dirs need write
            # permission restored for ordinary non-root test cleanup.
            for directory, _, _ in os.walk(root, followlinks=False):
                Path(directory).chmod(0o755)


class InertGatewayReport:
    """Fabricated existing-verifier files, never a native execution result."""
    def __init__(self, fixture):
        self.fixture = fixture
        fixture.select_explicit("toolkit_gateway")
        # Gateway qualification has no dependency on the companion CLI tests.
        for key in ("test_executable", "installed_composition", "native_shutdown_review"):
            fixture.record_value.pop(key)
        fixture.record.write_bytes(encoded(fixture.record_value))
        self.staging = fixture.assemble(stage_only=True)
        self.candidate = self.staging["candidate"]
        self.package = fixture.destination / fixture.prefix
        self.directory = fixture.root / "SYNTHETIC-gateway-evidence"
        self.directory.mkdir()
        self.path = self.directory / "manifest.json"
        self.report = {"schema": 1, "status": "passed", "script_sha256": ASSEMBLER.digest(ASSEMBLER.GATEWAY_VERIFIER),
            "toolkit_gateway": str(self.package / "bin/toolkit_gateway"),
            "toolkit_gateway_sha256": self.candidate["binary"]["sha256"],
            "raw_gateway": None, "raw_gateway_sha256": None, "installed_candidate": copy.deepcopy(self.candidate),
            "installed_inputs": {"staging": fixture.reference(fixture.destination / "staging.json"),
                "native_build": fixture.reference(fixture.destination / "build-record.json"),
                "resource_build": fixture.reference(fixture.destination / "resource-build-record.json"),
                "source_manifest": fixture.reference(fixture.source),
                "resource_manifest": fixture.reference(self.package / "libexec/meerkat-connect/resources.json")},
            "sdk_sources": [], "cases": []}
        for module, files in (("meerkat_toolkit", ("host_control.py", "application_runtime.py")),
                              ("meerkat_meta", ("__init__.py", "builder.py", "runtime.py", "_transport.py",
                                 "agent_builder.py", "identity_first_models.py", "identity_first_providers.py", "types.py"))):
            for name in files:
                self.report["sdk_sources"].append(self.write(fixture.root / "sdk" / module / name,
                    b"INERT SDK SOURCE - NEVER IMPORT\n"))
        self.events = {}
        for corrupt in (False, True):
            self.report["cases"].append(self.case(corrupt))
        self.save()

    def write(self, path, raw):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(raw)
        return {"path": str(path), "sha256": sha(raw), "bytes": len(raw)}

    def document(self, path, value):
        return self.write(path, encoded(value))

    def case(self, corrupt):
        name = "installed-product-resource-corruption" if corrupt else "installed-product-success"
        directory = self.directory / name
        directory.mkdir()
        candidate = copy.deepcopy(self.candidate)
        config = self.write(directory / "meerkat.toml", b"# INERT selected model configuration\n")
        plan = {"schema": 1, "application": {"realm": {"state_root": str(directory / "state")}},
                "output": str(directory / "application-preparation")}
        plan_ref = self.document(directory / "application-plan.json", plan)
        result = {"case": name, "status": "passed", "candidate": candidate,
            "forced_cleanup": False, "scoped_execution_proven": False, "natural_gateway_reap": True,
            "sdk_readers_joined": True, "observed_children_gone": True, "observed_children": [],
            "provider_joined": True, "model_configuration": config,
            "preparation_plan": plan_ref, "termination": {"state": "exited", "exit_code": 1 if corrupt else 0,
                "signal": None, "escalation": "none", "native_shutdown": "acknowledged"}}
        gateway = self.package / "bin/toolkit_gateway"
        if corrupt:
            result["sentinel_connections"] = []
            clone = directory / self.package.name
            clone_gateway = clone / "bin/toolkit_gateway"
            self.write(clone_gateway, gateway.read_bytes())
            self.write(clone / "libexec/meerkat-connect/resources.json",
                       (self.package / "libexec/meerkat-connect/resources.json").read_bytes())
            before = self.fixture.reference(self.package / "libexec/meerkat-connect/prepare.py")
            after = self.write(clone / "libexec/meerkat-connect/prepare.py", b"INERT changed resource\n")
            refusal = ("Installed Connect resources are unavailable or changed; "
                       "reinstall the complete product. No setup command was started.")
            result.update(resource_corruption={"before": before, "after": after,
                            "marker": str(directory / "bundled-entry-entered")},
                          resource_refusal=refusal, connect_error={"type": "SyntheticFailure", "message": refusal},
                          gateway_stderr=self.write(directory / "gateway.stderr", refusal.encode()), finalization={})
            events = [{"event": "main_gateway_started", "pid": 9102, "binary": str(clone_gateway)}]
        else:
            pid = 9101
            selection = {"deployment_root": str(directory / "deployment")}
            for key in ("activation", "policy_manifest", "local_policy"):
                selection[key] = self.document(directory / "deployment" / (key + ".json"), {"synthetic": key})
            completed = directory / "application-preparation"
            setup = self.document(completed / "application-setup.json", {"synthetic": "completed native setup"})
            manifest = self.document(completed / "manifest.json", {"schema": 1, "kind": "application_setup_preparation",
                "artifacts": [{"path": "application-setup.json", "sha256": setup["sha256"]}]})
            handoff = self.document(directory / "preparer/prepare.stdout", {"mcpServers": {"toolkit-setup": {"synthetic": True}}})
            finalization = {"completed_manifest": manifest, "completed_setup": setup,
                            "preparer_handoff": handoff, "selection": selection}
            init = {"id": "init-request", "method": "meta/init", "params": {"init_id": "fixture-init",
                "runtime_options": {"meerkat_config_path": config["path"]},
                "persistent_state": plan["application"]["realm"]["state_root"],
                "prepared_application": {"digest": plan_ref["sha256"],
                    "bundle_base64": base64.b64encode(Path(plan_ref["path"]).read_bytes()).decode()}}}
            ready = {"init_id": "fixture-init", "outcome": "ready"}
            catalog = {"id": "catalog-request", "method": "meta/models/catalog", "params": {}}
            catalog_value = {"synthetic": "static catalog, not a model run"}
            shutdown = {"id": "shutdown-request", "method": "meta/shutdown", "params": {}}
            reply = {"id": shutdown["id"], "result": {"shutdown": True,
                     "runtime_cleanup_completed": True, "host_cleanup_completed": True}}
            events = [{"event": "main_gateway_started", "pid": pid, "binary": str(gateway)},
                      {"event": "sdk_wire_out", "message": init}]
            for index, (command_name, kind) in enumerate((("prepare-policy", "application_policy_prepared"),
                                                 ("validate-policy", "application_policy_validated"))):
                argv = [str(gateway), "--" + command_name.replace("-policy", "-application-policy"), "INERT-ARGS"]
                finalization[command_name] = {"argv": argv, "pid": pid + 10 + index, "exit_code": 0,
                    "stdout": self.document(directory / (command_name + ".stdout"), {"kind": kind, "selection": selection}),
                    "stderr": self.write(directory / (command_name + ".stderr"), b"")}
                events.append({"event": "native_artifact_command", "name": command_name,
                               "pid": pid + 10 + index, "argv": argv[1:]})
            events += [{"event": "fixture_publish_entered", "selection": selection}, {"event": "fixture_published"},
                {"event": "sdk_init_event", "method": "meta/init_settled", "params": ready},
                {"event": "sdk_wire_out", "message": catalog},
                {"event": "sdk_wire_response", "message": {"id": catalog["id"], "result": catalog_value}},
                {"event": "sdk_wire_out", "message": shutdown}, {"event": "sdk_wire_response", "message": reply}]
            result.update(main_gateway_pid=pid, native_ready=ready, finalization=finalization,
                application_selection=selection, shutdown_response=reply, http_listener_closed=True,
                gateway_stderr=self.write(directory / "gateway.stderr", b""),
                model_catalog=self.document(directory / "model-catalog.json", catalog_value))
            self.acquisition(result, events, directory)
        self.events[name] = events
        result["events"] = self.document(directory / "events.json", events)
        return result

    def acquisition(self, case, events, directory):
        """Current producer fields with synthetic values; no native work occurs."""
        original = {"route": VERIFIER.ACQUISITION_ROUTE, "original": "inert-original-a", "event_sha256": "a" * 64}
        fresh = {**original, "original": "inert-original-b", "event_sha256": "b" * 64}
        admission = {"state": "admitted", "source_settlement": "fresh_acceptance_promoted",
            "deduplicated": False, "turn_ticket": "inert-ticket", "session_id": "inert-session", "input_id": "inert-input-a"}
        authenticated = {"event": {"route": original["route"], "channel": VERIFIER.ACQUISITION_CHANNEL,
            "sender": "fixture-requester", "occurrence": original["original"], "content": VERIFIER.ACQUISITION_REQUEST},
            "signature_sha256": "c" * 64, "authority": VERIFIER.installed_fixture_principal("event-source")}
        proposal = {"original": original, "issuance_id": "inert-parent", "decision": {
            "route": original["route"], "decision": fresh["original"]}}
        proposal_pin = self.document(directory / "service-parent-proposal.json", proposal)
        observed = {"outcome": "succeeded", "custody": "joined", "reason": None, "parent_issued": True,
            "parent_issuance_id": proposal["issuance_id"], "child": {"state": "grant_issued", "grant": {
                "route": original["route"], "decision": fresh["original"]}}, "source": fresh}
        story = {"id": 123, "title": "INERT synthetic story"}
        tool_result = json.dumps([story]) + "\n" + json.dumps({"value": json.dumps([story])})
        requests = [{"model": "fixture-helper-model", "messages": [{"role": "user", "content": VERIFIER.ACQUISITION_REQUEST}]},
            {"model": "fixture-original-model", "messages": [
                {"role": "user", "content": VERIFIER.ACQUISITION_REQUEST},
                {"role": "assistant", "content": VERIFIER.ORIGINAL_A_FINISH},
                {"role": "user", "content": VERIFIER.ACQUISITION_REQUEST},
                {"role": "tool", "tool_call_id": VERIFIER.ORIGINAL_STORY_CALL, "content": tool_result}],
                "tools": [{"type": "function", "function": {"name": VERIFIER.ACQUISITION_OPERATION}}]}]
        business = {"story": story, "answer": "Hacker News story 123: INERT synthetic story",
            "actual_tool_result": tool_result, "a_input_history_index": 0, "b_input_history_index": 2,
            "matching_result_history_index": 3, "model_request_index": 1,
            "model_request_sha256": sha(VERIFIER.canonical(requests[1])),
            "native_b_admission": "not_observed", "native_b_terminal": "not_observed"}
        native_b = {"state": "admitted", "session_id": admission["session_id"], "input_id": "inert-input-b",
            "receipt": {"source": "live_runtime", "report": {"status": "finalized", "input_id": "inert-input-b",
                "attempt_count": 1, "run_id": "inert-run-b", "scope": {"kind": "run", "run_id": "inert-run-b"},
                "owner_input_id": "inert-input-b", "recipient_input_ids": ["inert-input-b"],
                "terminal": {"outcome_type": "consumed"}, "outcome": {"completion_type": "completed",
                    "session_id": admission["session_id"], "tool_calls": 1, "text": business["answer"]}}}}
        custody = {"exchange_id": "inert-exchange", "transfer": "sdk_owned", "retirement": "not_observed",
            "proposal": proposal_pin, "ack": {"state": "answer_forwarded"}, "operation_observation": observed}
        acquisition = {"scope": "one actual installed SDK public read with an exact finalized native B result",
            "authenticated_event": self.document(directory / "authenticated-fixture-event.json", authenticated),
            "original": original, "admission": admission, "proposal": proposal_pin,
            "ack": custody["ack"], "observation": observed, "custody": custody, "rule_releases_after_joined": 1,
            "helper_result": {"state": "authorized", "grant_issued": True, "fresh_instruction_retained": True, "ready": False},
            "business": business, "fresh_original": fresh, "native_b": native_b, "native_receipt_reads": 1,
            "service_readiness": "not_independently_observed", "original_task_completion": "native_finalized_completed_for_exact_b_input"}
        # Preserve the producer's timing: business evidence precedes retirement;
        # the returned acquisition shares the finally-mutated custody object.
        case["original_business_observation"] = self.document(directory / "original-business-observation.json", acquisition)
        custody.update(retirement="joined", retirement_observation=copy.deepcopy(observed),
            rule_owner_created=True, rule_releases_after_joined=1, rule_exclusion_still_held=False)
        case.update(acquisition=acquisition, service_parent_custody=self.document(directory / "service-parent-custody.json", custody),
            model_requests=self.document(directory / "model-requests.json", requests), model_request_count=len(requests), model_schedule_errors=[])
        events[-2:-2] = [{"event": "authenticated_original_admitted", "original": copy.deepcopy(original), "outcome": copy.deepcopy(admission)},
            {"event": "original_public_answer_prepared", **copy.deepcopy(business)},
            {"event": "original_native_receipt_observed", "original": copy.deepcopy(fresh), "observation": copy.deepcopy(native_b)},
            {"event": "service_parent_custody_observed", **copy.deepcopy(custody)}]

    def save(self):
        for case in self.report["cases"]:
            directory = self.directory / case["case"]
            case["events"] = self.document(directory / "events.json", self.events[case["case"]])
            self.document(directory / "result.json", case)
        self.path.write_bytes(encoded(self.report))

    def save_business_correlations(self):
        case = self.report["cases"][0]
        acquisition = case["acquisition"]
        path = Path(case["original_business_observation"]["path"])
        record = copy.deepcopy(acquisition)
        record["custody"] = json.loads(path.read_bytes())["custody"]
        case["original_business_observation"] = self.document(path, record)
        for event in self.events[case["case"]]:
            if event["event"] == "original_public_answer_prepared":
                event.update(copy.deepcopy(acquisition["business"]))
            elif event["event"] == "original_native_receipt_observed":
                event["observation"] = copy.deepcopy(acquisition["native_b"])
        self.save()

    def assemble(self, **overrides):
        kwargs = {"gateway_verification_path": self.path, "gateway_verification_sha": ASSEMBLER.digest(self.path)}
        kwargs.update(overrides)
        return self.fixture.assemble(destination=self.fixture.root / "final-gateway", **kwargs)


class InstalledVerifierBoundaryTests(unittest.TestCase):
    """Exercise only pre-import admission and negative observation checks."""
    def test_public_catalog_plan_explicitly_selects_whole_candidate_policy(self):
        for selected in (False, True):
            with self.subTest(selected=selected), tempfile.TemporaryDirectory() as scratch:
                plan, _, _ = VERIFIER.installed_plan(Path(scratch).resolve(), [],
                    acquisition=True, catalog_policy=selected)
                expected = {"installation": "checked_software", "access": "provision_task_parent"}
                if selected:
                    expected["tool_policy"] = "until_superseded"
                self.assertEqual(plan["acquisition"]["standing_policy"], expected)
                original = plan["application"]["tool_policy"]["members"][1]
                self.assertFalse(any(row["tool_name"].startswith("tool_catalog_") for row in original["grants"]))
        with tempfile.TemporaryDirectory() as scratch:
            with self.assertRaises(AssertionError):
                VERIFIER.installed_plan(Path(scratch), [], catalog_policy=True)

    def test_service_observer_stops_on_schedule_failure_without_replay_or_signal(self):
        import asyncio, threading, time
        provider = mock.Mock(lock=threading.RLock(), errors=[])
        process = mock.Mock(); process.poll.return_value = None
        request = object()
        calls = []
        def observed(*, timeout):
            calls.append(timeout)
            if len(calls) == 1:
                raise TimeoutError("same SDK exchange pending")
            return request
        self.assertIs(asyncio.run(VERIFIER.observe_installed_service_reply(
            observed, provider, process, time.monotonic() + 10)), request)
        self.assertEqual(len(calls), 2)
        self.assertTrue(all(0 < value <= 0.25 for value in calls))
        def failed(*, timeout):
            provider.errors.append("exact synthetic schedule error")
            raise TimeoutError("same SDK exchange pending")
        observer = mock.Mock(side_effect=failed)
        with self.assertRaisesRegex(AssertionError, "exact synthetic schedule error"):
            asyncio.run(VERIFIER.observe_installed_service_reply(observer, provider, process, time.monotonic() + 10))
        observer.assert_called_once()
        process.terminate.assert_not_called(); process.kill.assert_not_called()

    def test_catalog_business_count_requires_exact_three_call_schedule(self):
        operation = VERIFIER.ACQUISITION_OPERATION
        messages = [{"role": "user", "content": "B"}]
        catalog = {"operation": operation, "helper_catalog_scope_reused": False}
        expected = [("search", "original-catalog-search", "tool_catalog_search", {"query": operation, "limit": 50}),
                    ("load", "original-catalog-load", "tool_catalog_load", {"names": [operation]}),
                    ("story", "original-public-story", operation, {"num_stories": 1})]
        for label, identifier, name, args in expected:
            messages.append({"role": "assistant", "tool_calls": [{"id": identifier,
                "function": {"name": name, "arguments": json.dumps(args)}}]})
            if label != "story":
                catalog[label + "_call_id"] = identifier
                catalog[label + "_result_history_index"] = len(messages)
                catalog[label + "_result"] = {"exact": label}
            messages.append({"role": "tool", "tool_call_id": identifier, "content": json.dumps({"exact": label})})
        business = {"original_catalog": catalog}
        self.assertEqual(ASSEMBLER.gateway_business_tool_count(business, messages, 0, 6), 3)
        self.assertEqual(ASSEMBLER.gateway_business_tool_count({}, messages, 0, 6), 1)
        for change in (lambda rows: rows[1]["tool_calls"][0]["function"].update(name="other"),
                       lambda rows: rows[3]["tool_calls"][0]["function"].update(arguments='{"names":["other"]}'),
                       lambda rows: rows[5]["tool_calls"].append(copy.deepcopy(rows[5]["tool_calls"][0])),
                       lambda rows: rows[4].update(content='{}')):
            wrong = copy.deepcopy(messages); change(wrong)
            with self.assertRaises(ValueError):
                ASSEMBLER.gateway_business_tool_count(business, wrong, 0, 6)

    def test_installed_plan_establishes_private_deployment_parents_before_startup(self):
        for acquisition in (False, True):
            with self.subTest(acquisition=acquisition), tempfile.TemporaryDirectory(
                    prefix="inert-installed-plan-") as scratch:
                directory = Path(scratch).resolve()
                with mock.patch.object(VERIFIER.subprocess, "Popen",
                        side_effect=AssertionError("no child may start")) as children:
                    plan, plan_pin, _ = VERIFIER.installed_plan(directory, [], acquisition=acquisition)
                root = Path(plan["deployment_root"])
                self.assertEqual(root, directory / "deployment")
                for relative in (".", "shared", "shared/data"):
                    path = root / relative
                    metadata = path.lstat()
                    self.assertTrue(VERIFIER.stat.S_ISDIR(metadata.st_mode))
                    self.assertEqual(VERIFIER.stat.S_IMODE(metadata.st_mode), 0o700)
                    self.assertTrue(path.resolve(strict=True).is_relative_to(root))
                # Only the real installation owner may create its native pair.
                self.assertEqual(list((root / "shared/data").iterdir()), [])
                self.assertFalse(Path(plan["output"]).exists())
                self.assertEqual(plan["startup"]["kind"], "initialize")
                self.assertEqual(Path(plan_pin["path"]).read_bytes(), VERIFIER.canonical(plan))
                children.assert_not_called()

    def test_installed_models_keep_declared_default_and_exact_profile_bindings(self):
        for acquisition in (False, True):
            with self.subTest(acquisition=acquisition), tempfile.TemporaryDirectory(
                    prefix="inert-installed-models-") as scratch:
                directory = Path(scratch).resolve()
                with mock.patch.object(VERIFIER.subprocess, "Popen",
                        side_effect=AssertionError("no child may start")) as children:
                    config = tomllib.loads(VERIFIER.config_text(18443, acquisition=acquisition))
                    plan, _, _ = VERIFIER.installed_plan(directory, [], acquisition=acquisition)
                models = config["self_hosted"]["models"]
                expected = {"fixture-model"}
                if acquisition:
                    expected.update(("fixture-original-model", "fixture-helper-model"))
                self.assertEqual(set(models), expected)
                self.assertEqual(config["self_hosted"]["default_model"], "fixture-model")
                self.assertIn(config["self_hosted"]["default_model"], models)
                self.assertEqual(config["self_hosted"]["servers"]["local"]["base_url"],
                                 "http://127.0.0.1:18443/v1")
                for model in expected:
                    self.assertEqual(models[model], {"server": "local", "remote_model": model})
                definition = tomllib.loads(plan["application"]["definition"]["text"])
                profiles = definition["profiles"]
                self.assertEqual(profiles["research"]["model"],
                                 "fixture-helper-model" if acquisition else "fixture-model")
                self.assertEqual(set(profiles), {"research", "original"} if acquisition else {"research"})
                if acquisition:
                    self.assertEqual(profiles["original"]["model"], "fixture-original-model")
                realm = config["realm"][plan["application"]["realm"]["realm_id"]]
                binding = realm["binding"][realm["default_binding"]]
                backend = realm["backend"][binding["backend_profile"]]
                auth = realm["auth"][binding["auth_profile"]]
                for profile in profiles.values():
                    self.assertIn(profile["model"], models)
                    self.assertEqual(profile["provider"], "self_hosted")
                    self.assertEqual(profile["self_hosted_server_id"], models[profile["model"]]["server"])
                    self.assertEqual(backend["server"], profile["self_hosted_server_id"])
                    self.assertEqual(backend["provider"], profile["provider"])
                    self.assertEqual(auth["provider"], profile["provider"])
                self.assertEqual(auth["auth_method"], "none")
                self.assertEqual(auth["source"], {"kind": "platform_default"})
                children.assert_not_called()

    def test_installed_profile_bundles_keep_session_account_composition(self):
        for acquisition in (False, True):
            with self.subTest(acquisition=acquisition), tempfile.TemporaryDirectory(
                    prefix="inert-installed-bundles-") as scratch:
                directory = Path(scratch).resolve()
                with mock.patch.object(VERIFIER.subprocess, "Popen",
                        side_effect=AssertionError("no child may start")) as children:
                    plan, _, _ = VERIFIER.installed_plan(directory, [], acquisition=acquisition)
                application = plan["application"]
                profiles = tomllib.loads(application["definition"]["text"])["profiles"]
                # PreparedGatewayRuntime registers setup and setup_results.
                # The helper projection adds setup_results and the real MCP
                # server; the selected session adapter supplies account tools.
                expected = {"research": []}
                if acquisition:
                    expected["original"] = ["meerkat_toolkit.setup"]
                self.assertEqual({name: profile["tools"].get("rust_bundles", [])
                                  for name, profile in profiles.items()}, expected)
                self.assertEqual(application["helper_profile"], "dedicated")
                self.assertEqual(application["helper_tools"],
                                 "checked_installation" if acquisition else "preparation")
                if acquisition:
                    account_target = {"kind": "registered_tool_bundle_operation",
                        "bundle": "meerkat_toolkit.setup_accounts",
                        "operation": "toolkit_request_public_service"}
                    helper_targets = plan["authorities"]["agent_setup"][0]["helper_targets"]
                    self.assertIn(account_target, [row["target"] for row in helper_targets])
                    helper = next(row for row in application["tool_policy"]["members"]
                                  if row["member_identity"] == "helper")
                    self.assertIn({"tool_name": "toolkit_request_public_service",
                                   "action": "invoke", "consequence": "r2"}, helper["grants"])
                children.assert_not_called()

    def test_output_admission_precedes_directory_creation_and_sdk_import(self):
        with inputs() as fixture:
            fixture.select_explicit("toolkit_gateway")
            fixture.assemble(stage_only=True)
            package = fixture.destination / fixture.prefix
            staging = fixture.destination / "staging.json"
            alias = fixture.root / "package-alias"
            alias.symlink_to(package, target_is_directory=True)
            before = sorted(str(path.relative_to(package)) for path in package.rglob("*"))
            prepare = package / "libexec/meerkat-connect/prepare.py"
            original = prepare.read_bytes()
            outputs = {"package": package, "descendant": package / "verification",
                       "alias": alias / "verification", "sibling": fixture.destination / "verification"}
            for name, output in outputs.items():
                with self.subTest(output=name):
                    argv = [str(ASSEMBLER.GATEWAY_VERIFIER), "--case", "installed-product",
                        "--toolkit-gateway", str(package / "bin/toolkit_gateway"),
                        "--toolkit-python", str(fixture.root / "absent-toolkit-sdk"),
                        "--meta-python", str(fixture.root / "absent-meta-sdk"),
                        "--installed-staging", str(staging),
                        "--installed-staging-sha256", ASSEMBLER.digest(staging),
                        "--output", str(output)]
                    with mock.patch.object(VERIFIER.sys, "argv", argv), \
                            mock.patch.object(VERIFIER.sys, "path", list(sys.path)), \
                            mock.patch.object(VERIFIER.sys, "platform", "darwin"), \
                            mock.patch.object(VERIFIER.platform, "machine", return_value="arm64"), \
                            mock.patch.object(VERIFIER.platform, "platform", return_value="inert fixture platform"), \
                            mock.patch.object(VERIFIER.importlib, "import_module",
                                side_effect=RuntimeError("inert SDK import boundary")) as imports, \
                            mock.patch.object(VERIFIER, "run_installed_product",
                                side_effect=AssertionError("no native case may start")) as cases, \
                            mock.patch.object(VERIFIER.subprocess, "Popen",
                                side_effect=AssertionError("no child may start")) as children, \
                            contextlib.redirect_stderr(io.StringIO()) as diagnostic:
                        if name == "sibling":
                            self.assertEqual(VERIFIER.main(), 1)
                            imports.assert_called_once_with("meerkat_toolkit.host_control")
                            report = json.loads((output / "manifest.json").read_bytes())
                            self.assertEqual(report["status"], "prerequisite_failed")
                            self.assertEqual(report["cases"], [])
                        else:
                            with self.assertRaises(SystemExit) as refused:
                                VERIFIER.main()
                            self.assertEqual(refused.exception.code, 2)
                            self.assertIn("outside the selected installed package", diagnostic.getvalue())
                            imports.assert_not_called()
                            if name != "package":
                                self.assertFalse(output.exists())
                        cases.assert_not_called()
                        children.assert_not_called()
                    self.assertEqual(sorted(str(path.relative_to(package)) for path in package.rglob("*")), before)
                    self.assertEqual(prepare.read_bytes(), original)

    def test_negative_recheck_refuses_late_effects_and_original_resource_change(self):
        for change in ("none", "marker", "preparation", "dangling_preparation", "finalizer", "ready", "original"):
            with self.subTest(change=change), tempfile.TemporaryDirectory(prefix="inert-negative-observation-") as scratch:
                root = Path(scratch).resolve()
                original = root / "original-prepare.py"
                original.write_bytes(b"INERT ORIGINAL RESOURCE - NEVER EXECUTE\n")
                finalizer = mock.Mock(entered=False)
                plan = {"output": str(root / "application-preparation")}
                corruption = {"before": VERIFIER.bytes_pin(original, original.read_bytes()),
                              "marker": str(root / "bundled-entry-entered")}
                settlements = []
                VERIFIER.require_installed_resource_refusal(finalizer, plan, corruption, settlements)
                if change == "marker": Path(corruption["marker"]).write_text("late entry")
                if change == "preparation": Path(plan["output"]).mkdir()
                if change == "dangling_preparation": Path(plan["output"]).symlink_to(root / "missing-preparation")
                if change == "finalizer": finalizer.entered = True
                if change == "ready": settlements.append({"method": "meta/init_settled", "params": {"outcome": "ready"}})
                if change == "original": original.write_bytes(b"CHANGED ORIGINAL RESOURCE\n")
                if change == "none":
                    settlements.append({"method": "meta/init_settled", "params": {"outcome": "failed"}})
                    VERIFIER.require_installed_resource_refusal(finalizer, plan, corruption, settlements)
                else:
                    with self.assertRaises(AssertionError):
                        VERIFIER.require_installed_resource_refusal(finalizer, plan, corruption, settlements)


    def test_descendant_failure_retains_evidence_without_excusing_live_pids(self):
        observed = {pid: {"pid": pid, "ppid": 40, "pgid": 40,
            "process_stat": {"status": "observed", "state": "S", "start_ticks": 100}}
            for pid in (101, 102, 103)}

        def liveness(pid, signal):
            self.assertEqual(signal, 0)
            if pid in (101, 103):
                raise ProcessLookupError()
            self.assertEqual(pid, 102)

        for diagnostic in ({"status": "observed", "state": "Z", "start_ticks": 100},
                {"status": "observed", "state": "S", "start_ticks": 999},
                {"status": "unavailable", "reason": "not_found"}):
            with self.subTest(diagnostic=diagnostic), \
                    mock.patch.object(VERIFIER.os, "kill", side_effect=liveness) as kills, \
                    mock.patch.object(VERIFIER, "descendant_process_stat", side_effect=[
                        {"status": "unavailable", "reason": "not_found"}, diagnostic,
                        {"status": "unavailable", "reason": "not_found"}]):
                report = {}
                with self.assertRaisesRegex(AssertionError, "observed native descendant survives shutdown"):
                    VERIFIER.require_descendants_gone(report, observed)
                retained = json.loads(json.dumps(report))
                self.assertEqual(retained["observed_children"], list(observed.values()))
                checks = retained["descendant_shutdown_checks"]
                self.assertEqual([(row["pid"], row["alive"]) for row in checks], [(101, False), (102, True), (103, False)])
                self.assertEqual(checks[1]["process_stat"], diagnostic)
                self.assertEqual(kills.call_args_list, [mock.call(101, 0), mock.call(102, 0), mock.call(103, 0)])

    def test_descendant_liveness_error_precedence_and_absence_are_unchanged(self):
        observed = {101: {"pid": 101, "ppid": 40, "pgid": 40}}
        failure = PermissionError("INERT-CANARY-NOT-FOR-REPORT")
        with mock.patch.object(VERIFIER.os, "kill", side_effect=failure), \
                mock.patch.object(VERIFIER, "descendant_process_stat") as diagnostic:
            report = {}
            with self.assertRaises(PermissionError) as raised:
                VERIFIER.require_descendants_gone(report, observed)
            self.assertIs(raised.exception, failure)
            self.assertEqual(report["observed_children"], list(observed.values()))
            self.assertEqual(report["descendant_shutdown_checks"][0]["liveness_error"], "permission_denied")
            self.assertNotIn("INERT-CANARY", json.dumps(report))
            diagnostic.assert_not_called()
        with mock.patch.object(VERIFIER.os, "kill", side_effect=ProcessLookupError()), \
                mock.patch.object(VERIFIER, "descendant_process_stat", return_value={
                    "status": "unavailable", "reason": "permission_denied"}):
            report = {}
            VERIFIER.require_descendants_gone(report, observed)
            self.assertIs(report["descendant_shutdown_checks"][0]["alive"], False)

    def test_descendant_stat_is_bounded_and_never_retains_command_text(self):
        fields = [b"S", b"40", b"41", b"42", *([b"0"] * 15), b"123456"]
        raw = b"101 (INERT-CANARY ) PRIVATE-COMMAND) " + b" ".join(fields) + b"\n"
        with mock.patch.object(VERIFIER.sys, "platform", "linux"), \
                mock.patch.object(VERIFIER.Path, "open", return_value=io.BytesIO(raw)) as opened:
            result = VERIFIER.descendant_process_stat(101)
            self.assertEqual(result, {"status": "observed", "state": "S", "ppid": 40,
                "pgid": 41, "session_id": 42, "start_ticks": 123456})
            opened.assert_called_once_with("rb")
            self.assertNotIn("CANARY", json.dumps(result))
            self.assertNotIn("COMMAND", json.dumps(result))
        for malformed in (raw.replace(b"101 (", b"102 (", 1), raw[:10], raw + b"x" * 4097,
                          raw.replace(b" S ", b" ? ", 1), raw.replace(b"123456", b"-1")):
            with self.subTest(malformed_length=len(malformed)), \
                    mock.patch.object(VERIFIER.sys, "platform", "linux"), \
                    mock.patch.object(VERIFIER.Path, "open", return_value=io.BytesIO(malformed)):
                self.assertEqual(VERIFIER.descendant_process_stat(101),
                    {"status": "unavailable", "reason": "invalid_stat"})
        with mock.patch.object(VERIFIER.sys, "platform", "linux"), \
                mock.patch.object(VERIFIER.Path, "open", side_effect=FileNotFoundError("INERT-CANARY")):
            self.assertEqual(VERIFIER.descendant_process_stat(101),
                {"status": "unavailable", "reason": "not_found"})


class GatewayAssemblyEvidenceTests(unittest.TestCase):
    def test_coherently_changed_story_cannot_replace_actual_tool_result(self):
        with inputs() as fixture:
            evidence = InertGatewayReport(fixture)
            success = evidence.report["cases"][0]
            acquisition = success["acquisition"]
            business = acquisition["business"]
            requests_path = Path(success["model_requests"]["path"])
            original_requests = requests_path.read_bytes()
            original_result = business["actual_tool_result"]
            business["story"] = {"id": 456, "title": "DIFFERENT INERT STORY - not returned by the tool"}
            business["answer"] = "Hacker News story 456: " + business["story"]["title"]
            acquisition["native_b"]["receipt"]["report"]["outcome"]["text"] = business["answer"]
            evidence.save_business_correlations()
            self.assertEqual(requests_path.read_bytes(), original_requests)
            self.assertEqual(business["actual_tool_result"], original_result)
            self.assertNotEqual(VERIFIER.installed_public_story(original_result), business["story"])
            with self.assertRaisesRegex(ValueError, "business story differs from the selected tool result"):
                evidence.assemble()
            self.assertFalse((fixture.root / "final-gateway").exists())

    def test_coherently_pinned_malformed_tool_result_is_refused(self):
        for content in ('[{"id":123,"id":123,"title":"INERT synthetic story"}]\n'
                        '{"value":"[{\\"id\\":123,\\"title\\":\\"INERT synthetic story\\"}]"}',
                        '[{"id":123,"title":"INERT synthetic story"}]'):
            with self.subTest(content=content), inputs() as fixture:
                evidence = InertGatewayReport(fixture)
                success = evidence.report["cases"][0]
                business = success["acquisition"]["business"]
                requests_path = Path(success["model_requests"]["path"])
                requests = json.loads(requests_path.read_bytes())
                request = requests[business["model_request_index"]]
                request["messages"][business["matching_result_history_index"]]["content"] = content
                business["actual_tool_result"] = content
                business["model_request_sha256"] = sha(VERIFIER.canonical(request))
                success["model_requests"] = evidence.document(requests_path, requests)
                evidence.save_business_correlations()
                with self.assertRaisesRegex(ValueError, "duplicate record key|expected actual Agno text"):
                    evidence.assemble()
                self.assertFalse((fixture.root / "final-gateway").exists())

    def test_story_decoder_preserves_existing_producer_contract(self):
        def pair(first, second=None):
            return first + "\n" + json.dumps({"value": first if second is None else second})
        ordinary = '[{"id":123,"title":"INERT synthetic story"}]'
        extended = '[{"id":18446744073709551615,"title":"  Caf\u00e9  ","score":1}]'
        for content in (pair(ordinary), " \n" + pair(extended) + "\t\r\n"):
            with self.subTest(valid=content):
                self.assertEqual(ASSEMBLER.gateway_public_story(content), VERIFIER.installed_public_story(content))
        invalid = {
            "not_text": None, "empty": "", "one_block": ordinary,
            "extra_block": pair(ordinary) + "\n{}", "malformed": pair(ordinary) + "x",
            "wrapper_extra_field": ordinary + "\n" + json.dumps({"value": ordinary, "extra": 1}),
            "wrapper_not_text": ordinary + "\n" + json.dumps({"value": [123]}),
            "duplicate_wrapper_field": ordinary + '\n{"value":' + json.dumps(ordinary) + ',"value":' + json.dumps(ordinary) + '}',
            "duplicate_text_field": pair('[{"id":123,"id":123,"title":"INERT synthetic story"}]', ordinary),
            "duplicate_structured_field": pair(ordinary, '[{"id":123,"id":123,"title":"INERT synthetic story"}]'),
            "different_stories": pair(ordinary, '[{"id":456,"title":"INERT synthetic story"}]'),
            "boolean_vs_integer": pair('[{"id":1,"title":"story"}]', '[{"id":true,"title":"story"}]'),
            "float_vs_integer": pair('[{"id":1,"title":"story"}]', '[{"id":1.0,"title":"story"}]'),
            "boolean_extra_field": pair('[{"id":1,"title":"story","score":true}]', '[{"id":1,"title":"story","score":1}]'),
            "boolean_id": pair('[{"id":true,"title":"story"}]'),
            "float_id": pair('[{"id":1.0,"title":"story"}]'),
            "zero_id": pair('[{"id":0,"title":"story"}]'),
            "negative_id": pair('[{"id":-1,"title":"story"}]'),
            "overflow_id": pair('[{"id":18446744073709551616,"title":"story"}]'),
            "missing_id": pair('[{"title":"story"}]'),
            "empty_title": pair('[{"id":1,"title":"  "}]'),
            "numeric_title": pair('[{"id":1,"title":123}]'),
            "no_stories": pair('[]'), "two_stories": pair('[{"id":1,"title":"story"},{"id":2,"title":"story"}]'),
            "not_an_object": pair('[123]'),
            "nonfinite_field": pair('[{"id":1,"title":"story","score":NaN}]'),
            "unpaired_surrogate": pair('[{"id":1,"title":"\\ud800"}]'),
        }
        for label, content in invalid.items():
            with self.subTest(invalid=label):
                with self.assertRaises((AssertionError, ValueError)):
                    VERIFIER.installed_public_story(content)
                with self.assertRaises(ValueError):
                    ASSEMBLER.gateway_public_story(content)

    def test_final_gateway_uses_current_report_shape_without_changing_staged_build(self):
        with inputs() as fixture:
            evidence = InertGatewayReport(fixture)
            original = fixture.record.read_bytes()
            selected = evidence.path.read_bytes()
            success, negative = evidence.report["cases"]
            self.assertNotIn("sentinel_connections", success)
            self.assertNotIn("sentinel_joined", success)
            self.assertNotIn("sentinel_joined", negative)
            business = json.loads(Path(success["original_business_observation"]["path"]).read_bytes())
            self.assertEqual(business["custody"]["retirement"], "not_observed")
            self.assertEqual(success["acquisition"]["custody"]["retirement"], "joined")
            result = evidence.assemble()
            destination = fixture.root / "final-gateway"
            self.assertEqual(fixture.record.read_bytes(), original)
            self.assertEqual((destination / "build-record.json").read_bytes(), original)
            self.assertEqual((destination / "gateway-verification.json").read_bytes(), selected)
            self.assertEqual(result["qualification"]["installed_gateway_verification_sha256"], sha(selected))
            self.assertEqual(result["scope"], "installed_gateway_schema4_resource_startup_and_shutdown_preview")
            self.assertEqual(result["archive"], "meerkat-toolkit-gateway-" + fixture.target + ".tar.gz")
            self.assertIn("model_execution", result["not_qualified"])
            self.assertIn("acquisition", result["not_qualified"])
            package = destination / fixture.prefix
            self.assertEqual({path.name for path in (package / "bin").iterdir()}, {"toolkit_gateway"})
            self.assertIn("bin/toolkit_gateway", (package / "README.md").read_text())
            self.assertNotIn("./" + fixture.prefix + "/bin/meerkat-connect", (package / "README.md").read_text())
            self.assertTrue((destination / result["archive"]).is_file())

    def test_startup_only_report_cannot_enable_gateway_finalization(self):
        with inputs() as fixture:
            evidence = InertGatewayReport(fixture)
            success = evidence.report["cases"][0]
            for key in ("acquisition", "model_requests", "model_request_count", "model_schedule_errors",
                        "service_parent_custody", "original_business_observation"):
                success.pop(key)
            evidence.save()
            with self.assertRaisesRegex(ValueError, "acquisition lacks original task completion"):
                evidence.assemble()
            self.assertFalse((fixture.root / "final-gateway").exists())

    def test_actual_provider_cleanup_is_required_for_both_cases(self):
        for index in (0, 1):
            for joined in (None, False):
                with self.subTest(case=index, joined=joined), inputs() as fixture:
                    evidence = InertGatewayReport(fixture)
                    case = evidence.report["cases"][index]
                    case["sentinel_joined"] = True
                    if joined is None:
                        case.pop("provider_joined")
                    else:
                        case["provider_joined"] = joined
                    evidence.save()
                    with self.assertRaisesRegex(ValueError, "joined cleanup"):
                        evidence.assemble()
                    self.assertFalse((fixture.root / "final-gateway").exists())

    def test_acquisition_evidence_pins_and_required_files_are_checked(self):
        for key in ("model_requests", "service_parent_custody", "original_business_observation"):
            for change in ("missing", "changed", "other_path"):
                with self.subTest(key=key, change=change), inputs() as fixture:
                    evidence = InertGatewayReport(fixture)
                    success = evidence.report["cases"][0]
                    if change == "missing":
                        success.pop(key)
                    elif change == "changed":
                        Path(success[key]["path"]).write_bytes(b"{}")
                    else:
                        success[key] = evidence.write(evidence.directory / (key + ".json"), Path(success[key]["path"]).read_bytes())
                    evidence.save()
                    with self.assertRaises(ValueError):
                        evidence.assemble()
                    self.assertFalse((fixture.root / "final-gateway").exists())

    def test_missing_or_mismatched_business_and_custody_are_refused(self):
        for change in ("completion", "business_record", "custody_record", "retirement", "retirement_observation",
                       "rule_owner", "rule_releases", "rule_exclusion", "custody_event", "fresh_source", "native_event",
                       "pending", "owner", "recipients", "terminal", "outcome", "session", "tool_calls", "answer",
                       "model_count", "model_errors", "model_digest", "model_index", "business_event"):
            with self.subTest(change=change), inputs() as fixture:
                evidence = InertGatewayReport(fixture)
                success = evidence.report["cases"][0]
                acquisition = success["acquisition"]
                custody = acquisition["custody"]
                receipt = acquisition["native_b"]["receipt"]["report"]
                events = evidence.events[success["case"]]
                if change == "completion": acquisition.pop("original_task_completion")
                elif change == "business_record": acquisition["business"]["answer"] = "different business record"
                elif change == "custody_record": custody["exchange_id"] = "another-exchange"
                elif change == "retirement": custody["retirement"] = "retained_uncertain"
                elif change == "retirement_observation": custody["retirement_observation"]["outcome"] = "failed"
                elif change == "rule_owner": custody["rule_owner_created"] = False
                elif change == "rule_releases": custody["rule_releases_after_joined"] = 2
                elif change == "rule_exclusion": custody["rule_exclusion_still_held"] = True
                elif change == "custody_event": next(row for row in events if row["event"] == "service_parent_custody_observed")["retirement"] = "not_observed"
                elif change == "fresh_source": acquisition["fresh_original"] = acquisition["original"]
                elif change == "native_event": next(row for row in events if row["event"] == "original_native_receipt_observed")["observation"] = {"state": "admission_unresolved"}
                elif change == "pending": receipt["status"] = "pending"
                elif change == "owner": receipt["owner_input_id"] = "another-input"
                elif change == "recipients": receipt["recipient_input_ids"].append("another-input")
                elif change == "terminal": receipt["terminal"] = {"outcome_type": "cancelled"}
                elif change == "outcome": receipt["outcome"]["completion_type"] = "failed"
                elif change == "session": acquisition["native_b"]["session_id"] = "another-session"
                elif change == "tool_calls": receipt["outcome"]["tool_calls"] = True
                elif change == "answer": receipt["outcome"]["text"] = "prepared but different answer"
                elif change == "model_count": success["model_request_count"] = 0
                elif change == "model_errors": success["model_schedule_errors"] = ["controlled model failed"]
                elif change == "model_digest": acquisition["business"]["model_request_sha256"] = "0" * 64
                elif change == "model_index": acquisition["business"]["model_request_index"] = -1
                elif change == "business_event": next(row for row in events if row["event"] == "original_public_answer_prepared")["answer"] = "another-answer"
                # Keep unrelated pins/correlations valid so semantic mutants
                # exercise completion/custody checks, not just hash mismatch.
                if change != "business_record":
                    record = copy.deepcopy(acquisition)
                    record["custody"] = json.loads(Path(success["original_business_observation"]["path"]).read_bytes())["custody"]
                    success["original_business_observation"] = evidence.document(Path(success["original_business_observation"]["path"]), record)
                if change != "custody_record":
                    success["service_parent_custody"] = evidence.document(Path(success["service_parent_custody"]["path"]), custody)
                if change != "native_event":
                    next(row for row in events if row["event"] == "original_native_receipt_observed")["observation"] = copy.deepcopy(acquisition["native_b"])
                if change not in ("custody_event", "custody_record"):
                    row = next(row for row in events if row["event"] == "service_parent_custody_observed")
                    row.update(copy.deepcopy(custody))
                evidence.save()
                with self.assertRaises(ValueError):
                    evidence.assemble()
                self.assertFalse((fixture.root / "final-gateway").exists())

    def test_selected_report_is_captured_once_before_use(self):
        with inputs() as fixture:
            evidence = InertGatewayReport(fixture)
            selected = evidence.path.read_bytes()
            capture = ASSEMBLER.gateway_capture
            calls = []
            def replace_after_capture(path):
                raw = capture(path)
                if path == evidence.path:
                    calls.append(path)
                    evidence.path.write_bytes(b'{"schema":1,"status":"failed"}')
                return raw
            with mock.patch.object(ASSEMBLER, "gateway_capture", side_effect=replace_after_capture):
                result = evidence.assemble(gateway_verification_sha=sha(selected))
            self.assertEqual(calls, [evidence.path])
            self.assertEqual((fixture.root / "final-gateway/gateway-verification.json").read_bytes(), selected)
            self.assertEqual(result["qualification"]["installed_gateway_verification_sha256"], sha(selected))

    def test_partial_wrong_scope_and_staging_reports_cannot_enable_gateway_finalization(self):
        for change in ("no_hash", "no_path", "bad_hash", "stage_only", "cli_scope", "staging_record", "cli_record"):
            with self.subTest(change=change), inputs() as fixture:
                evidence = InertGatewayReport(fixture)
                overrides = {"no_hash": {"gateway_verification_sha": None},
                    "no_path": {"gateway_verification_path": None}, "bad_hash": {"gateway_verification_sha": "0" * 64},
                    "stage_only": {"stage_only": True}, "cli_scope": {"binary_kind": "meerkat-connect"},
                    "staging_record": {"gateway_verification_path": fixture.destination / "staging.json",
                        "gateway_verification_sha": ASSEMBLER.digest(fixture.destination / "staging.json")},
                    "cli_record": {"gateway_verification_path": fixture.composition,
                        "gateway_verification_sha": ASSEMBLER.digest(fixture.composition)}}[change]
                with self.assertRaises(ValueError):
                    evidence.assemble(**overrides)
                self.assertFalse((fixture.root / "final-gateway").exists())

    def test_report_capture_rejects_links_special_files_duplicates_and_bounds(self):
        for change in ("symlink", "fifo", "duplicate", "bound"):
            with self.subTest(change=change), inputs() as fixture:
                evidence = InertGatewayReport(fixture)
                overrides = {}
                if change == "symlink":
                    retained = evidence.path.with_name("retained.json")
                    evidence.path.rename(retained)
                    evidence.path.symlink_to(retained)
                elif change == "fifo":
                    fifo = evidence.directory / "not-a-report"
                    os.mkfifo(fifo)
                    overrides.update(gateway_verification_path=fifo, gateway_verification_sha="0" * 64)
                elif change == "duplicate":
                    evidence.path.write_bytes(b'{"schema":1,' + evidence.path.read_bytes().lstrip()[1:])
                limit = 1 if change == "bound" else ASSEMBLER.GATEWAY_EVIDENCE_LIMIT
                with mock.patch.object(ASSEMBLER, "GATEWAY_EVIDENCE_LIMIT", limit):
                    with self.assertRaises(ValueError): evidence.assemble(**overrides)
                self.assertFalse((fixture.root / "final-gateway").exists())

    def test_candidate_source_and_exact_evidence_mismatches_are_refused(self):
        for change in ("candidate", "verifier_source", "sdk_source", "sdk_set", "resource_build",
                       "completed_manifest", "result_file", "catalog_file", "diagnostic_file"):
            with self.subTest(change=change), inputs() as fixture:
                evidence = InertGatewayReport(fixture)
                report = evidence.report
                success, negative = report["cases"]
                if change == "candidate": report["installed_candidate"]["binary"]["sha256"] = "0" * 64
                elif change == "verifier_source": report["script_sha256"] = "0" * 64
                elif change == "sdk_source": Path(report["sdk_sources"][0]["path"]).write_bytes(b"changed source")
                elif change == "sdk_set": report["sdk_sources"][-1] = report["sdk_sources"][0]
                elif change == "resource_build": report["installed_inputs"]["resource_build"]["sha256"] = "0" * 64
                elif change == "completed_manifest": success["finalization"]["completed_manifest"]["sha256"] = "0" * 64
                elif change == "catalog_file": Path(success["model_catalog"]["path"]).write_bytes(b"changed catalog")
                elif change == "diagnostic_file": Path(negative["gateway_stderr"]["path"]).write_bytes(b"unrelated failure")
                evidence.save()
                if change == "result_file":
                    (evidence.directory / success["case"] / "result.json").write_bytes(b"{}")
                with self.assertRaises(ValueError): evidence.assemble()
                self.assertFalse((fixture.root / "final-gateway").exists())

    def test_unbound_or_forced_observations_do_not_count_as_success(self):
        for change in ("ready", "init_plan", "persistent_state", "catalog_reply", "command_pid", "command_argv",
                       "publication", "shutdown_reply", "forced", "readers", "sentinel", "negative_child",
                       "negative_publication", "negative_prepare", "negative_marker", "negative_before"):
            with self.subTest(change=change), inputs() as fixture:
                evidence = InertGatewayReport(fixture)
                success, negative = evidence.report["cases"]
                events = evidence.events[success["case"]]
                init = next(row["message"] for row in events if row.get("event") == "sdk_wire_out"
                            and row["message"]["method"] == "meta/init")
                if change == "ready": success["native_ready"] = {"init_id": "another", "outcome": "ready"}
                elif change == "init_plan": init["params"]["prepared_application"]["bundle_base64"] = base64.b64encode(b"{}").decode()
                elif change == "persistent_state": init["params"]["persistent_state"] = "/wrong/state"
                elif change == "catalog_reply": next(row["message"] for row in events if row.get("event") == "sdk_wire_response"
                    and row["message"]["id"] == "catalog-request")["result"] = {"different": True}
                elif change == "command_pid": success["finalization"]["prepare-policy"]["pid"] += 1
                elif change == "command_argv": success["finalization"]["prepare-policy"]["argv"].append("EXTRA")
                elif change == "publication": next(row for row in events if row["event"] == "fixture_publish_entered")["selection"] = {}
                elif change == "shutdown_reply": success["shutdown_response"] = {"id": "wrong", "result": {}}
                elif change == "forced": success["termination"]["escalation"] = "sigterm"
                elif change == "readers": success["sdk_readers_joined"] = False
                elif change == "sentinel": negative["sentinel_connections"] = [{"observed": True}]
                elif change == "negative_child": evidence.events[negative["case"]][0]["binary"] = str(evidence.package / "bin/toolkit_gateway")
                elif change == "negative_publication": evidence.events[negative["case"]].append({"event": "fixture_published"})
                elif change == "negative_prepare": Path(negative["preparation_plan"]["path"]).parent.joinpath("application-preparation").mkdir()
                elif change == "negative_marker": Path(negative["resource_corruption"]["marker"]).write_text("entered")
                elif change == "negative_before": negative["resource_corruption"]["before"] = negative["resource_corruption"]["after"]
                evidence.save()
                with self.assertRaises(ValueError): evidence.assemble()
                self.assertFalse((fixture.root / "final-gateway").exists())


class AssemblyTests(unittest.TestCase):
    def check_construction(self, fixture, *, default_target=False):
        result = fixture.assemble(default_target=default_target)
        second = fixture.assemble(destination=fixture.root / "second-inert")
        self.assertEqual(result["archive_sha256"], second["archive_sha256"])
        self.assertEqual(result["target"], fixture.target)
        self.assertEqual(result["archive"], "meerkat-connect-" + fixture.target + ".tar.gz")
        self.assertFalse(result["qualification"]["cli_tty_qualified"])
        self.assertIn("cli_terminal_interaction", result["not_qualified"])
        self.assertIn("account_sign_in", result["not_qualified"])
        self.assertIn("other_targets", result["not_qualified"])
        self.assertEqual(result["resources_sha256"], fixture.resource_sha)
        self.assertEqual(result["resource_component_sha256"], fixture.selected["component_sha256"])
        self.assertEqual((fixture.destination / "build-record.json").read_bytes(), fixture.record.read_bytes())
        package = fixture.destination / fixture.prefix
        manifest, digest = PREPARE["verify_resources"](package / "libexec/meerkat-connect")
        self.assertEqual(digest, fixture.resource_sha)
        self.assertEqual(manifest["entries"], fixture.original_entries)
        readme = (package / "README.md").read_text()
        self.assertIn("tar -xpf " + result["archive"], readme)
        self.assertIn("./" + fixture.prefix + "/bin/meerkat-connect", readme)
        self.assertIn("It does not qualify", readme)
        self.assertNotIn("does not yet provide", readme)
        with tarfile.open(fixture.destination / result["archive"]) as archive:
            members = archive.getmembers()
            self.assertEqual(len(members), len({member.name for member in members}))
            self.assertTrue(all(member.name == fixture.prefix or member.name.startswith(fixture.prefix + "/")
                                for member in members))
            self.assertTrue(all((member.uid, member.gid, member.uname, member.gname, member.mtime)
                                == (0, 0, "", "", 0) for member in members))
            supplied = fixture.prefix + "/bin/meerkat-connect"
            self.assertEqual(archive.extractfile(supplied).read(), fixture.binary.read_bytes())
            self.assertEqual(archive.getmember(supplied).mode, 0o755)
            for name in ("README.md", *fixture.licenses):
                self.assertEqual(archive.getmember(fixture.prefix + "/" + name).mode, 0o444)
            resource_prefix = fixture.prefix + "/libexec/meerkat-connect/"
            for entry in fixture.original_entries:
                member = archive.getmember(resource_prefix + entry["path"])
                self.assertEqual(member.mode, entry["mode"])
                if entry["kind"] == "symlink":
                    self.assertTrue(member.issym())
                    self.assertEqual(member.linkname, entry["target"])
        self.assertFalse((fixture.destination / (result["archive"] + ".partial")).exists())

    def test_mac_default_builds_only_deterministic_inert_archive(self):
        with inputs() as fixture:
            previous = os.umask(0o077)
            try:
                self.check_construction(fixture, default_target=True)
            finally:
                os.umask(previous)

    def test_linux_builds_only_deterministic_inert_archive_on_sensitive_storage(self):
        with inputs("linux-x86_64") as fixture:
            try:
                PACKAGER["require_case_sensitive"](fixture.root)
            except ValueError:
                with self.assertRaisesRegex(ValueError, "case-sensitive staging filesystem"):
                    fixture.assemble()
                self.assertFalse(fixture.destination.exists())
                self.skipTest("Linux construction needs an actual case-sensitive test volume")
            self.check_construction(fixture)

    def test_shutdown_gate_binds_candidate_and_evidence_before_creating_output(self):
        for change in ("missing", "refused", "target", "source", "binary", "resource", "evidence"):
            with self.subTest(change=change), inputs() as fixture:
                value = fixture.shutdown_value
                if change == "missing":
                    fixture.shutdown.unlink()
                else:
                    if change == "refused":
                        value["disposition"] = "pending"
                    elif change == "target":
                        value["candidate"]["target"] = "linux-x86_64"
                    elif change == "source":
                        value["candidate"]["source_manifest_sha256"] = "0" * 64
                    elif change == "binary":
                        value["candidate"]["companion_cli_binary"]["sha256"] = "0" * 64
                    elif change == "resource":
                        value["candidate"]["resources_sha256"] = "0" * 64
                    else:
                        value["evidence"][0]["sha256"] = "0" * 64
                    fixture.shutdown.write_bytes(encoded(value))
                    fixture.update_record()
                with self.assertRaises((ValueError, FileNotFoundError)):
                    fixture.assemble()
                self.assertFalse(fixture.destination.exists())

    def test_existing_composition_scope_cannot_be_replaced_by_ordinary_cli_or_another_build(self):
        for change in ("scope", "candidate", "copied_test", "build_target", "embedded_build_pin"):
            with self.subTest(change=change), inputs() as fixture:
                if change == "scope":
                    fixture.composition_value["scope"] = "actual_cli_test"
                elif change == "candidate":
                    fixture.composition_value["candidate"]["source_manifest_sha256"] = "0" * 64
                elif change == "copied_test":
                    fixture.installed_test.write_bytes(b"wrong copied executable")
                elif change == "build_target":
                    fixture.record_value["target"] = "linux-x86_64"
                else:
                    fixture.record_value["cargo"]["env"]["MEERKAT_CONNECT_RESOURCES_SHA256"] = "0" * 64
                fixture.composition.write_bytes(encoded(fixture.composition_value))
                fixture.update_record()
                with self.assertRaises(ValueError):
                    fixture.assemble()
                self.assertFalse(fixture.destination.exists())

    def test_manifest_target_must_match_even_with_valid_structure_and_matching_digest(self):
        with inputs(manifest_target="linux-x86_64") as fixture:
            with self.assertRaisesRegex(ValueError, "resource manifest or root modes changed"):
                fixture.assemble()
            self.assertTrue(fixture.destination.exists())  # Partial extraction is retained.
            self.assertFalse((fixture.destination / "assembly.json").exists())
            self.assertFalse(list(fixture.destination.glob("*.tar.gz")))
            self.assertFalse((fixture.destination / fixture.prefix / "bin/meerkat-connect").exists())

    def test_changed_component_and_symlink_binary_refuse_before_output(self):
        for change in ("component", "binary_symlink", "destination_symlink", "unknown_target"):
            with self.subTest(change=change), inputs() as fixture:
                if change == "component":
                    with fixture.component.open("ab") as stream:
                        stream.write(b"tampered")
                elif change == "binary_symlink":
                    real = fixture.binary.with_name("inert-real-cli")
                    fixture.binary.rename(real)
                    fixture.binary.symlink_to(real.name)
                elif change == "destination_symlink":
                    fixture.destination.symlink_to(fixture.root / "absent")
                else:
                    with self.assertRaisesRegex(ValueError, "unsupported assembly target"):
                        ASSEMBLER.assemble(fixture.binary, ASSEMBLER.digest(fixture.binary), fixture.record,
                                           fixture.destination, target="unsupported")
                    self.assertFalse(fixture.destination.exists())
                    continue
                with self.assertRaises(ValueError):
                    fixture.assemble()
                self.assertFalse(fixture.destination.exists())


class ExplicitSelectionTests(unittest.TestCase):
    def test_relocated_resources_keep_the_original_record_and_archive_binding(self):
        for target in ASSEMBLER.TARGETS:
            for kind in ASSEMBLER.BINARY_KINDS:
                with self.subTest(target=target, kind=kind), inputs(target) as fixture:
                    fixture.select_explicit(kind)
                    original = fixture.resource_record.read_bytes()
                    original_digest = sha(original)
                    original_directory = fixture.resource_record.parent
                    delivered = fixture.root / "delivered"
                    original_directory.rename(delivered)
                    fixture.resource_record = delivered / "build.json"
                    fixture.component = delivered / fixture.component.name
                    self.assertFalse(original_directory.exists())

                    selected, raw, _ = ASSEMBLER.selected_resources(
                        target, fixture.resource_record, original_digest, kind)
                    self.assertEqual(raw, original)
                    self.assertEqual(selected["component"], fixture.component)
                    with ASSEMBLER.checked_component(selected, target) as (_, entries):
                        self.assertIn(fixture.prefix + "/libexec/meerkat-connect/resources.json", entries)

                    if target == "macos-arm64":
                        result = fixture.assemble(stage_only=True)
                        self.assertEqual(result["candidate"]["resource_build_record_sha256"], original_digest)
                        self.assertEqual((fixture.destination / "resource-build-record.json").read_bytes(), original)

    def test_relocated_archive_still_rejects_different_bytes_before_staging(self):
        with inputs() as fixture:
            fixture.select_explicit("toolkit_gateway")
            delivered = fixture.root / "delivered"
            fixture.resource_record.parent.rename(delivered)
            fixture.resource_record = delivered / "build.json"
            fixture.component = delivered / fixture.component.name
            changed = bytearray(fixture.component.read_bytes())
            changed[-1] ^= 1
            fixture.component.write_bytes(changed)
            with self.assertRaisesRegex(ValueError, "component archive pin mismatch"):
                fixture.assemble(stage_only=True)
            self.assertFalse(fixture.destination.exists())

    def test_each_fixed_binary_kind_stages_exact_resources_without_an_archive_or_acceptance(self):
        for target, kind in ((target, kind) for target in ASSEMBLER.TARGETS for kind in ASSEMBLER.BINARY_KINDS):
            with self.subTest(target=target, kind=kind), inputs(target) as fixture:
                fixture.select_explicit(kind)
                if target == "linux-x86_64":
                    try:
                        PACKAGER["require_case_sensitive"](fixture.root)
                    except ValueError:
                        with self.assertRaisesRegex(ValueError, "case-sensitive staging filesystem"):
                            fixture.assemble(stage_only=True)
                        self.assertFalse(fixture.destination.exists())
                        continue
                if kind == "toolkit_gateway":
                    for field in ("test_executable", "installed_composition", "native_shutdown_review"):
                        del fixture.record_value[field]
                    fixture.record.write_bytes(encoded(fixture.record_value))
                result = fixture.assemble(stage_only=True)
                package = fixture.destination / fixture.prefix
                self.assertEqual({path.name for path in (package / "bin").iterdir()}, {kind})
                self.assertEqual((package / "bin" / kind).read_bytes(), fixture.binary.read_bytes())
                self.assertEqual(result["scope"], "unqualified_product_prefix_for_verification")
                self.assertEqual(result["candidate"]["binary_kind"], kind)
                self.assertEqual(result["candidate"]["source_manifest_sha256"], ASSEMBLER.digest(fixture.source))
                self.assertEqual(result["candidate"]["resource_build_record_sha256"],
                                 ASSEMBLER.digest(fixture.resource_record))
                self.assertNotIn("qualification", result)
                if kind == "toolkit_gateway":
                    self.assertNotIn("test_executable", result["candidate"])
                self.assertNotIn("archive", result)
                self.assertFalse((fixture.destination / "assembly.json").exists())
                self.assertFalse(list(fixture.destination.glob("*.tar.gz*")))
                self.assertEqual((fixture.destination / "resource-build-record.json").read_bytes(),
                                 fixture.resource_record.read_bytes())
                self.assertEqual(PREPARE["verify_resources"](package / "libexec/meerkat-connect")[1],
                                 fixture.resource_sha)

    def test_explicit_cli_final_archive_requires_its_exact_execution_and_review_candidate(self):
        with inputs() as fixture:
            fixture.select_explicit()
            result = fixture.assemble()
            self.assertEqual(result["binary_kind"], "meerkat-connect")
            self.assertEqual(result["qualification"]["candidate"]["resource_build_record_sha256"],
                             ASSEMBLER.digest(fixture.resource_record))
            self.assertFalse(result["qualification"]["cli_tty_qualified"])
            self.assertTrue((fixture.destination / result["archive"]).is_file())

    def test_gateway_staging_and_relabelled_cli_records_never_enable_final_assembly(self):
        with inputs() as fixture:
            fixture.select_explicit("toolkit_gateway")
            staged = fixture.assemble(stage_only=True)
            # The synthetic existing CLI evidence already names the gateway
            # candidate. Relabelling and actual prefix staging confer no proof.
            for change in ("cli_evidence", "staging_metadata"):
                with self.subTest(change=change):
                    if change == "staging_metadata":
                        fixture.record_value["installed_composition"] = fixture.reference(fixture.destination / "staging.json")
                        fixture.record.write_bytes(encoded(fixture.record_value))
                    destination = fixture.root / ("final-" + change)
                    with self.assertRaisesRegex(ValueError, "final gateway qualification is not available"):
                        fixture.assemble(destination=destination)
                    self.assertFalse(destination.exists())
            self.assertEqual(staged["scope"], "unqualified_product_prefix_for_verification")

    def test_partial_or_invalid_explicit_arguments_never_fall_back_to_legacy(self):
        for change in ("record", "digest", "kind", "unknown_kind", "bad_digest", "wrong_hash", "no_explicit_stage"):
            with self.subTest(change=change), inputs() as fixture:
                fixture.select_explicit()
                overrides = {"record": {"resource_record_path": None},
                             "digest": {"resource_record_sha": None},
                             "kind": {"binary_kind": None},
                             "unknown_kind": {"binary_kind": "gateway"},
                             "bad_digest": {"resource_record_sha": "not-a-digest"},
                             "wrong_hash": {"resource_record_sha": "0" * 64},
                             "no_explicit_stage": {"resource_record_path": None,
                                                   "resource_record_sha": None, "binary_kind": None}}[change]
                with self.assertRaises(ValueError):
                    fixture.assemble(stage_only=True, **overrides)
                self.assertFalse(fixture.destination.exists())

    def test_selected_record_and_archive_must_be_regular_non_symlink_files(self):
        for selected in ("record", "archive"):
            with self.subTest(selected=selected), inputs() as fixture:
                fixture.select_explicit("toolkit_gateway")
                path = fixture.resource_record if selected == "record" else fixture.component
                real = path.with_name("retained-" + path.name)
                path.rename(real)
                path.symlink_to(real.name)
                with self.assertRaises(ValueError):
                    fixture.assemble(stage_only=True)
                self.assertFalse(fixture.destination.exists())

    def test_resource_record_target_layout_provenance_and_source_pins_are_checked(self):
        for change in ("target", "archive_path", "resource_path", "relative_layout", "parent_traversal",
                       "archive_sha", "archive_bytes",
                       "prepare", "packager", "kind", "manifest", "starter_file", "duplicate_key"):
            with self.subTest(change=change), inputs() as fixture:
                fixture.select_explicit("toolkit_gateway")
                value = fixture.resource_value
                if change == "target":
                    value["target"] = "linux-x86_64"
                elif change == "archive_path":
                    value["archive"] = str(fixture.root / fixture.component.name)
                elif change == "resource_path":
                    value["resources"] = str(fixture.root)
                elif change in ("relative_layout", "parent_traversal"):
                    parent = "build" if change == "relative_layout" else "/build/../other"
                    value["archive"] = parent + "/" + fixture.component.name
                    value["resources"] = parent + "/" + fixture.prefix + "/libexec/meerkat-connect"
                elif change == "archive_sha":
                    value["archive_sha256"] = "0" * 64
                elif change == "archive_bytes":
                    value["archive_bytes"] = True
                elif change in ("prepare", "packager"):
                    value[change + "_source_sha256"] = "0" * 64
                elif change == "kind":
                    value["input_selection"]["kind"] = "implicit_legacy_defaults"
                elif change == "manifest":
                    value["input_selection"]["starter"]["manifest_sha256"] = "0" * 64
                elif change == "starter_file":
                    del value["input_selection"]["starter"]["files"]["start.py"]
                raw = encoded(value)
                if change == "duplicate_key":
                    raw = raw[:-2] + b', "target": "macos-arm64"}\n'
                fixture.resource_record.write_bytes(raw)
                fixture.record_value["resource_build_record_sha256"] = sha(raw)
                fixture.update_record()
                with self.assertRaises(ValueError):
                    fixture.assemble(stage_only=True)
                self.assertFalse(fixture.destination.exists())

    def test_build_candidate_source_target_binary_and_resource_binding_are_checked_before_staging(self):
        for change in ("kind", "resource", "target", "source", "binary", "argv", "build_env"):
            with self.subTest(change=change), inputs() as fixture:
                fixture.select_explicit("toolkit_gateway")
                value = fixture.record_value
                if change == "kind":
                    value["binary_kind"] = "meerkat-connect"
                elif change == "resource":
                    value["resource_build_record_sha256"] = "0" * 64
                elif change == "target":
                    value["target"] = "linux-x86_64"
                elif change == "source":
                    value["source_manifest"]["sha256"] = "0" * 64
                elif change == "binary":
                    value["binary"]["sha256"] = "0" * 64
                elif change == "argv":
                    value["cargo"]["argv"][-1] = "meerkat-connect"
                else:
                    value["cargo"]["env"]["MEERKAT_CONNECT_RESOURCES_SHA256"] = "0" * 64
                fixture.record.write_bytes(encoded(value))
                with self.assertRaises(ValueError):
                    fixture.assemble(stage_only=True)
                self.assertFalse(fixture.destination.exists())

    def test_selected_library_and_starter_payloads_cannot_be_swapped_under_same_resource_record(self):
        for change in ("library", "starter"):
            with self.subTest(change=change), inputs() as fixture:
                fixture.select_explicit("toolkit_gateway")
                selected = fixture.resource_value["input_selection"]
                if change == "library":
                    selected["library"]["sha256"] = "0" * 64
                else:
                    selected["starter"]["files"]["start.py"]["sha256"] = "0" * 64
                fixture.repin_resource_record()
                with self.assertRaisesRegex(ValueError, "input pin mismatch"):
                    fixture.assemble(stage_only=True)
                self.assertTrue(fixture.destination.exists())
                self.assertFalse((fixture.destination / "staging.json").exists())
                self.assertFalse(list((fixture.destination / fixture.prefix / "bin").iterdir()))


class ComponentConfinementTests(unittest.TestCase):
    def rewrite(self, fixture, appended):
        with tarfile.open(fixture.component) as source:
            rows = [(member, source.extractfile(member).read() if member.isfile() else None)
                    for member in source]
        with tarfile.open(fixture.component, "w:gz") as output:
            for member, raw in rows + appended:
                output.addfile(member, io.BytesIO(raw) if raw is not None else None)
        fixture.repin_resource_record()

    def test_traversal_target_collision_special_and_link_entries_refuse_before_any_output(self):
        for attack in ("absolute", "parent", "target", "duplicate", "case_collision", "hardlink",
                       "device", "absolute_link", "escaping_link", "dangling_link", "cycle", "link_parent"):
            with self.subTest(attack=attack), inputs() as fixture:
                fixture.select_explicit("toolkit_gateway")
                resource = fixture.prefix + "/libexec/meerkat-connect"
                path = resource + "/bad"
                if attack == "absolute": path = "/escaped-component"
                elif attack == "parent": path = fixture.prefix + "/../escaped-component"
                elif attack == "target": path = "meerkat-connect-linux-x86_64/libexec/foreign"
                elif attack == "duplicate": path = resource + "/prepare.py"
                elif attack == "case_collision": path = resource + "/PREPARE.py"
                member = tarfile.TarInfo(path)
                member.mode = 0o444
                extra = []
                if attack == "hardlink":
                    member.type, member.linkname = tarfile.LNKTYPE, resource + "/prepare.py"
                elif attack == "device":
                    member.type = tarfile.CHRTYPE
                elif attack in ("absolute_link", "escaping_link", "dangling_link", "cycle", "link_parent"):
                    member.type, member.mode = tarfile.SYMTYPE, 0o777
                    member.linkname = {"absolute_link": "/outside", "escaping_link": "../../../outside",
                                       "dangling_link": "absent", "cycle": "bad", "link_parent": "runtime"}[attack]
                    if attack == "link_parent":
                        child = tarfile.TarInfo(path + "/child")
                        child.mode = 0o444
                        extra.append((child, b""))
                self.rewrite(fixture, [(member, b"" if member.isfile() else None), *extra])
                with self.assertRaises(ValueError):
                    fixture.assemble(stage_only=True)
                self.assertFalse(fixture.destination.exists())
                self.assertFalse((fixture.root / "escaped-component").exists())

    def test_archive_and_decompression_limits_apply_even_to_correctly_pinned_inputs(self):
        for bound in ("COMPONENT_LIMIT", "UNPACKED_LIMIT", "ENTRY_LIMIT"):
            with self.subTest(bound=bound), inputs() as fixture:
                fixture.select_explicit("toolkit_gateway")
                with mock.patch.object(ASSEMBLER, bound, 1), self.assertRaises(ValueError):
                    fixture.assemble(stage_only=True)
                self.assertFalse(fixture.destination.exists())

    def test_path_replacement_at_decompression_cannot_replace_the_pinned_capture(self):
        with inputs() as fixture:
            fixture.select_explicit("toolkit_gateway")
            original = fixture.component.read_bytes()
            replacement = io.BytesIO()
            with tarfile.open(fileobj=io.BytesIO(original), mode="r:gz") as source, \
                    tarfile.open(fileobj=replacement, mode="w:gz") as output:
                for member in source:
                    output.addfile(member, source.extractfile(member) if member.isfile() else None)
                extra = tarfile.TarInfo(fixture.prefix + "/libexec/meerkat-connect/unselected")
                extra.mode = 0o444
                output.addfile(extra)
            selected_gzip = ASSEMBLER.gzip.GzipFile
            observations = []

            @contextlib.contextmanager
            def replace_path_during_use(*args, **kwargs):
                fixture.component.write_bytes(replacement.getvalue())
                observations.append(ASSEMBLER.digest(fixture.component))
                try:
                    with selected_gzip(*args, **kwargs) as stream:
                        yield stream
                finally:
                    fixture.component.write_bytes(original)

            with mock.patch.object(ASSEMBLER.gzip, "GzipFile", side_effect=replace_path_during_use):
                result = fixture.assemble(stage_only=True)
            self.assertEqual(observations, [sha(replacement.getvalue())])
            self.assertNotEqual(observations[0], sha(original))
            self.assertEqual(fixture.component.read_bytes(), original)
            resources = fixture.destination / fixture.prefix / "libexec/meerkat-connect"
            self.assertFalse((resources / "unselected").exists())
            self.assertEqual(PREPARE["tree_entries"](resources), fixture.original_entries)
            self.assertEqual(result["resource_component_sha256"], sha(original))

    def test_selected_starter_uses_the_producers_two_mebibyte_file_bound(self):
        for size in (2 * 1024 * 1024, 2 * 1024 * 1024 + 1):
            with self.subTest(size=size), inputs() as fixture:
                fixture.select_explicit("toolkit_gateway", starter_readme=b"S" * size)
                if size == 2 * 1024 * 1024:
                    fixture.assemble(stage_only=True)
                    copied = fixture.destination / fixture.prefix / "libexec/meerkat-connect/starter/README.md"
                    self.assertEqual(copied.read_bytes(), b"S" * size)
                else:
                    with self.assertRaisesRegex(ValueError, "invalid selected starter file"):
                        fixture.assemble(stage_only=True)
                    self.assertFalse(fixture.destination.exists())

    def test_safe_relative_symlinks_remain_preserved(self):
        with inputs() as fixture:
            fixture.select_explicit("toolkit_gateway")
            fixture.assemble(stage_only=True)
            link = fixture.destination / fixture.prefix / "libexec/meerkat-connect/runtime/node/bin/node-link"
            self.assertTrue(link.is_symlink())
            self.assertEqual(os.readlink(link), "node")
            self.assertTrue(link.resolve(strict=True).is_relative_to(fixture.destination / fixture.prefix))


class ExecutableHeaderTests(unittest.TestCase):
    def test_both_headers_and_linux_exec_or_pie_are_checked_without_execution(self):
        with tempfile.TemporaryDirectory(prefix="inert-connect-headers-") as scratch:
            path = Path(scratch).resolve() / "NEVER-EXECUTE"
            for target in ASSEMBLER.TARGETS:
                for kind in (2, 3):
                    with self.subTest(target=target, kind=kind):
                        pin = ASSEMBLER.TARGETS[target]["resources_sha256"]
                        path.write_bytes(binary_bytes(target, pin, elf_kind=kind))
                        ASSEMBLER.check_binary(path, ASSEMBLER.digest(path), target=target)
                        other = next(value for value in ASSEMBLER.TARGETS if value != target)
                        with self.assertRaises(ValueError):
                            ASSEMBLER.check_binary(path, ASSEMBLER.digest(path), target=other)

    def test_wrong_elf_class_endianness_arch_kind_entry_size_and_digest_are_refused(self):
        with tempfile.TemporaryDirectory(prefix="inert-connect-headers-") as scratch:
            path = Path(scratch).resolve() / "NEVER-EXECUTE"
            pin = ASSEMBLER.TARGETS["linux-x86_64"]["resources_sha256"]
            original = binary_bytes("linux-x86_64", pin)
            for offset, replacement in ((4, b"\x01"), (5, b"\x02"), (6, b"\x00"),
                                        (16, b"\x01\x00"), (18, b"\xb7\x00"),
                                        (20, b"\x00" * 4), (24, b"\x00" * 8),
                                        (52, b"\x20\x00"), (len(original) - 64, b"0" * 64)):
                with self.subTest(offset=offset):
                    changed = bytearray(original)
                    changed[offset:offset + len(replacement)] = replacement
                    path.write_bytes(changed)
                    with self.assertRaises(ValueError):
                        ASSEMBLER.check_binary(path, ASSEMBLER.digest(path), target="linux-x86_64")


class InstalledConsoleBoundaryTests(unittest.TestCase):
    """Pure fixture/parser controls, never native or browser acceptance."""
    @classmethod
    def setUpClass(cls):
        cls.console = load("connect_installed_console_tests",
                           ROOT / "scripts/verify_installed_console.py")

    def test_first_console_transport_records_refusal_and_retires_login(self):
        from dataclasses import dataclass
        from types import SimpleNamespace
        c = self.console

        @dataclass
        class Retirement:
            fenced: bool = True
            reservations_drained: bool = True
            uncertain: bool = False

        class StopAfterFirstTransport(RuntimeError):
            pass

        # Run the actual nested request path without an event loop's self-pipe
        # or any worker/socket. The parent and transport are inert test doubles.
        async def immediate(call, *args, **kwargs):
            return call(*args, **kwargs)

        enrollment = SimpleNamespace(enrollment_id=17, result=mock.Mock(),
            retire=mock.Mock(return_value=Retirement()))
        private = mock.Mock()
        private.begin_console_login.return_value = enrollment
        response = SimpleNamespace(status=401, read=mock.Mock(
            return_value=b'{"error":"Protected setup is unavailable"}'))
        connection = mock.Mock()
        connection.getresponse.return_value = response
        factory = mock.Mock(side_effect=[connection,
            StopAfterFirstTransport("stop before authenticated native request")])
        process, provider, events = mock.Mock(), mock.Mock(), mock.Mock()
        process.poll.return_value = None
        sdk = SimpleNamespace(ConsoleLoginSelection=lambda **values: values)
        with tempfile.TemporaryDirectory(prefix="inert-console-first-transport-") as scratch:
            directory = Path(scratch)
            scenario = c.ConsoleScenario(VERIFIER, "http://127.0.0.1:18444")
            # create=True lets this same behavioral control reach the original
            # shadowed http.client call when checking the unchanged predecessor.
            with mock.patch.object(c, "HTTPConnection", factory, create=True), \
                    mock.patch.object(c.asyncio, "to_thread", immediate), \
                    mock.patch.object(VERIFIER, "SDK", sdk, create=True):
                work = scenario.run(private, provider, directory, events, process, scenario.origin)
                try:
                    with self.assertRaisesRegex(StopAfterFirstTransport,
                            "stop before authenticated native request"):
                        work.send(None)
                finally:
                    work.close()
            report = json.loads((directory / "console-journey.json").read_bytes())
        self.assertEqual(factory.call_count, 2)
        self.assertEqual(factory.call_args_list[0].args, ("127.0.0.1", 18444))
        connection.request.assert_called_once_with("GET", c.PANEL + "/data", body=None,
                                                  headers={"Accept": "application/json"})
        connection.getresponse.assert_called_once_with()
        response.read.assert_called_once_with(4 * 1024 * 1024 + 1)
        connection.close.assert_called_once_with()
        enrollment.result.assert_called_once_with()
        enrollment.retire.assert_called_once_with(timeout=10.0)
        self.assertEqual(report["http"], [{"method": "GET", "path": c.PANEL + "/data",
            "request": None, "status": 401, "response": {"error": "Protected setup is unavailable"}}])
        self.assertEqual(report["status"], "failed")
        self.assertTrue(report["enrollment"]["acknowledged"])
        self.assertEqual(report["login_retirement"],
                         {"fenced": True, "reservations_drained": True, "uncertain": False})
        self.assertEqual((report["starts"], report["decisions"], report["timeline"]), ([], [], []))
        self.assertEqual(provider.mock_calls, [])
        events.add.assert_not_called()

    def test_human_plan_preserves_native_authorities_without_service_policy(self):
        with tempfile.TemporaryDirectory(prefix="inert-console-plan-") as scratch:
            root = Path(scratch).resolve()
            service_root, console_root = root / "service", root / "console"
            service_root.mkdir(); console_root.mkdir()
            scenario = self.console.ConsoleScenario(VERIFIER, "http://127.0.0.1:18444")
            with mock.patch.object(VERIFIER.subprocess, "Popen", side_effect=AssertionError("no child")), \
                    mock.patch.object(self.console.socket, "socket", side_effect=AssertionError("no network")):
                service, _, service_source = VERIFIER.installed_plan(service_root, [], acquisition=True)
                selected, _, selected_source = VERIFIER.installed_plan(console_root, [], acquisition=True, console=scenario)
            self.assertEqual(service["owner"]["native_host"]["routes"][0]["origin"]["kind"], "provider_event")
            self.assertEqual(service["acquisition"]["standing_policy"],
                             {"installation": "checked_software", "access": "provision_task_parent"})
            self.assertIsNone(service["acquisition"]["console_setup"])
            self.assertNotIn("standing_policy", selected["acquisition"])
            self.assertEqual(selected["acquisition"]["console_setup"], {"origin": scenario.origin})
            self.assertEqual(selected["authorities"], service["authorities"])
            self.assertEqual(selected["application"]["helper"], service["application"]["helper"])
            service_definition = tomllib.loads(service["application"]["definition"]["text"])
            console_definition = tomllib.loads(selected["application"]["definition"]["text"])
            for definition in (service_definition, console_definition):
                self.assertIs(definition["profiles"]["original"].get("external_addressable"), True)
                self.assertEqual({name for name, profile in definition["profiles"].items()
                                  if profile.get("external_addressable", False)}, {"original"})
                self.assertIs(definition["profiles"]["research"].get("external_addressable", False), False)
            self.assertEqual(console_definition, service_definition)
            self.assertEqual(selected["application"]["definition"]["format"],
                             service["application"]["definition"]["format"])
            self.assertEqual(selected["application"]["tool_policy"]["members"],
                             service["application"]["tool_policy"]["members"])
            native = selected["owner"]["native_host"]
            self.assertEqual(selected["owner"]["components"], [])
            self.assertEqual(selected["owner"]["channels"], [])
            self.assertNotIn("service_parent_administration", native)
            self.assertEqual(native["remote_operator"], service["owner"]["native_host"]["remote_operator"])
            original = next(row for row in native["members"] if row["member"]["member"] == "original")
            route = native["routes"][0]
            self.assertEqual(route["origin"], {"kind": "local_console", "member": original["member"],
                "requester": self.console.human(), "ingress_actor": route["source_authority"]})
            self.assertEqual(route["represented_subject"], self.console.human())
            self.assertEqual(scenario.source_authority, route["source_authority"])
            self.assertEqual(original["audience"], {"audience": "principal", "principal": self.console.human()})
            self.assertIn("finite_service_rule", json.loads(Path(service_source["path"]).read_bytes()))
            self.assertNotIn("finite_service_rule", json.loads(Path(selected_source["path"]).read_bytes()))
            # The existing model sequence is shared; only the actual public
            # human response differs from the standing-policy owner response.
            policy = object.__new__(VERIFIER.InstalledAcquisitionProvider)
            with mock.patch.object(VERIFIER.InstalledAcquisitionProvider, "__init__", return_value=None):
                provider = scenario.provider(root, None)
            self.assertEqual(provider.public_reply(), {"state": "reviewed", "access_reviewed": True,
                "grant_issued": True, "fresh_instruction_retained": True, "ready": False})
            self.assertNotEqual(provider.public_reply(), policy.public_reply())

    def test_public_prompt_binds_exact_actual_review_and_original_session(self):
        c = self.console
        operation = VERIFIER.ACQUISITION_OPERATION
        checked = {"validation_id": "actual-fixture-check", "scope": "configuration_only"}
        software = {"review_id": 21, "can_start": False, "prompt": {"kind": "approval",
            "approval_id": "actual-software-approval", "allowed_decisions": ["deny", "approve"],
            "body": {"kind": "service_software_install", "check_tool": "setup_check",
                "scope": "software_installation_only", "owner_proposal": checked,
                "selection": {"family": "agno", "profile": "hackernews", "operations": [operation],
                              "values": {"timeout": 7}}}}}
        self.assertEqual(c.software_review(software, checked, operation),
            {"review_id": 21, "approval_id": "actual-software-approval", "decision": "approve"})
        for change in (lambda v: v.update(can_start=True),
                       lambda v: v["prompt"]["body"].update(owner_proposal={}),
                       lambda v: v["prompt"].update(allowed_decisions=["approve"]),
                       lambda v: v.update(review_id=True)):
            wrong = copy.deepcopy(software); change(wrong)
            with self.assertRaises(AssertionError): c.software_review(wrong, checked, operation)
        accepted = {"session_id": "native-session"}
        owner = VERIFIER.installed_fixture_principal("original-agent")
        attachment = {"destination": {"context": "native-session", "logical_owner": owner,
            "audience": {"audience": "principal", "principal": c.human()}}}
        task_scope = {"realm": "mob.fixture", "owner": "toolkit/acquisition/installed-fixture"}
        source_authority = VERIFIER.installed_fixture_principal("event-source")
        context = {"fresh_original": "actual-fresh",
            "prior_original": {"authority": source_authority, "work": "actual-prior"},
            "business_task": {**task_scope, "id": "actual-task"},
            "live_attachment": attachment, "operator_and_requester": c.human(), "represented_subject": c.human(),
            "proposed_fresh_instruction": VERIFIER.ACQUISITION_REQUEST}
        decision = {"route": VERIFIER.ACQUISITION_ROUTE, "decision": "actual-fresh", "choice": "approve",
            "live_attachment": attachment, "permission": {"target": "original", "connection": "actual-server",
                "resource": "actual-server", "operations": [operation],
                "lifetime": {"scope": {"kind": "task", "task": "actual-fresh"}, "expires_at_ms": 1900000000000}}}
        review = {"review_id": 42, "can_start": False, "prompt": {"kind": "approval",
            "approval_id": "actual-public-approval", "allowed_decisions": ["approve", "deny"],
            "body": {"access_request": {"software": {"job_id": "actual-job", "family": "agno", "profile": "hackernews", "server_name": "actual-server"},
                "phase": "installed_public_service_access", "authentication": {"kind": "none"},
                "application_selection": {"agent": "original", "operations": [operation], "expires_in_ms": None},
                "service_instruction": context}, "service_access": decision}}}
        results = {"original-request": {"operation": "actual-task"}, "installed-job": {"job": {"id": "actual-job"}},
            "reconciled-install": {"observation": {"result": {
            "mcp_configuration": {"mcpServers": {"actual-server": {}}}}}}}
        answer, retained = c.public_review(review, accepted, results, operation, owner, VERIFIER.ACQUISITION_ROUTE, task_scope, source_authority)
        self.assertEqual(answer, {"review_id": 42, "approval_id": "actual-public-approval", "decision": "approve"})
        self.assertIs(retained, context)
        for change in (lambda v: v["prompt"]["body"]["service_access"]["permission"].update(operations=["other"]),
                       lambda v: v["prompt"]["body"]["service_access"]["live_attachment"]["destination"].update(context="other-session"),
                       lambda v: v["prompt"]["body"]["access_request"]["service_instruction"].update(represented_subject=None),
                       lambda v: v["prompt"]["body"]["service_access"]["permission"]["lifetime"].update(scope={"kind": "session"}),
                       lambda v: v["prompt"]["body"]["access_request"]["service_instruction"].update(business_task="actual-task"),
                       lambda v: v["prompt"]["body"]["access_request"]["service_instruction"]["business_task"].update(owner="other-owner"),
                       lambda v: v["prompt"]["body"]["access_request"]["service_instruction"].update(prior_original="actual-prior"),
                       lambda v: v["prompt"]["body"]["access_request"]["service_instruction"]["prior_original"].update(work=" "),
                       lambda v: v["prompt"]["body"]["access_request"]["service_instruction"]["prior_original"].update(work=42),
                       lambda v: v["prompt"]["body"]["access_request"]["service_instruction"]["prior_original"].pop("work"),
                       lambda v: v["prompt"]["body"]["access_request"]["service_instruction"]["prior_original"].update(session_id="native-session"),
                       lambda v: v["prompt"]["body"]["access_request"]["service_instruction"]["prior_original"].update(authority=c.human()),
                       lambda v: v["prompt"]["body"]["access_request"]["service_instruction"]["prior_original"]["authority"].update(id="other-source"),
                       lambda v: v["prompt"]["body"]["access_request"]["service_instruction"]["prior_original"]["authority"]["qualification"].update(trust_domain_id="other-domain")):
            wrong = copy.deepcopy(review); change(wrong)
            with self.assertRaises(AssertionError):
                c.public_review(wrong, accepted, results, operation, owner, VERIFIER.ACQUISITION_ROUTE, task_scope, source_authority)

    def test_approved_service_context_is_exact_and_immediately_before_b(self):
        v = VERIFIER
        proposal = {"acquisition": {"id": "selected-acquisition"}, "decision": {
            "decision": "selected-B", "connection_revision": "a" * 64,
            "permission": {"connection": "selected-server", "resource": "selected-resource",
                "operations": ["read"], "lifetime": {"expires_at_ms": 1900000000000}}}}
        expected = v.installed_approved_service_context(proposal)
        self.assertEqual({k: expected[k] for k in ("operation", "original", "connection", "resource",
            "allowed_operations", "expires_at_ms", "configuration_sha256")}, {
            "operation": "selected-acquisition", "original": "selected-B",
            "connection": "selected-server", "resource": "selected-resource",
            "allowed_operations": ["read"], "expires_at_ms": 1900000000000,
            "configuration_sha256": "a" * 64})
        user = lambda text: {"role": "user", "content": text}
        rows = [user("request"), {"role": "assistant", "content": "A finished"},
                user(v.canonical(expected).decode()), user("request")]
        self.assertIsNone(v.original_catalog_history(rows, "request", "read", 3,
                                                    service_context=expected))
        # An unselected context is not accepted merely because it looks typed.
        with self.assertRaises(AssertionError):
            v.original_catalog_history(rows, "request", "read", 3)
        for key in expected:
            changed = copy.deepcopy(expected)
            changed[key] = "wrong" if not isinstance(changed[key], list) else ["other"]
            wrong = copy.deepcopy(rows)
            wrong[2]["content"] = v.canonical(changed).decode()
            with self.subTest(field=key), self.assertRaises(AssertionError):
                v.original_catalog_history(wrong, "request", "read", 3, service_context=expected)
        for wrong, fresh in (
                (rows[:2] + rows[3:], 2),
                (rows[:2] + [rows[2], rows[2], rows[3]], 4),
                ([rows[0], rows[2], rows[1], rows[3]], 3),
                (rows + [rows[2]], 3),
                (rows[:2] + [{"role": "system", "content": rows[2]["content"]}, rows[3]], 3),
                (rows[:2] + [user(rows[2]["content"] + " trailing"), rows[3]], 3),
                (rows[:2] + [user(v.canonical({**expected, "extra": "field"}).decode()), rows[3]], 3)):
            with self.assertRaises(AssertionError):
                v.original_catalog_history(wrong, "request", "read", fresh, service_context=expected)

    def test_controller_reprepare_feedback_is_single_and_pre_tool_only(self):
        v = VERIFIER
        request, operation = v.ACQUISITION_REQUEST, v.ACQUISITION_OPERATION
        rows = [{"role": "user", "content": request},
                {"role": "user", "content": v.ORIGINAL_CONTROLLER_FEEDBACK}]
        self.assertEqual(v.original_catalog_history(rows, request, operation, 0,
                         allow_controller_recovery=True), 1)
        with self.assertRaises(AssertionError): v.original_catalog_history(rows, request, operation, 0)
        for wrong in (rows + [rows[1]], [rows[0], {"role": "assistant", "tool_calls": [{}]}, rows[1]],
                      [rows[0], {"role": "user", "content": v.ORIGINAL_CONTROLLER_FEEDBACK + " trailing"}]):
            with self.assertRaises(AssertionError):
                v.original_catalog_history(wrong, request, operation, 0, allow_controller_recovery=True)
        with mock.patch.object(v.InstalledAcquisitionProvider, "__init__", return_value=None):
            provider = v.InstalledPublicCatalogProvider(Path("unused"), None)
            self.assertFalse(v.InstalledAcquisitionProvider.controller_reprepare)
            self.original_catalog_response_control(provider, controller_recovery=True)

    def test_controller_reprepare_completed_boundaries_preserve_single_catalog_sequence(self):
        v = VERIFIER
        request, operation = v.ACQUISITION_REQUEST, v.ACQUISITION_OPERATION
        rows = [{"role": "user", "content": request},
                {"role": "assistant", "tool_calls": [{"id": "original-catalog-search"}]},
                {"role": "tool", "tool_call_id": "original-catalog-search", "content": "{}"},
                {"role": "user", "content": v.ORIGINAL_CONTROLLER_FEEDBACK}]
        self.assertEqual(v.original_catalog_history(rows, request, operation, 0,
            allow_controller_recovery=True), 3)
        completed = rows[:-1]
        boundaries = [copy.deepcopy(rows)]
        for call_id in ("original-catalog-load", v.ORIGINAL_STORY_CALL):
            completed += [{"role": "assistant", "tool_calls": [{"id": call_id}]},
                          {"role": "tool", "tool_call_id": call_id, "content": "{}"}]
            history = completed + [rows[-1]]
            self.assertEqual(v.original_catalog_history(history, request, operation, 0,
                allow_controller_recovery=True), len(history) - 1)
            boundaries.append(copy.deepcopy(history))
        for wrong in (rows + [rows[-1]], rows[:2] + [rows[-1]],
                      rows[:3] + [rows[1], rows[2], rows[-1]],
                      rows[:3] + [{"role": "tool", "tool_call_id": "original-catalog-load"}, rows[-1]]):
            with self.assertRaises(AssertionError):
                v.original_catalog_history(wrong, request, operation, 0, allow_controller_recovery=True)
        notice = {"role": "user", "content": (
            "Tool configuration changed at turn boundary: boundary_applied(base_changed=true,visible_changed=true,revision=1)"
            "\nTool configuration changed: boundary_applied(base_changed=true,visible_changed=true,revision=1)")}
        with_notice = boundaries[1][:-1] + [notice, rows[-1]]
        self.assertEqual(v.original_catalog_history(with_notice, request, operation, 0,
            allow_controller_recovery=True), len(with_notice) - 1)
        boundaries.append(with_notice)
        with self.assertRaises(AssertionError):
            v.original_catalog_history(rows, request, operation, 0)
        state = self.controller_reprepare_state()
        prior = copy.deepcopy(state["authorization_audit"][2:])
        for row in prior: row["observation"]["operation_id"] = "earlier-model"
        state["authorization_audit"] = prior + state["authorization_audit"]
        projected = v.original_reprepare_projection(state, "session", "B", "run-B")
        self.assertEqual(projected["refusal_count"], 2)
        with tempfile.TemporaryDirectory(prefix="inert-post-search-audit-") as scratch:
            directory = Path(scratch)
            projected["row_sha256"] = "a" * 64
            path = directory / "original-b-controller-recovery.json"
            path.write_bytes(v.canonical(projected))
            acquisition = {"controller_recovery_audit": v.artifact_pin(path)}
            business = {"original_catalog": {}, "controller_recovery_history_index": 3}
            receipt = {"input_id": "B", "run_id": "run-B", "outcome": {"session_id": "session"}}
            for history in boundaries:
                business["controller_recovery_history_index"] = len(history) - 1
                ASSEMBLER.gateway_controller_recovery(acquisition, business, history, 0, directory, receipt)
            business["controller_recovery_history_index"] = 3
            for wrong in (rows + [rows[-1]], rows[:2] + [rows[-1]]):
                with self.assertRaises(ValueError):
                    ASSEMBLER.gateway_controller_recovery(acquisition, business, wrong, 0, directory, receipt)
        for mutate in (
            lambda x: x["authorization_audit"][0]["observation"].update(operation_id="refused"),
            lambda x: x["authorization_audit"][0]["observation"].update(observation={"kind": "authorization_unavailable"}),
            lambda x: x["authorization_audit"][3]["observation"]["observation"].update(reason="denied"),
            lambda x: x["authorization_audit"].append(copy.deepcopy(x["authorization_audit"][3])),
        ):
            wrong = copy.deepcopy(state); mutate(wrong)
            with self.assertRaises(AssertionError):
                v.original_reprepare_projection(wrong, "session", "B", "run-B")

    def test_controlled_provider_preserves_explicit_tool_refusals(self):
        v = VERIFIER
        provider = object.__new__(v.InstalledAcquisitionProvider)
        for identifier in ("original-catalog-search", "original-catalog-load", "helper-catalog-load-1", "install"):
            body = {"model": "fixture-original-model", "tools": [], "messages": [
                {"role": "tool", "tool_call_id": identifier, "content": json.dumps({
                    "error": "operation_refused", "message": "operation unavailable under current authorization"})}]}
            with self.assertRaisesRegex(AssertionError, "actual tool result " + identifier + " refused: operation_refused"):
                provider.response(body)

    def controller_reprepare_state(self):
        target = {"kind": "model", "model": "fixture-original-model", "provider": "self_hosted",
                  "usage": "controller_inference", "hosted_capabilities": [], "endpoint": "inert-only"}
        refused = {"kind": "refused", "reason": "reprepare_required", "target": target}
        def row(op, value):
            return {"contributors": [{"input_id": "B"}], "observation": {
                "run_id": "run-B", "operation_id": op, "observation": value}}
        return {"input_id": "B", "last_run_id": "run-B", "authorization_audit": [
            row("refused", refused), row("refused", copy.deepcopy(refused)),
            row("recovered", {"kind": "prepared", "target": copy.deepcopy(target),
                "policy": {"publication_sequence": 94, "observed_at_ms": 100}}),
            row("recovered", {"kind": "entry"}), row("recovered", {"kind": "outcome",
                "outcome": {"kind": "http_response", "status": 200}})]}

    def test_controller_reprepare_audit_rejects_other_refusals_targets_and_inputs(self):
        v = VERIFIER
        state = self.controller_reprepare_state()
        observed = v.original_reprepare_projection(state, "session", "B", "run-B")
        self.assertEqual((observed["reason"], observed["refusal_count"], observed["recovered_http_status"]),
                         ("reprepare_required", 2, 200))
        mutations = [
            lambda x: x.update(input_id="other"),
            lambda x: x.update(last_run_id="other"),
            lambda x: x["authorization_audit"][0]["contributors"][0].update(input_id="other"),
            lambda x: x["authorization_audit"][0]["observation"]["observation"].update(reason="denied"),
            lambda x: x["authorization_audit"][0]["observation"]["observation"].update(reason="malformed_facts"),
            lambda x: x["authorization_audit"][0]["observation"]["observation"]["target"].update(hosted_capabilities=["web_search"]),
            lambda x: x["authorization_audit"][2]["observation"]["observation"]["target"].update(endpoint="different"),
            lambda x: x["authorization_audit"][2]["observation"].update(operation_id="refused"),
            lambda x: x["authorization_audit"][4]["observation"]["observation"]["outcome"].update(status=500),
            lambda x: x["authorization_audit"].append(copy.deepcopy(x["authorization_audit"][0])),
            lambda x: x["authorization_audit"].append({"contributors": [{"input_id": "B"}],
                "observation": {"run_id": "run-B", "operation_id": "refused", "observation": {"kind": "entry"}}})]
        for mutate in mutations:
            wrong = copy.deepcopy(state); mutate(wrong)
            with self.assertRaises(AssertionError): v.original_reprepare_projection(wrong, "session", "B", "run-B")

    def test_controller_reprepare_reads_only_exact_persisted_input(self):
        v = VERIFIER
        with tempfile.TemporaryDirectory(prefix="inert-controller-store-") as scratch:
            directory = Path(scratch); (directory / "state").mkdir()
            database = directory / "state/runtime.sqlite"
            state = self.controller_reprepare_state()
            with v.sqlite3.connect(database) as connection:
                connection.execute("CREATE TABLE runtime_input_states(runtime_id TEXT, input_id TEXT, state_json BLOB)")
                connection.execute("INSERT INTO runtime_input_states VALUES(?,?,?)",
                                   ("rt:session:session", "B", v.canonical(state)))
                connection.execute("INSERT INTO runtime_input_states VALUES(?,?,?)",
                                   ("rt:session:other", "B", b"unrelated row must not be parsed"))
            before = database.read_bytes()
            pin = v.retained_original_reprepare(directory, "session", "B", "run-B")
            result = json.loads(Path(pin["path"]).read_bytes())
            self.assertEqual(result["row_sha256"], sha(v.canonical(state)))
            self.assertEqual(result["input_id"], "B")
            self.assertNotIn("inert-only", Path(pin["path"]).read_text())
            self.assertEqual(database.read_bytes(), before)

    def test_controller_reprepare_assembler_requires_exact_retained_audit(self):
        v, a = VERIFIER, ASSEMBLER
        with tempfile.TemporaryDirectory(prefix="inert-controller-audit-") as scratch:
            directory = Path(scratch)
            audit = v.original_reprepare_projection(self.controller_reprepare_state(), "session", "B", "run-B")
            audit["row_sha256"] = "a" * 64
            path = directory / "original-b-controller-recovery.json"
            messages = [{"role": "user", "content": v.ACQUISITION_REQUEST},
                        {"role": "user", "content": v.ORIGINAL_CONTROLLER_FEEDBACK}]
            business = {"original_catalog": {}, "controller_recovery_history_index": 1}
            receipt = {"input_id": "B", "run_id": "run-B", "outcome": {"session_id": "session"}}
            def check(value):
                path.write_bytes(v.canonical(value))
                a.gateway_controller_recovery({"controller_recovery_audit": v.artifact_pin(path)},
                                              business, messages, 0, directory, receipt)
            check(audit)
            for key, value in (("reason", "denied"), ("reason", "malformed_facts"), ("input_id", "other"),
                               ("run_id", "other"), ("hosted_capabilities", ["web_search"]),
                               ("recovery_operation_id", "refused"), ("refusal_count", 0),
                               ("recovered_http_status", 500)):
                with self.assertRaises(ValueError): check({**audit, key: value})
            with self.assertRaises(ValueError):
                a.gateway_controller_recovery({}, business, messages, 0, directory, receipt)
            a.gateway_controller_recovery({}, {}, messages[:1], 0, directory, receipt)

    def test_original_catalog_accepts_only_exact_core_090_display_notices(self):
        v = VERIFIER
        operation, request = "list_boards", "the selected original business request"
        suffix = "\nTool configuration changed: deferred catalog updated"
        added = "Deferred catalog changed at turn boundary: new deferred tools available: " + operation + suffix
        visible = "Deferred catalog changed at turn boundary: deferred tools now available to call: " + operation + suffix
        for notice, kind in ((added, "added"), (visible, "removed")):
            self.assertEqual(v.original_tool_notice(notice, operation), kind)
            for wrong in (notice + " trailing", notice + "\n", notice.replace(operation, "other"),
                          notice.replace(operation, operation + ", other"),
                          notice.replace(suffix, ""), notice.replace("catalog updated", "catalog accepted")):
                self.assertIsNone(v.original_tool_notice(wrong, operation))
        self.assertIsNone(v.original_tool_notice(
            visible.replace("deferred tools now available to call: ", "deferred tools no longer available: "), operation))
        user = lambda content: {"role": "user", "content": content}
        loaded = {"role": "tool", "tool_call_id": "original-catalog-load", "content": "{}"}
        messages = [user(request), user(added), loaded, user(visible)]
        v.original_catalog_history(messages, request, operation, 0)
        for wrong in ([user(request), user(visible)],
                      [user(request), user(added), user(added)],
                      [user(request), loaded, user(added)],
                      [user(request), loaded, user(visible)],
                      messages + [user(visible)], messages + [user(added)],
                      messages + [user(request)], messages + [user("another business input")],
                      messages + [user(added.replace(operation, "other"))]):
            with self.assertRaises(AssertionError):
                v.original_catalog_history(wrong, request, operation, 0)

    def test_original_catalog_uses_exact_native_results_before_business_call(self):
        scenario = self.console.ConsoleScenario(VERIFIER, "http://127.0.0.1:18444")
        with mock.patch.object(VERIFIER.InstalledAcquisitionProvider, "__init__", return_value=None):
            provider = scenario.provider(Path("unused"), None)
        self.original_catalog_response_control(provider, expected_tool_policy={"tools": [
            {"name": VERIFIER.ACQUISITION_OPERATION, "consequence": "r2"}], "catalog": "r2"})

    def test_installed_original_requests_reviewed_catalog_and_reads_only_after_exact_load(self):
        with mock.patch.object(VERIFIER.InstalledAcquisitionProvider, "__init__", return_value=None):
            provider = VERIFIER.InstalledPublicCatalogProvider(Path("unused"), None)
            unselected = VERIFIER.InstalledAcquisitionProvider(Path("unused"), None)
        self.assertFalse(unselected.original_catalog)
        self.assertNotIn("tool_policy", unselected.access_arguments("job", [VERIFIER.ACQUISITION_OPERATION]))
        self.assertTrue(provider.original_catalog)
        self.original_catalog_response_control(provider)

    def original_catalog_response_control(self, provider, *, controller_recovery=False, expected_tool_policy=None):
        import threading
        v = VERIFIER
        provider.results, provider.issued, provider.requests = {}, set(), []
        provider.events, provider.business_observation = mock.Mock(), None
        provider.business_response_prepared = threading.Event()
        operation = v.ACQUISITION_OPERATION
        boundary = ("Tool configuration changed at turn boundary: "
            "boundary_applied(base_changed=true,visible_changed=false,revision=0)\n"
            "Tool configuration changed: boundary_applied(base_changed=true,visible_changed=false,revision=0)")
        added = ("Deferred catalog changed at turn boundary: new deferred tools available: " + operation
            + "\nTool configuration changed: deferred catalog updated")
        removed = ("Deferred catalog changed at turn boundary: deferred tools now available to call: " + operation
            + "\nTool configuration changed: deferred catalog updated")
        messages = [{"role": "user", "content": v.ACQUISITION_REQUEST}]
        body = {"model": "fixture-original-model", "messages": messages,
                "tools": [{"function": {"name": name}} for name in
                    ("find_and_install", "tool_catalog_search", "tool_catalog_load")]}
        def response(identifier, name, args):
            provider.requests.append(copy.deepcopy(body))
            actual = provider.response(body)
            call = actual["delta"]["tool_calls"][0]
            self.assertEqual((call["id"], call["function"]["name"], json.loads(call["function"]["arguments"])),
                             (identifier, name, args))
            messages.append({"role": "assistant", "tool_calls": [call]})
        def result(identifier, value):
            messages.append({"role": "tool", "tool_call_id": identifier, "content": json.dumps(value)})
        response("original-request", "find_and_install", {"request": v.ACQUISITION_REQUEST,
                 "user_context": [], "caller_observations": []})
        result("original-request", {"operation": "actual-task", "created": True,
            "worker": {"state": "input_admitted", "deduplicated": False,
                       "source_settlement": "fresh_acceptance_promoted"}})
        self.assertEqual(provider.response(body)["delta"]["content"], v.ORIGINAL_A_FINISH)
        messages.extend([{"role": "assistant", "content": v.ORIGINAL_A_FINISH},
            {"role": "user", "content": v.ACQUISITION_REQUEST},
            {"role": "user", "content": boundary}, {"role": "user", "content": added}])
        fresh = 4
        if controller_recovery:
            messages.append({"role": "user", "content": v.ORIGINAL_CONTROLLER_FEEDBACK})
        # This is the actual response seam; no server/native/model is started.
        for extra in (v.ACQUISITION_REQUEST, "unrelated input", added + " trailing",
                      added.replace(operation, "other_operation"), removed,
                      boundary.replace("revision=0", "revision=00"),
                      boundary.replace("base_changed=true", "base_changed=false")):
            wrong = copy.deepcopy(body); wrong["messages"].append({"role": "user", "content": extra})
            with self.assertRaises(AssertionError): provider.response(wrong)
        response("original-catalog-search", "tool_catalog_search", {"query": operation, "limit": 50})
        found = {"catalog_exact": True, "pending_sources": [],
                 "results": [{"name": operation, "visibility_status": "deferred"}]}
        for bad in ({**found, "catalog_exact": False}, {**found, "pending_sources": ["pending"]},
                    {**found, "results": [{"name": "other", "visibility_status": "deferred"}]}):
            wrong = copy.deepcopy(body)
            wrong["messages"].append({"role": "tool", "tool_call_id": "original-catalog-search", "content": json.dumps(bad)})
            with self.assertRaises(AssertionError): provider.response(wrong)
            provider.results.pop("original-catalog-search", None)
        result("original-catalog-search", found)
        response("original-catalog-load", "tool_catalog_load", {"names": [operation]})
        loaded = {"catalog_exact": True, "accepted_names": [operation], "noop_names": [],
                  "resolutions": [{"name": operation, "accepted": True, "accepted_noop": False}]}
        for bad in ({**loaded, "accepted_names": [operation, "other"]},
                    {**loaded, "resolutions": [{"name": operation, "accepted": False}]}):
            wrong = copy.deepcopy(body)
            wrong["messages"].append({"role": "tool", "tool_call_id": "original-catalog-load", "content": json.dumps(bad)})
            with self.assertRaises(AssertionError): provider.response(wrong)
            provider.results.pop("original-catalog-load", None)
        result("original-catalog-load", loaded)
        messages.extend([{"role": "user", "content": boundary.replace("visible_changed=false", "visible_changed=true")},
                         {"role": "user", "content": removed}])
        with self.assertRaises(AssertionError): provider.response(body)
        body["tools"].append({"function": {"name": operation}})
        response(v.ORIGINAL_STORY_CALL, operation, {"num_stories": 1})
        story = [{"id": 123, "title": "Controlled public story"}]
        content = json.dumps(story) + "\n" + json.dumps({"value": json.dumps(story)})
        messages.append({"role": "tool", "tool_call_id": v.ORIGINAL_STORY_CALL, "content": content})
        self.assertEqual(provider.response(body)["delta"]["content"], "Hacker News story 123: Controlled public story")
        self.assertEqual(provider.business_observation["b_input_history_index"], fresh)
        if controller_recovery:
            self.assertEqual(provider.business_observation["controller_recovery_history_index"], 7)
        self.assertEqual(provider.business_observation["original_catalog"]["load_result"], loaded)
        self.assertEqual(provider.access_arguments("job", [operation])["tool_policy"],
                         {"tools": [], "catalog": "r2"} if expected_tool_policy is None else expected_tool_policy)
        self.assertNotIn("tool_policy", v.InstalledAcquisitionProvider.access_arguments(provider, "job", [operation]))
        with self.assertRaises(AssertionError): provider.response(body)

    def test_catalog_review_preserves_whole_native_policy_and_original_session(self):
        c, v = self.console, VERIFIER
        def artifact(revision, members, source):
            payload = {"schema_version": 1, "provider_id": "fixture", "policy_id": "tools", "revision": revision,
                       "source": source, "default_deny": True, "members": members}
            digest = v.installed_fixture_policy_digest(json.dumps(payload, separators=(",", ":")).encode())
            full = {key: payload[key] for key in ("schema_version", "provider_id", "policy_id", "revision")}
            full.update(policy_digest=digest, **{key: payload[key] for key in ("source", "default_deny", "members")})
            return json.dumps(full, separators=(",", ":")).encode() + b"\n"
        grants = lambda names: [{"tool_name": name, "action": "invoke", "consequence": "r2"} for name in sorted(names)]
        members = [{"member_identity": "helper", "grants": grants(["setup_report"])},
                   {"member_identity": "original", "grants": grants(["find_and_install", v.ACQUISITION_OPERATION])}]
        prior = artifact(1, members, {"source_id": "fixture", "source_digest": "sha256:" + "a" * 64})
        successor = copy.deepcopy(members)
        successor[1]["grants"] = grants(["find_and_install", v.ACQUISITION_OPERATION, "tool_catalog_search", "tool_catalog_load"])
        candidate = artifact(2, successor, {"source_id": "toolkit-application-service-decision", "source_digest": "sha256:" + "b" * 64})
        proposal = {"provider_id": "fixture", "policy_id": "tools", "member": "original",
            "selection": {"tools": [], "catalog": "r2"}, "catalog_session": "original-session", "lifetime": "until_superseded",
            "prior": {"revision": 1, "digest": v.installed_fixture_policy_digest(prior)},
            "candidate": {"revision": 2, "digest": v.installed_fixture_policy_digest(candidate)},
            "candidate_canonical_json": candidate.decode()}
        request = {"member_tool_policy": {"proposal": proposal,
            "approval_scope": "the complete successor policy, including preserved permissions for all listed members",
            "lifetime": "until superseded; this member policy remains after the service task ends",
            "catalog_visibility": "when selected, eligible tool metadata in the entire original conversation, not only this integration",
            "native_service_scope": "the separate native service and catalog grants still expire with this task"}}
        accepted, decision = {"session_id": "original-session"}, {"catalog": "original_session"}
        c.reviewed_original_catalog(request, decision, accepted, prior, v)
        for field, value in (("member", "helper"), ("catalog_session", "helper-session"),
                             ("selection", {"tools": [], "catalog": "r3"}), ("prior", {"revision": 1, "digest": "wrong"})):
            wrong = copy.deepcopy(request); wrong["member_tool_policy"]["proposal"][field] = value
            with self.assertRaises(AssertionError): c.reviewed_original_catalog(wrong, decision, accepted, prior, v)
        # Even internally rehashed candidates cannot smuggle a different member grant.
        for changed in ([], grants(["setup_report", "other_operation"])):
            wrong = copy.deepcopy(request); rows = copy.deepcopy(successor); rows[0]["grants"] = changed
            altered = artifact(2, rows, json.loads(candidate)["source"])
            selected = wrong["member_tool_policy"]["proposal"]
            selected["candidate_canonical_json"] = altered.decode()
            selected["candidate"]["digest"] = v.installed_fixture_policy_digest(altered)
            with self.assertRaises(AssertionError): c.reviewed_original_catalog(wrong, decision, accepted, prior, v)
        with self.assertRaises(AssertionError): c.reviewed_original_catalog(request, {"catalog": None}, accepted, prior, v)
        wrong = copy.deepcopy(request)
        selected = wrong["member_tool_policy"]["proposal"]
        alternate = json.dumps(json.loads(candidate), indent=2).encode() + b"\n"
        selected["candidate_canonical_json"] = alternate.decode()
        selected["candidate"]["digest"] = v.installed_fixture_policy_digest(alternate)
        with self.assertRaises(AssertionError): c.reviewed_original_catalog(wrong, decision, accepted, prior, v)

    def journey(self):
        v = VERIFIER
        accepted = {"identity": "original", "session_id": "actual-session", "interaction_id": "actual-A",
                    "input_frame_id": "accepted-send"}
        story = {"id": 123, "title": "Actual fixture story"}
        text = json.dumps([story])
        structured = {"value": text}
        content = text + "\n" + v.canonical(structured).decode()
        business = {"story": story, "answer": "Hacker News story 123: Actual fixture story", "actual_tool_result": content}
        context = {"proposed_fresh_instruction": v.ACQUISITION_REQUEST}
        def frame(identifier, kind, run, payload, *, session=True):
            payload = {"source_event_type": "run_completed" if kind == "interaction_complete" else kind,
                "identity": {"run_id": run}, **payload}
            value = {"id": identifier, "kind": kind, "identity": "original", "run_id": run,
                "payload": payload,
                "status": "completed" if kind == "interaction_complete" else "delivered",
                "source": {"kind": "console_event"}, "source_event_id": identifier}
            if session: value["session_id"] = payload["session_id"] = "actual-session"
            return value
        frames = [frame("a-start", "run_started", "run-A", {"input": {"kind": "content", "content": v.ACQUISITION_REQUEST}}),
            frame("a-done", "interaction_complete", "run-A", {"result": v.ORIGINAL_A_FINISH, "assistant_message_id": "a-answer"}),
            frame("b-start", "run_started", "run-B", {"input": {"kind": "content", "content": [{"type": "text", "text": v.ACQUISITION_REQUEST}]}}),
            frame("call", "tool_call_requested", "run-B", {"id": v.ORIGINAL_STORY_CALL, "name": v.ACQUISITION_OPERATION,
                "args": {"num_stories": 1}}, session=False),
            frame("result", "tool_result_received", "run-B", {"id": v.ORIGINAL_STORY_CALL, "name": v.ACQUISITION_OPERATION,
                "is_error": False, "content": [{"type": "text", "text": text},
                    {"type": "structured", "data": structured}]}, session=False),
            frame("b-done", "interaction_complete", "run-B", {"result": business["answer"], "assistant_message_id": "b-answer"})]
        anchor = {"id": accepted["input_frame_id"], "kind": "user_input", "source": {"kind": "send"},
            "identity": accepted["identity"], "session_id": accepted["session_id"],
            "interaction_id": accepted["interaction_id"], "status": "delivered",
            "payload": {"content": v.ACQUISITION_REQUEST}}
        return [anchor] + frames, accepted, context, business

    def test_native_completion_requires_real_run_tool_result_and_same_original(self):
        c = self.console
        frames, accepted, context, business = self.journey()
        result = c.completed_journey(frames, accepted, context, business, VERIFIER)
        self.assertEqual((result["a_run_id"], result["b_run_id"], result["session_id"]),
                         ("run-A", "run-B", "actual-session"))
        self.assertEqual(result["assistant_message_id"], "b-answer")
        mutations = [lambda f: f[-1].update(kind="text_complete"),
            lambda f: f[-1]["payload"].update(source_event_type="turn_completed"),
            lambda f: f[-1].update(source={"kind": "session_history"}),
            lambda f: f[-1].update(source={"kind": "synthetic"}),
            lambda f: f[-1].update(identity="other-original"),
            lambda f: f[-1].update(session_id="other-session"),
            lambda f: f[-1]["payload"]["identity"].update(run_id="other-run"),
            lambda f: f[-1].pop("run_id"),
            lambda f: f[-1].update(source_event_id="other-source"),
            lambda f: f[-1].pop("source_event_id"),
            lambda f: f[-1]["payload"].update(result="invented answer"),
            lambda f: f[-1]["payload"].pop("assistant_message_id"),
            lambda f: f[-2]["payload"].update(is_error=True),
            lambda f: f[-2]["payload"].update(name="other-operation"),
            lambda f: f[-2]["payload"]["content"][1].update(type="unknown"),
            lambda f: f[-2]["payload"]["content"][1].update(data={"value": "[]"}),
            lambda f: f[-2]["payload"]["content"][1].update(extra="ignored"),
            lambda f: f[-2]["payload"]["content"].append({"type": "text", "text": "extra"}),
            lambda f: f[-2]["payload"]["content"].reverse(),
            lambda f: f[-2].update(run_id="other-run"),
            lambda f: f.pop(-2),
            lambda f: f.append(copy.deepcopy(f[-1])),
            lambda f: f.insert(5, copy.deepcopy(f[4])),
            lambda f: f[3]["payload"].update(input={"kind": "pending_tool_results"})]
        for index, change in enumerate(mutations):
            with self.subTest(mutation=index):
                wrong = copy.deepcopy(frames); change(wrong)
                try: value = c.completed_journey(wrong, accepted, context, business, VERIFIER)
                except AssertionError: value = None
                self.assertIsNone(value)
        altered = copy.deepcopy(business); altered["actual_tool_result"] += "\n"
        with self.assertRaises(AssertionError):
            c.completed_journey(frames, accepted, context, altered, VERIFIER)
        # Contradictory typed values fail even if the model observed that same projection.
        wrong = copy.deepcopy(frames)
        wrong[-2]["payload"]["content"][1]["data"] = {"value": "[]"}
        altered = copy.deepcopy(business)
        altered["actual_tool_result"] = (wrong[-2]["payload"]["content"][0]["text"] + "\n"
            + VERIFIER.canonical(wrong[-2]["payload"]["content"][1]["data"]).decode())
        with self.assertRaises(AssertionError):
            c.completed_journey(wrong, accepted, context, altered, VERIFIER)
        catalog_frames = []
        business["original_catalog"] = {"operation": VERIFIER.ACQUISITION_OPERATION}
        for prefix, tool, args in (("search", "tool_catalog_search", {"query": VERIFIER.ACQUISITION_OPERATION, "limit": 50}),
                                  ("load", "tool_catalog_load", {"names": [VERIFIER.ACQUISITION_OPERATION]})):
            receipt = {"inert_fixture": prefix}
            business["original_catalog"][prefix + "_result"] = receipt
            for kind in ("tool_call_requested", "tool_result_received"):
                frame = copy.deepcopy(frames[4])
                identifier = prefix + "-" + kind
                frame.update(id=identifier, source_event_id=identifier, kind=kind)
                frame["payload"].update(id="original-catalog-" + prefix, name=tool,
                    args=args, is_error=False, content=[{"type": "text", "text": json.dumps(receipt)}])
                catalog_frames.append(frame)
        with_catalog = frames[:4] + catalog_frames + frames[4:]
        self.assertIsNotNone(c.completed_journey(with_catalog, accepted, context, business, VERIFIER))
        for change in (lambda f: f.pop(4), lambda f: f[5].update(run_id="helper-run"),
                       lambda f: f[6]["payload"].update(args={"names": ["other"]}),
                       lambda f: f[7]["payload"].update(is_error=True),
                       lambda f: f[7]["payload"].update(content=[{"type": "text", "text": "{}"}]),
                       lambda f: f.insert(7, copy.deepcopy(f[7]))):
            wrong = copy.deepcopy(with_catalog); change(wrong)
            try: value = c.completed_journey(wrong, accepted, context, business, VERIFIER)
            except AssertionError: value = None
            self.assertIsNone(value)
        # A native failure after accepted send remains a failure, not a retry.
        failed = copy.deepcopy(frames[1]); failed.update(kind="interaction_failed")
        with self.assertRaisesRegex(AssertionError, "actual original Console interaction failed"):
            c.completed_journey([failed], accepted, None, None, VERIFIER)

    def test_native_run_pair_uses_accepted_send_when_interaction_lineage_is_absent(self):
        c = self.console
        frames, accepted, context, business = self.journey()
        self.assertIsNotNone(c.completed_journey(frames, accepted, context, business, VERIFIER))
        explicit = copy.deepcopy(frames)
        for frame in explicit[1:]:
            frame["interaction_id"] = frame["payload"]["identity"]["interaction_id"] = accepted["interaction_id"]
        self.assertIsNotNone(c.completed_journey(explicit, accepted, context, business, VERIFIER))
        # Incomplete observations cannot establish a completed journey.
        for partial in (frames[1:], frames[:3], frames[:-1]):
            self.assertIsNone(c.completed_journey(partial, accepted, context, business, VERIFIER))
        # Session-history projection is not a second accepted send.
        history = copy.deepcopy(frames[0]); history.update(id="history", source={"kind": "session_history"})
        self.assertIsNotNone(c.completed_journey([history] + frames, accepted, context, business, VERIFIER))
        third = copy.deepcopy(frames[3]); third.update(id="third", source_event_id="third", run_id="run-C")
        third["payload"]["identity"]["run_id"] = "run-C"
        duplicate_done = copy.deepcopy(frames[-1]); duplicate_done.update(id="other-done", source_event_id="other-done")
        second_send = copy.deepcopy(frames[0]); second_send.update(id="other-send")
        mutations = [lambda f: f.append(third), lambda f: f.append(duplicate_done),
            lambda f: f.append(second_send), lambda f: f.append(copy.deepcopy(f[1])),
            lambda f: f[3].update(source_event_id=f[1]["source_event_id"]),
            lambda f: f[3].update(interaction_id="foreign"),
            lambda f: f[3]["payload"]["identity"].update(interaction_id="foreign"),
            lambda f: f[3].update(run_id="run-A"),
            lambda f: f[3]["payload"].update(session_id="foreign"),
            lambda f: f[3]["payload"].update(input={"kind": "content", "content": "another request"}),
            lambda f: f[0].update(identity="foreign"), lambda f: f[0].update(session_id="foreign"),
            lambda f: f[0].update(interaction_id="foreign"), lambda f: f[0].update(status="failed"),
            lambda f: f[0].update(source={"kind": "session_history"}),
            lambda f: f[0]["payload"].update(content="another request"),
            lambda f: f.insert(3, f.pop(2)), lambda f: f.append(f.pop(0))]
        for index, change in enumerate(mutations):
            with self.subTest(mutation=index):
                wrong = copy.deepcopy(frames); change(wrong)
                with self.assertRaises(AssertionError):
                    c.completed_journey(wrong, accepted, context, business, VERIFIER)

    def credential_journey(self):
        c = self.console
        frames, accepted, context, _business = self.journey()
        # Inert setup-only evidence has no B tool call or provider operation.
        frames = [frame for frame in frames if frame["kind"] not in
                  ("tool_call_requested", "tool_result_received")]
        frames[0]["payload"]["content"] = c.TRELLO_REQUEST
        for frame in frames:
            if frame["kind"] == "run_started":
                frame["payload"]["input"] = {"kind": "content", "content": c.TRELLO_REQUEST}
        frames[-1]["payload"]["result"] = c.TRELLO_FINISH
        context["proposed_fresh_instruction"] = c.TRELLO_REQUEST
        business = {"answer": c.TRELLO_FINISH, "attached_operation": c.TRELLO_OPERATION}
        return frames, accepted, context, business

    def test_credential_response_retains_history_and_exact_deferred_metadata_without_effects(self):
        import threading
        v, c = VERIFIER, self.console
        boundary = ("Tool configuration changed at turn boundary: "
            "boundary_applied(base_changed=true,visible_changed=false,revision=0)\n"
            "Tool configuration changed: boundary_applied(base_changed=true,visible_changed=false,revision=0)")
        added = ("Deferred catalog changed at turn boundary: new deferred tools available: " + c.TRELLO_OPERATION
            + "\nTool configuration changed: deferred_catalog_delta(added_hidden=1,removed_hidden=0,pending_sources=0)")

        def fixture(inline=False):
            scenario = c.TrelloConsoleScenario(v, "http://127.0.0.1:18444")
            with mock.patch.object(v.InstalledAcquisitionProvider, "__init__", return_value=None):
                provider = scenario.provider(Path("unused"), None)
            provider.results, provider.issued, provider.requests = {}, set(), []
            provider.events, provider.business_observation = mock.Mock(), None
            provider.business_response_prepared = threading.Event()
            messages = [{"role": "user", "content": c.TRELLO_REQUEST}]
            body = {"model": "fixture-original-model", "messages": messages,
                    "tools": [{"function": {"name": name}} for name in
                        ("find_and_install", "tool_catalog_search", "tool_catalog_load")]}
            provider.requests.append(copy.deepcopy(body))
            call = provider.response(body)["delta"]["tool_calls"][0]
            self.assertEqual((call["id"], call["function"]["name"]), ("original-request", "find_and_install"))
            messages.extend([{"role": "assistant", "tool_calls": [call]},
                {"role": "tool", "tool_call_id": "original-request", "content": json.dumps({
                    "operation": "inert-task", "created": True, "worker": {"state": "input_admitted",
                    "deduplicated": False, "source_settlement": "fresh_acceptance_promoted"}})}])
            self.assertEqual(provider.response(body)["delta"]["content"], v.ORIGINAL_A_FINISH)
            messages.extend([{"role": "assistant", "content": v.ORIGINAL_A_FINISH},
                {"role": "user", "content": c.TRELLO_REQUEST},
                {"role": "user", "content": boundary}])
            if inline:
                body["tools"].append({"function": {"name": c.TRELLO_OPERATION}})
            else:
                messages.append({"role": "user", "content": added})
            # This is only an inert oracle control. Native review/persistence and
            # native A/B completion remain independently required by the scenario.
            scenario._persistence_proposal = {"inert_review": True}
            scenario._credential_service_context = {"inert_review": "selected-context"}
            messages.insert(4, {"role": "user",
                "content": v.canonical(scenario._credential_service_context).decode()})
            return scenario, provider, body

        for inline in (False, True):
            with self.subTest(inline=inline):
                scenario, provider, body = fixture(inline)
                before = (copy.deepcopy(provider.results), set(provider.issued))
                provider.check_model_request(body)
                self.assertEqual(provider.response(body), {"delta": {"content": c.TRELLO_FINISH}, "finish_reason": "stop"})
                self.assertEqual((provider.results, provider.issued), before)
                self.assertEqual(provider.business_observation["operation_metadata_visibility"],
                                 "inline" if inline else "deferred")
                self.assertEqual(provider.business_observation["vendor_operation"], "not_requested")
                with self.assertRaises(AssertionError):
                    provider.response(body)

        changes = [
            ("missing_original", lambda s, p, b: b["messages"].pop(0)),
            ("missing_fresh", lambda s, p, b: b["messages"].pop(5)),
            ("missing_a_completion", lambda s, p, b: b["messages"].pop(3)),
            ("duplicate_input", lambda s, p, b: b["messages"].append({"role": "user", "content": c.TRELLO_REQUEST})),
            ("unrelated_input", lambda s, p, b: b["messages"].insert(4, {"role": "user", "content": "another input"})),
            ("duplicate_notice", lambda s, p, b: b["messages"].append(copy.deepcopy(b["messages"][-1]))),
            ("duplicate_boundary", lambda s, p, b: b["messages"].append({"role": "user", "content": boundary})),
            ("foreign_metadata", lambda s, p, b: b["messages"][-1].update(content=added.replace(c.TRELLO_OPERATION, "other"))),
            ("noncanonical_metadata", lambda s, p, b: b["messages"][-1].update(content=added + " trailing")),
            ("missing_metadata", lambda s, p, b: b["messages"].pop()),
            ("contradictory_visibility", lambda s, p, b: b["tools"].append({"function": {"name": c.TRELLO_OPERATION}})),
            ("missing_review", lambda s, p, b: setattr(s, "_persistence_proposal", None)),
            ("unissued_setup", lambda s, p, b: p.issued.clear()),
            ("altered_setup_result", lambda s, p, b: b["messages"][2].update(content="{}")),
            ("extra_tool_result", lambda s, p, b: b["messages"].append({"role": "tool", "tool_call_id": "unissued", "content": "{}"})),
            ("vendor_call", lambda s, p, b: b["messages"].append({"role": "assistant", "tool_calls": [
                {"id": "vendor", "function": {"name": c.TRELLO_OPERATION, "arguments": "{}"}}]})),
        ]
        for name, change in changes:
            with self.subTest(refusal=name):
                scenario, provider, body = fixture()
                change(scenario, provider, body)
                with self.assertRaises(AssertionError):
                    provider.check_model_request(body)
                    provider.response(body)
                self.assertIsNone(provider.business_observation)

    def test_credential_native_pair_preserves_anchor_lineage_and_order(self):
        c = self.console
        frames, accepted, context, business = self.credential_journey()
        result = c.completed_credential_setup(frames, accepted, context, business, VERIFIER)
        self.assertEqual((result["a_run_id"], result["b_run_id"], result["session_id"]),
                         ("run-A", "run-B", accepted["session_id"]))
        self.assertEqual(result["answer"], c.TRELLO_FINISH)
        self.assertEqual(result["vendor_operation"], "not_requested_or_observed")
        explicit = copy.deepcopy(frames)
        for frame in explicit[1:]:
            frame["interaction_id"] = frame["payload"]["identity"]["interaction_id"] = accepted["interaction_id"]
        self.assertIsNotNone(c.completed_credential_setup(explicit, accepted, context, business, VERIFIER))
        for partial in (frames[1:], frames[:3], frames[:-1]):
            self.assertIsNone(c.completed_credential_setup(partial, accepted, context, business, VERIFIER))
        third = copy.deepcopy(frames[3]); third.update(id="third", source_event_id="third", run_id="run-C")
        third["payload"]["identity"]["run_id"] = "run-C"
        duplicate_done = copy.deepcopy(frames[-1]); duplicate_done.update(id="other-done", source_event_id="other-done")
        mutations = [lambda f: f[0].update(interaction_id="foreign"),
            lambda f: f[0]["payload"].update(content=VERIFIER.ACQUISITION_REQUEST),
            lambda f: f[3].update(interaction_id="foreign"),
            lambda f: f[3]["payload"]["identity"].update(interaction_id="foreign"),
            lambda f: f.append(third), lambda f: f.append(duplicate_done),
            lambda f: f[3].update(source_event_id=f[1]["source_event_id"]),
            lambda f: f[3]["payload"].update(session_id="foreign"),
            lambda f: f[3]["payload"].update(input={"kind": "content", "content": "another request"}),
            lambda f: f.insert(3, f.pop(2))]
        for index, change in enumerate(mutations):
            with self.subTest(mutation=index):
                wrong = copy.deepcopy(frames); change(wrong)
                with self.assertRaises(AssertionError):
                    c.completed_credential_setup(wrong, accepted, context, business, VERIFIER)

    def test_credential_completion_rejects_tool_effects_and_false_setup_answer(self):
        c = self.console
        frames, accepted, context, business = self.credential_journey()
        setup_call = copy.deepcopy(self.journey()[0][4])
        setup_call.update(id="a-setup", source_event_id="a-setup", run_id="run-A")
        setup_call["payload"]["identity"]["run_id"] = "run-A"
        setup_call["payload"].update(id="original-request", name="find_and_install")
        self.assertIsNotNone(c.completed_credential_setup(frames[:2] + [setup_call] + frames[2:],
                                                          accepted, context, business, VERIFIER))
        for kind, run, name in (("tool_call_requested", "run-B", "tool_catalog_search"),
                                ("tool_result_received", "run-B", "find_and_install"),
                                ("tool_call_requested", "run-A", c.TRELLO_OPERATION),
                                ("tool_result_received", "run-A", c.TRELLO_OPERATION)):
            forbidden = copy.deepcopy(setup_call)
            forbidden.update(kind=kind, run_id=run)
            forbidden["payload"]["identity"]["run_id"] = run
            forbidden["payload"].update(name=name, source_event_type=kind)
            with self.subTest(kind=kind, run=run, name=name), self.assertRaises(AssertionError):
                c.completed_credential_setup(frames[:-1] + [forbidden] + frames[-1:],
                                             accepted, context, business, VERIFIER)
        for field, value in (("answer", "Account access was verified"),
                             ("attached_operation", "another_operation")):
            wrong = {**business, field: value}
            with self.assertRaises(AssertionError):
                c.completed_credential_setup(frames, accepted, context, wrong, VERIFIER)
        wrong = copy.deepcopy(frames); wrong[-1]["payload"]["result"] = "Account access was verified"
        with self.assertRaises(AssertionError):
            c.completed_credential_setup(wrong, accepted, context, business, VERIFIER)
        failed = copy.deepcopy(frames[1]); failed.update(kind="interaction_failed")
        with self.assertRaisesRegex(AssertionError, "actual credential setup original interaction failed"):
            c.completed_credential_setup([failed], accepted, None, None, VERIFIER)

    def test_frame_envelopes_preserve_lineage_and_refuse_malformed_json(self):
        frame = self.journey()[0][-1]
        self.assertEqual(self.console.frame_record({"type": "frame", "frame": frame}), frame)
        self.assertEqual(self.console.frame_record({"kind": "frame_updated", "payload": {"frame": frame}}), frame)
        for value in (None, [], {"type": "frame", "frame": {}}, {"id": "x", "payload": []}):
            with self.assertRaises(AssertionError): self.console.frame_record(value)
        for raw in (b'{"frames":', b'{"frames":[],"frames":[]}'):
            with self.assertRaises((ValueError, AssertionError)):
                VERIFIER.unique_json(raw)


if __name__ == "__main__":
    unittest.main()
