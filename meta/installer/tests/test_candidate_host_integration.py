"""Source-to-worker candidate integration with inert artifacts and no runtime.

Only source, receipts and installed file hashes are exercised. The synthetic
fixture acceptance does not establish application usefulness or native health.
"""
import asyncio
from copy import deepcopy
from dataclasses import asdict
import hashlib
import json
import os
from pathlib import Path
import subprocess
import uuid
import zipfile

import pytest

from launchkit import deployment, deployment_worker as worker
from launchkit import deployment_updates as updates, runtime_dependencies as dependencies
from launchkit import runtime_selection as selection
from launchkit.projects import DraftConflict
from launchkit.templates import install_candidate as adapter, storage_setup
from test_candidate_deployment_updates import crossing, installation, packed
from test_candidate_implementation import (service as implementation_service,
    bundle as implementation_bundle, accepted_ordinary, accepted_fixture, target)
from test_deployment import Service as DeploymentFixture
from test_runtime_candidates import Bundle, PLATFORM, PREFIX


def sha(content):
    return hashlib.sha256(content).hexdigest()


def write_json(path, value, *, private=False):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")
    path.chmod(0o600 if private else 0o644)


def snapshot(root):
    return {str(path.relative_to(root)): (path.read_bytes(), path.stat().st_mode & 0o777)
        for path in root.rglob("*") if path.is_file() and not path.is_symlink()}


def test_saved_selection_parsing_is_pure_when_artifacts_are_unavailable(tmp_path, monkeypatch):
    bundle = Bundle(tmp_path / "external")
    host_target = selection.RuntimeCandidateTarget(bundle.tuple_path, bundle.root, bundle.binding)
    value = selection.candidate_selection(host_target)
    def forbidden(*args, **kwargs):
        pytest.fail("Historical identity parsing must not read current artifacts.")
    monkeypatch.setattr(selection, "read_candidate_artifacts", forbidden)
    assert selection.parse_candidate_selection(value) == host_target


@pytest.mark.parametrize("mutation", ["none", "extra", "accepted-extra", "run-bool", "attempt-zero",
    "source", "tuple-hash", "wheel-hash", "repository", "version", "core", "platform",
    "platform-array", "platform-object", "relative", "dot", "parent", "nul", "long"])
def test_saved_selection_parser_refuses_ambiguous_identity_and_locators(tmp_path, mutation):
    bundle = Bundle(tmp_path / "external")
    value = selection.candidate_selection(selection.RuntimeCandidateTarget(bundle.tuple_path, bundle.root, bundle.binding))
    if mutation == "none": value = None
    elif mutation == "extra": value["verified"] = True
    elif mutation == "accepted-extra": value["accepted"]["verified"] = True
    elif mutation == "run-bool": value["accepted"]["native_run_id"] = True
    elif mutation == "attempt-zero": value["accepted"]["native_run_attempt"] = 0
    elif mutation in {"source", "tuple-hash", "wheel-hash", "repository", "version", "core", "platform",
            "platform-array", "platform-object"}:
        name, replacement = {"source": ("source_sha", "B" * 40), "tuple-hash": ("tuple_sha256", "x" * 64),
            "wheel-hash": ("sdk_wheel_sha256", "f" * 63), "repository": ("repository", "foreign/repo"),
            "version": ("native_version", "0.8.49"), "core": ("core_version", "0.8.52"),
            "platform": ("platform", "unknown-platform"),
            "platform-array": ("platform", []), "platform-object": ("platform", {})}[mutation]
        value["accepted"][name] = replacement
    else:
        value["tuple_path"] = {"relative": "tuple.json", "dot": "/tmp/./tuple.json",
            "parent": "/tmp/../tuple.json", "nul": "/tmp/\x00.json", "long": "/" + "a" * 4096}[mutation]
    with pytest.raises(ValueError): selection.parse_candidate_selection(value)


async def host_installation(tmp_path, implementation_service, monkeypatch):
    first, _ = await accepted_ordinary(implementation_service)
    monkeypatch.setattr(deployment, "require_available_console_port", lambda port: None)
    owner = DeploymentFixture(tmp_path / "deployment-host", implementation_service.guide, implementation_service)
    destination = tmp_path / "actual-source"
    await owner.submit(implementation_service.guide.identity, 3, first["build_id"], str(destination),
        settings={"OPENAI_API_KEY": "synthetic-not-a-provider-key"})
    directory = owner._directory(implementation_service.guide.identity)
    deployment.materialize(directory)
    await owner.shutdown()
    deployment.update_receipt(directory, status="stopped")
    return owner, directory, destination, first


async def accepted_successor(implementation_service, first, implementation_bundle):
    selected = await implementation_service.submit(implementation_service.guide.identity, 3,
        refresh_framework=True, preserve_design=True, expected_selected_build_id=first["build_id"],
        runtime_target=target(implementation_bundle))
    await accepted_fixture(implementation_service, selected)
    return selected


async def test_real_implementation_owner_hands_exact_candidate_to_staging(tmp_path, implementation_service, implementation_bundle, monkeypatch):
    owner, directory, destination, first = await host_installation(tmp_path, implementation_service, monkeypatch)
    async def inert_capability():
        return {"available": False, "detail": "Inert fixture; no service manager is executed."}
    monkeypatch.setattr(owner.service_manager, "capability", inert_capability)
    monkeypatch.setattr(deployment, "observe_local_console_port", lambda *_, **__: None)
    before = snapshot(destination)
    selected = await accepted_successor(implementation_service, first, implementation_bundle)
    options = await owner.options(implementation_service.guide.identity, selected["build_id"])
    assert options["supported"] is True
    assert options["fresh_install"] == {"available": False, "reason": deployment.PRIVATE_RUNTIME_UPDATE_ONLY}
    assert snapshot(destination) == before
    staged = await owner.stage_update(implementation_service.guide.identity,
        deployment.read_receipt(directory)["deployment_id"], 3, selected["build_id"])
    descriptor = staged["update"]["runtime_dependency_update"]
    assert descriptor["runtime_candidate"] == selection.candidate_selection(target(implementation_bundle))
    assert descriptor["from_version"] == "0.8.47" and descriptor["to_version"] == "0.8.48"
    assert staged["update"]["status"] == "staged" and not staged["update"]["blockers"]
    assert staged.get("runtime_candidate") is None and snapshot(destination) == before
    await implementation_service.shutdown()


@pytest.mark.parametrize("mutation", ["no-target", "guide", "build", "source"])
async def test_handoff_refuses_missing_host_target_or_changed_final_fences_before_staging(tmp_path, implementation_service, implementation_bundle, monkeypatch, mutation):
    owner, directory, destination, first = await host_installation(tmp_path, implementation_service, monkeypatch)
    selected = await accepted_successor(implementation_service, first, implementation_bundle)
    before = snapshot(destination), (directory / "package.zip").read_bytes()
    recover = implementation_service.selected_runtime_target
    def changed(identity, **kwargs):
        selected_target = recover(identity, **kwargs)
        if mutation == "no-target": return None
        if mutation == "guide": implementation_service.guide.state["version"] += 1
        elif mutation == "build":
            write_json(implementation_service.root / identity / "latest.json", {"build_id": first["build_id"]}, private=True)
        elif mutation == "source":
            # Retained HMAC source drift, not a changed guide or package marker.
            root = implementation_service._directory(identity, selected["build_id"]) / "workspace/project/application/inert.txt"
            root.write_text("changed authored context\n")
        return selected_target
    monkeypatch.setattr(implementation_service, "selected_runtime_target", changed)
    with pytest.raises((DraftConflict, LookupError, ValueError)):
        await owner.stage_update(implementation_service.guide.identity,
            deployment.read_receipt(directory)["deployment_id"], 3, selected["build_id"])
    assert (snapshot(destination), (directory / "package.zip").read_bytes()) == before
    assert not deployment.read_receipt(directory).get("update")
    await implementation_service.shutdown()


async def test_candidate_cannot_enter_fresh_installation_or_change_destination(tmp_path, implementation_service, implementation_bundle, monkeypatch):
    first, _ = await accepted_ordinary(implementation_service)
    selected = await accepted_successor(implementation_service, first, implementation_bundle)
    owner = DeploymentFixture(tmp_path / "new-deployment-host", implementation_service.guide, implementation_service)
    destination = tmp_path / "not-created"
    async def inert_capability():
        return {"available": False, "detail": "Inert fixture; no service manager is executed."}
    monkeypatch.setattr(owner.service_manager, "capability", inert_capability)
    monkeypatch.setattr(deployment, "observe_local_console_port", lambda *_, **__: None)
    options = await owner.options(implementation_service.guide.identity, selected["build_id"])
    assert options["supported"] is True
    assert options["fresh_install"] == {"available": False, "reason": deployment.PRIVATE_RUNTIME_UPDATE_ONLY}
    with pytest.raises(DraftConflict, match="only through the checked update"):
        await owner.submit(implementation_service.guide.identity, 3, selected["build_id"], str(destination),
            settings={"OPENAI_API_KEY": "synthetic-not-a-provider-key"})
    assert not destination.exists() and not owner.root.exists()
    await implementation_service.shutdown()


def prepared_worker(tmp_path, crossing):
    directory, destination = installation(tmp_path, crossing)
    with (destination / ".env").open("a") as stream: stream.write("MOBKIT_PORT=18889\n")
    _, selected, _, candidate, _, report = crossing
    staged = updates.stage(directory, packed(candidate), report, runtime_target=selected)
    identity = staged["update"]["update_id"]
    return directory, destination, identity


def inert_pair(root, bundle, selected, *, candidate):
    """Write inert fixture bytes manually; never run adapter installation."""
    (root / ".bin").mkdir(mode=0o700)
    (root / ".venv/bin").mkdir(parents=True)
    (root / ".venv/bin/python").write_bytes(b"Inert interpreter; never execute.\n")
    (root / ".venv/bin/python").chmod(0o700)
    if candidate:
        site = root / ".venv/lib/python-fixture/site-packages"
        with zipfile.ZipFile(bundle.wheel_path) as wheel:
            for name in wheel.namelist():
                path = site / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(wheel.read(name))
        common = updates._candidate_binding_fields(selection.candidate_selection(selected))
        sdk = {"format": "forge-installed-candidate-sdk-v1", "source": "local-build-from-exact-frozen-source",
            "signed": False, "published": False, "version": "0.8.48", "site_packages": str(site),
            "payload_sha256": bundle.sdk["source_payload"], "metadata_sha256": {
                name: sha(bundle.wheel_values[PREFIX + name]) for name in ("METADATA", "WHEEL", "top_level.txt")}, **common}
        write_json(root / ".venv/forge-sdk.receipt.json", sdk, private=True)
    for name, prefix in dependencies.BINARIES.items():
        payload = bundle.binary_payloads[name] if candidate else ("inert old " + name).encode()
        binary = root / ".bin" / name
        binary.write_bytes(payload); binary.chmod(0o700)
        if candidate:
            record = next(value for value in bundle.records if value["binary"] == name)
            proof = {"source": "signed-candidate", "version": "0.8.48", "platform": PLATFORM,
                "archive": record["name"], "archive_sha256": record["sha256"],
                "binary_sha256": record["inner"]["sha256"], **common}
        else:
            proof = {"version": "0.8.47", "archive": f"{prefix}-0.8.47-{PLATFORM}.tar.gz",
                "archive_sha256": "f" * 64, "binary_sha256": sha(payload),
                "release_url": "https://github.com/lukacf/meerkat-mobkit/releases/download/v0.8.47"}
        write_json(root / ".bin" / (name + ".receipt.json"), proof, private=True)


def sdk_location_mock(root, events, monkeypatch):
    def locate(argv, **kwargs):
        assert argv[:4] == [str(root / ".venv/bin/python"), "-I", "-S", "-c"]
        assert argv[-1] == str(root / ".venv")
        assert "sysconfig.get_paths" in argv[4] and "sys.argv[1]" in argv[4]
        events.append("sdk-files")
        site = str(root / ".venv/lib/python-fixture/site-packages")
        return subprocess.CompletedProcess(argv, 0, json.dumps({"purelib": site, "platlib": site}))
    monkeypatch.setattr(adapter.subprocess, "run", locate)
    monkeypatch.setattr(dependencies, "platform_target", lambda: PLATFORM)
    for name in adapter.OVERRIDES: monkeypatch.delenv(name, raising=False)


@pytest.mark.parametrize("late_drift", [None, "sdk", "native", "selection", "override"])
async def test_worker_stopped_crossing_uses_full_actual_file_proof_and_preserves_forward_state(tmp_path, crossing, monkeypatch, late_drift):
    directory, root, identity = prepared_worker(tmp_path, crossing)
    bundle, selected, _, candidate, _, _ = crossing
    inert_pair(root, bundle, selected, candidate=False)
    definition = json.loads(candidate["deployment.json"][0])
    marker = {"format": 1, "project": definition["slug"], "version": "0.8.47", "phase": "ready"}
    write_json(root / ".mobkit/launch-storage.json", marker, private=True)
    private = (root / ".env").read_bytes()
    retained_native, retained_app = (root / ".mobkit/state/native.db").read_bytes(), (root / "data/ledger.db").read_bytes()
    events = []
    sdk_location_mock(root, events, monkeypatch)
    def upgrade(path, old, new, *, require_owner_current):
        assert path == root and (old, new) == ("0.8.47", "0.8.48")
        assert callable(require_owner_current)
        require_owner_current()
        events.append("storage")
        write_json(root / ".mobkit/launch-storage.json", {**marker, "version": new}, private=True)
        return {"from_version": old, "to_version": new, "applied": True}
    monkeypatch.setattr(storage_setup, "upgrade_storage", upgrade)
    class InertSupervisor(worker.Supervisor):
        async def native_definition_revision(self, *args, **kwargs):
            pytest.fail("An unchanged .47 candidate crossing cannot declare a new definition.")
        async def command(self, argv, phase, *args, **kwargs):
            if argv[:2] == ["/bin/bash", "install.sh"]:
                assert argv == ["/bin/bash", "install.sh", "--non-interactive"]
                assert not (root / ".bin").exists() and not (root / ".venv").exists()
                events.append("install-fixture")
                inert_pair(root, bundle, selected, candidate=True)
                (root / ".mobkit/state/native.db").write_bytes(retained_native + b"; observed")
                (root / "data/ledger.db").write_bytes(retained_app + b"; observed")
                return ""
            assert argv[-1] in {"--doctor", "--application-check"}
            events.append(argv[-1])
            if argv[-1] == "--application-check" and late_drift:
                if late_drift == "sdk":
                    path = next((root / ".venv/lib/python-fixture/site-packages/meerkat_mobkit").glob("*.py"))
                    path.write_bytes(path.read_bytes() + b"# late changed SDK\n")
                elif late_drift == "native": (root / ".bin/rpc_gateway").write_bytes(b"changed executable")
                elif late_drift == "selection":
                    value = json.loads((root / "runtime-candidate.json").read_text())
                    value["accepted"]["native_run_attempt"] += 1
                    write_json(root / "runtime-candidate.json", value)
                else:
                    with (root / ".env").open("a") as stream: stream.write("MOBKIT_RPC_GATEWAY_BIN=/foreign/native\n")
        async def start(self, definition, *, on_ready=None):
            events.append("start-fixture")
            assert self._candidate_runtime_target(candidate) == selected
            proof = self._verify_candidate_runtime(candidate)
            assert proof == deployment.read_receipt(directory)["update"]["runtime_artifacts"]["native"]
            assert on_ready is not None
            on_ready()
    supervisor = InertSupervisor(directory)
    if late_drift:
        with pytest.raises((RuntimeError, DraftConflict)):
            await worker.update_runtime(supervisor, "update", update_id=identity)
        current = deployment.read_receipt(directory)
        assert current["update"]["status"] == "recovery-needed" and not current["update"]["rollback_available"]
        assert "start-fixture" not in events
    else:
        await worker.update_runtime(supervisor, "update", update_id=identity)
        current, archive, saved = updates.transaction(directory, identity)
        assert saved["status"] == "completed" and events.count("start-fixture") == 1
        assert events.index("sdk-files") < events.index("storage") < events.index("--doctor") < events.index("start-fixture")
        assert saved["runtime_artifacts"]["sdk"] == saved["runtime_artifacts"]["native"]["sdk"]
        assert saved.get("native_definition_activation") is None
        assert (archive / "runtime-before/.bin/rpc_gateway").read_bytes() == b"inert old rpc_gateway"
    assert current["runtime_candidate"] == selection.candidate_selection(selected)
    assert (root / ".mobkit/state/native.db").read_bytes() == retained_native + b"; observed"
    assert (root / "data/ledger.db").read_bytes() == retained_app + b"; observed"
    if late_drift != "override": assert (root / ".env").read_bytes() == private


def prepared_candidate_restart(tmp_path, crossing, monkeypatch):
    """Verify valid installed inert files before testing a restart boundary."""
    directory, root, identity = prepared_worker(tmp_path, crossing)
    bundle, selected, _, candidate, _, _ = crossing
    updates.activate(directory, identity)
    inert_pair(root, bundle, selected, candidate=True)
    events = []
    sdk_location_mock(root, events, monkeypatch)
    supervisor = worker.Supervisor(directory)
    proof = supervisor._verify_candidate_runtime(candidate)
    assert proof["sdk"]["payload_sha256"] == bundle.sdk["source_payload"]
    assert proof["candidate_binding"] == asdict(selected.accepted)
    assert set(proof["binaries"]) == set(dependencies.BINARIES)
    assert events and set(events) == {"sdk-files"}
    events.clear()
    return directory, root, supervisor, events


async def test_restart_valid_candidate_reaches_port_preflight_before_process(tmp_path, crossing, monkeypatch):
    directory, root, supervisor, events = prepared_candidate_restart(tmp_path, crossing, monkeypatch)
    _, _, _, candidate, _, _ = crossing
    definition = json.loads(candidate["deployment.json"][0])

    class PortPreflightReached(Exception):
        pass

    def port_preflight(port):
        assert port == 18889
        events.append("port-preflight")
        raise PortPreflightReached("Valid inert candidate reached the port boundary.")

    def forbidden(*args, **kwargs):
        pytest.fail("The positive control stops before process creation.")

    monkeypatch.setattr(worker, "require_available_console_port", port_preflight)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", forbidden)
    before = snapshot(root), snapshot(directory)
    with pytest.raises(PortPreflightReached):
        await supervisor.start(definition)
    assert events[-1] == "port-preflight" and events.count("port-preflight") == 1
    assert "sdk-files" in events[:-1], "Startup rechecks the actual installed SDK before the port boundary."
    assert (snapshot(root), snapshot(directory)) == before


@pytest.mark.parametrize("mutation", ["no-carrier", "foreign-carrier", "selection-bool", "framework-bool", "source", "override", "old-definition"])
async def test_restart_refuses_unbound_candidate_before_port_or_process(tmp_path, crossing, monkeypatch, mutation):
    directory, root, supervisor, events = prepared_candidate_restart(tmp_path, crossing, monkeypatch)
    _, _, previous, candidate, _, _ = crossing
    if mutation == "no-carrier":
        receipt = deployment.read_receipt(directory); receipt.pop("runtime_candidate")
        write_json(directory / "receipt.json", receipt, private=True)
    elif mutation == "foreign-carrier":
        receipt = deployment.read_receipt(directory); receipt["runtime_candidate"]["accepted"]["source_sha"] = "b" * 40
        write_json(directory / "receipt.json", receipt, private=True)
    elif mutation in {"selection-bool", "framework-bool"}:
        name = "runtime-candidate.json" if mutation == "selection-bool" else "implementation-report.json"
        value = json.loads(candidate[name][0])
        claim = value if mutation == "selection-bool" else value["framework"]["runtime_candidate"]
        claim["accepted"]["native_run_attempt"] = True
        candidate = {**candidate, name: (json.dumps(value).encode(), candidate[name][1])}
        write_json(root / name, value)
    elif mutation == "source": (root / "app.py").write_text("unaccepted host source\n")
    elif mutation == "override":
        with (root / ".env").open("a") as stream: stream.write("MOBKIT_RPC_GATEWAY_BIN=/foreign/native\n")
    def forbidden(*args, **kwargs): pytest.fail("No port probe or process can precede candidate refusal.")
    monkeypatch.setattr(worker, "require_available_console_port", forbidden)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", forbidden)
    definition = json.loads((previous if mutation == "old-definition" else candidate)["deployment.json"][0])
    refusal = {
        "no-carrier": "no previously admitted host runtime selection",
        "foreign-carrier": "differs from its exact accepted package",
        "selection-bool": "Installed source differs from the checked package",
        "framework-bool": "Installed source differs from the checked package",
        "source": "Installed source differs from the checked package",
        "override": "cannot use native gateway overrides",
        "old-definition": "startup definition differs from its checked package",
    }[mutation]
    before = snapshot(root), snapshot(directory)
    with pytest.raises((RuntimeError, DraftConflict, ValueError), match=refusal):
        await supervisor.start(definition)
    assert not events, "Source/selection/override refusal precedes installed-runtime probing."
    assert (snapshot(root), snapshot(directory)) == before


async def test_worker_fresh_candidate_refusal_precedes_supervisor_and_materialize(tmp_path, crossing, monkeypatch):
    directory, root = installation(tmp_path, crossing)
    _, _, _, candidate, _, _ = crossing
    payload = packed(candidate)
    (directory / "package.zip").write_bytes(payload)
    deployment.update_receipt(directory, package_sha256=sha(payload))
    before = snapshot(root), snapshot(directory)
    def forbidden(*args, **kwargs): pytest.fail("Fresh candidate refusal must precede construction and source copy.")
    monkeypatch.setattr(worker, "Supervisor", forbidden)
    monkeypatch.setattr(worker, "materialize", forbidden)
    with pytest.raises(RuntimeError, match="fresh installation is not admitted"):
        await worker.run(directory, "install")
    assert (snapshot(root), snapshot(directory)) == before
