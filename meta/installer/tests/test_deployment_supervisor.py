"""Inert normal-owner supervisor publication and exact crash recovery.

All service, application, SDK and native execution is forbidden. The only
manager interaction is a synthetic read-only stopped-state observation.
"""
import asyncio
from copy import deepcopy
from dataclasses import asdict
import json
import os
from pathlib import Path
import subprocess
import uuid
import zipfile

import pytest

from launchkit import deployment, deployment_service as services
from launchkit import deployment_supervisor as refresh
from launchkit import deployment_updates as updates
from launchkit.projects import DraftConflict, fingerprint
from historical_runtime_fixture import retained_candidate_v1
from launchkit.templates.storage_setup import project_lock
from test_candidate_deployment_updates import (candidate_proof, crossing, installation,
    native_fixture_proof, packed)


class Manager:
    def __init__(self):
        self.observation = {"loaded": False, "pid": None, "state": "stopped"}
        self.calls = []

    async def observe(self, service):
        self.calls.append(service["label"])
        return deepcopy(self.observation)

    async def start(self, service):
        pytest.fail("Supervisor refresh cannot start a service.")

    async def stop(self, service):
        pytest.fail("Supervisor refresh cannot stop a service.")


def snapshot(root):
    return {str(p.relative_to(root)): (p.read_bytes(), p.stat().st_mode & 0o777)
        for p in root.rglob("*") if p.is_file() and not p.is_symlink()}


def rewrite(path, value):
    mode = path.stat().st_mode & 0o777
    path.chmod(0o600)
    path.write_bytes(value if isinstance(value, bytes) else json.dumps(value).encode())
    path.chmod(mode)


@pytest.fixture
def trusted_supervisor_sources():
    return services.supervisor_source_files()


@pytest.fixture
def owned(tmp_path, crossing, monkeypatch, trusted_supervisor_sources):
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    Path.home().mkdir(mode=0o700)
    directory, destination = installation(tmp_path, crossing)
    receipt = deployment.read_receipt(directory)
    manager = Manager()
    owner = deployment.DeploymentService(tmp_path / "owner", None, None, service_manager=manager)
    selected = owner.root / receipt["guide_id"] / receipt["deployment_id"]
    selected.parent.mkdir(mode=0o700, parents=True)
    directory.rename(selected)
    directory = selected
    (selected.parent / "latest.json").write_text(json.dumps({"deployment_id": receipt["deployment_id"]}))
    (selected.parent / "latest.json").chmod(0o600)
    deployment.update_receipt(directory, execution_mode="background", status="stopped",
        service_supervisor_pid=1234, service_instance_id="synthetic-cleaned-instance",
        service_cleanup_confirmed=True, runtime_pid=None, process_live=False)
    launcher = trusted_supervisor_sources["launcher.py"]
    old = {"launcher.py": launcher, "launchkit/deployment_worker.py": b"raise RuntimeError('Old host cannot execute')\n"}
    old.update({f"launchkit/old_{n}.py": b"raise RuntimeError('Old module cannot execute')\n" for n in range(62)})
    monkeypatch.setattr(services, "supervisor_source_files", lambda: deepcopy(old))
    service = services.prepare_service(directory, deployment.read_receipt(directory))
    # Exact retained legacy unit: absent metadata renders the old 45s budget.
    service.pop("stop_timeout_seconds")
    service.pop("candidate_startup_protocol")
    service.pop("candidate_worker_sha256")
    service["public_summary"] = {"arbitrary": ["display only", {"legacy": True}]}
    unit = services.unit_bytes(service)
    Path(service["unit_path"]).write_bytes(unit)
    service["unit_sha256"] = services.digest(unit)
    deployment.update_receipt(directory, service=service)
    assert services.verify_service(directory, deployment.read_receipt(directory)) == service
    current = {"launcher.py": launcher, "launchkit/deployment_worker.py": b"raise RuntimeError('New host cannot execute')\n",
        "launchkit/__init__.py": b"raise RuntimeError('Host import forbidden')\n",
        "launchkit/runtime_selection.py": b"raise RuntimeError('Selection import forbidden')\n",
        "launchkit/runtime_dependencies.py": b"raise RuntimeError('Dependency import forbidden')\n",
        "launchkit/runtime_candidates.py": b"raise RuntimeError('Candidate import forbidden')\n",
        "launchkit/sdk_wheel_integrity.py": b"raise RuntimeError('Wheel helper import forbidden')\n",
        "launchkit/deployment_supervisor.py": b"raise RuntimeError('Custody import forbidden')\n",
        "launchkit/templates/install_candidate.py": b"raise RuntimeError('Installer import forbidden')\n",
        "launchkit/blueprint-limits.json": b"{}\n"}
    monkeypatch.setattr(services, "supervisor_source_files", lambda: deepcopy(current))
    with project_lock(destination):
        pass

    def forbidden(*args, **kwargs):
        pytest.fail("No process, SDK, native binary or OS manager may execute.")

    monkeypatch.setattr(asyncio, "create_subprocess_exec", forbidden)
    monkeypatch.setattr(os, "system", forbidden)
    return owner, manager, directory, destination, service, old, current


@pytest.fixture
def current_admission_owned(owned, monkeypatch, trusted_supervisor_sources):
    """Migrate inert custody before admitting today's exact candidate closure."""
    owner, manager, directory, destination, service, old, _ = owned
    before, unit = snapshot(destination), Path(service["unit_path"]).read_bytes()
    monkeypatch.setattr(services, "supervisor_source_files", lambda: deepcopy(trusted_supervisor_sources))
    result = refresh.prepare(directory, refresh.service_digest(service))
    asyncio.run(refresh.apply(directory, result["revision"], manager,
        expected_digest=result["descriptor_sha256"]))
    root = directory / "supervisor-refreshes" / result["revision"]
    assert {name: content for name, (content, _) in snapshot(root / "before/host").items()} == old
    assert snapshot(destination) == before and Path(service["unit_path"]).read_bytes() == unit
    current_service = services.verify_service(directory, deployment.read_receipt(directory))
    assert current_service["host_files"] == {
        name: services.digest(content) for name, content in trusted_supervisor_sources.items()}
    future = deepcopy(trusted_supervisor_sources)
    future["launchkit/deployment_supervisor.py"] += b"# Inert future supervisor fixture revision\n"
    monkeypatch.setattr(services, "supervisor_source_files", lambda: deepcopy(future))
    manager.calls.clear()
    return owner, manager, directory, destination, current_service, trusted_supervisor_sources, future


def prepared(owned):
    _, _, directory, _, service, _, _ = owned
    result = refresh.prepare(directory, refresh.service_digest(service))
    root = directory / "supervisor-refreshes" / result["revision"]
    return result, root


def support_prepare_fields(service):
    return {"expected_service_digest": refresh.service_digest(service)}


def support_apply_fields(selected):
    return {key: selected[key] for key in ("revision", "descriptor_sha256")}


@pytest.mark.parametrize("action", ["prepare-support", "apply-support"])
@pytest.mark.parametrize("outcome", [None, "returned", "error"])
async def test_support_command_observation_is_metadata_only(owned, monkeypatch, action, outcome):
    owner, manager, directory, _, service, _, _ = owned
    request_id = str(uuid.uuid4())
    fields = {"expected_service_digest": refresh.service_digest(service),
        "revision": None, "descriptor_sha256": None} if action == "prepare-support" else {
        "expected_service_digest": None, "revision": str(uuid.uuid4()), "descriptor_sha256": "a" * 64}
    binding = {"guide_id": directory.parent.name, "deployment_id": directory.name,
        "action_request_id": request_id, "action": action, "update_id": None, **fields}
    value, claimed = owner._claim_action_request(binding)
    assert claimed
    if outcome:
        value = owner._settle_action_request(binding, outcome)
    before, calls = snapshot(owner.root), list(manager.calls)

    def forbidden(*args, **kwargs):
        pytest.fail("Command settlement must not validate or observe the installation.")

    monkeypatch.setattr(refresh, "support_projection", forbidden)
    monkeypatch.setattr(deployment, "read_receipt", forbidden)
    monkeypatch.setattr(manager, "observe", forbidden)
    observed = await owner.action_request(directory.parent.name, directory.name, request_id)
    assert observed == {"action_request": value}
    assert snapshot(owner.root) == before and manager.calls == calls
    with pytest.raises(DraftConflict, match="different deployment action"):
        await owner.action_request(str(uuid.uuid4()), directory.name, request_id)
    with pytest.raises(DraftConflict, match="different deployment action"):
        await owner.action_request(directory.parent.name, str(uuid.uuid4()), request_id)


async def test_support_action_owner_prepares_reviews_and_applies_without_runtime_effects(owned, monkeypatch):
    owner, manager, directory, destination, service, _, current = owned
    request_id = str(uuid.uuid4())
    original, unit = snapshot(destination), Path(service["unit_path"]).read_bytes()
    result = await owner.action(directory.parent.name, directory.name, "prepare-support",
        action_request_id=request_id, **support_prepare_fields(service))
    selected = result["support_request"]["selected"]
    assert selected["revision"] == request_id and selected["phase"] == "prepared"
    assert result["action_request"] == {"version": 1, "guide_id": directory.parent.name,
        "deployment_id": directory.name, "action_request_id": request_id, "action": "prepare-support",
        "update_id": None, "expected_service_digest": refresh.service_digest(service),
        "revision": None, "descriptor_sha256": None, "phase": "settled", "outcome": "returned"}
    before = snapshot(directory)
    def forbidden(*args, **kwargs):
        pytest.fail("Pending support status cannot reconcile ordinary background health.")
    monkeypatch.setattr(services, "background_status", forbidden)
    status = await owner.status(directory.parent.name)
    observation = await owner.action_request(directory.parent.name, directory.name, request_id)
    assert status["support_request"]["selected"] == selected
    assert status["status"] == "stopped" and status["service_cleanup_confirmed"] is True
    assert status["support_request"]["stopped_verified"] is True
    assert observation == {"action_request": result["action_request"]}
    assert snapshot(directory) == before  # Both GETs are strictly read-only.
    applied = await owner.action(directory.parent.name, directory.name, "apply-support",
        action_request_id=str(uuid.uuid4()), **support_apply_fields(selected))
    assert applied["support_request"]["selected"]["phase"] == "completed"
    assert applied["support_request"]["stopped_verified"] is True
    assert not applied["support_request"]["apply_available"]
    assert applied["status"] == "stopped" and applied["runtime_pid"] is None
    assert snapshot(destination) == original and Path(service["unit_path"]).read_bytes() == unit
    assert refresh._files(directory / "service-host")[0] == current
    settled = snapshot(directory)
    await owner.action(directory.parent.name, directory.name, "apply-support",
        action_request_id=applied["action_request"]["action_request_id"], **support_apply_fields(selected))
    assert snapshot(directory) == settled


async def test_cold_pending_prepare_requires_explicit_same_request_and_reuses_frozen_custody(owned, monkeypatch):
    owner, manager, directory, _, service, _, _ = owned
    request_id = str(uuid.uuid4())
    async def lost(*args):
        raise asyncio.CancelledError
    monkeypatch.setattr(owner, "_support_result_locked", lost)
    with pytest.raises(asyncio.CancelledError):
        await owner.action(directory.parent.name, directory.name, "prepare-support",
            action_request_id=request_id, **support_prepare_fields(service))
    selected = deployment.read_receipt(directory)["supervisor_refresh"]
    before = snapshot(directory)
    cold = deployment.DeploymentService(owner.root.parent, None, None, service_manager=manager)
    duplicate = await cold.action(directory.parent.name, directory.name, "prepare-support",
        action_request_id=request_id, **support_prepare_fields(service))
    assert duplicate["action_request"]["phase"] == "pending" and snapshot(directory) == before
    monkeypatch.setattr(services, "supervisor_source_files", lambda: pytest.fail("Prepared source must remain frozen."))
    resumed = await cold.action(directory.parent.name, directory.name, "prepare-support",
        action_request_id=request_id, resume=True, **support_prepare_fields(service))
    assert resumed["action_request"]["phase"] == "settled"
    assert deployment.read_receipt(directory)["supervisor_refresh"] == selected
    assert [path.name for path in (directory / "supervisor-refreshes").iterdir()] == [request_id]
    assert snapshot(directory) == before


@pytest.mark.parametrize("interruption", ["retained", "completed"])
async def test_cold_pending_apply_resumes_only_its_frozen_revision(owned, monkeypatch, interruption):
    owner, manager, directory, destination, service, _, current = owned
    selected, root = prepared(owned)
    request_id = str(uuid.uuid4())
    original, unit = snapshot(destination), Path(service["unit_path"]).read_bytes()
    phase = refresh._phase
    def interrupted(root, journal, value):
        changed = phase(root, journal, value)
        if value == interruption:
            raise asyncio.CancelledError
        return changed
    monkeypatch.setattr(refresh, "_phase", interrupted)
    with pytest.raises(asyncio.CancelledError):
        await owner.action(directory.parent.name, directory.name, "apply-support",
            action_request_id=request_id, **support_apply_fields(selected))
    monkeypatch.setattr(refresh, "_phase", phase)
    cold = deployment.DeploymentService(owner.root.parent, None, None, service_manager=manager)
    before = snapshot(directory)
    observation = await cold.action_request(directory.parent.name, directory.name, request_id)
    assert observation["action_request"]["phase"] == "pending" and snapshot(directory) == before
    changed = deepcopy(current); changed["launchkit/later.py"] = b"Never selected for this custody.\n"
    monkeypatch.setattr(services, "supervisor_source_files", lambda: changed)
    resumed = await cold.action(directory.parent.name, directory.name, "apply-support",
        action_request_id=request_id, resume=True, **support_apply_fields(selected))
    assert resumed["action_request"]["phase"] == "settled"
    assert resumed["support_request"]["selected"]["phase"] == "completed"
    assert resumed["support_request"]["phase"] == "completed"
    assert resumed["support_request"]["prepare_available"] is True
    assert refresh._files(directory / "service-host")[0] == current
    assert snapshot(destination) == original and Path(service["unit_path"]).read_bytes() == unit


@pytest.mark.parametrize("action,changed", [("prepare-support", "expected_service_digest"),
    ("apply-support", "revision"), ("apply-support", "descriptor_sha256")])
async def test_support_request_binding_cannot_drift_during_cold_continuation(owned, monkeypatch, action, changed):
    owner, _, directory, _, service, _, _ = owned
    selected, _ = prepared(owned) if action == "apply-support" else (None, None)
    fields = support_prepare_fields(service) if selected is None else support_apply_fields(selected)
    async def lost(*args):
        raise asyncio.CancelledError
    monkeypatch.setattr(owner, "_support_result_locked", lost)
    request_id = str(uuid.uuid4())
    with pytest.raises(asyncio.CancelledError):
        await owner.action(directory.parent.name, directory.name, action, action_request_id=request_id, **fields)
    fields[changed] = str(uuid.uuid4()) if changed == "revision" else "f" * 64
    before, calls = snapshot(directory), list(owned[1].calls)
    with pytest.raises(DraftConflict, match="different deployment action"):
        await owner.action(directory.parent.name, directory.name, action,
            action_request_id=request_id, resume=True, **fields)
    assert snapshot(directory) == before and owned[1].calls == calls


async def test_support_missing_request_and_partial_unselected_preparation_never_get_replaced(owned):
    owner, _, directory, _, service, _, _ = owned
    request_id = str(uuid.uuid4())
    with pytest.raises(LookupError, match="not been recorded"):
        await owner.action(directory.parent.name, directory.name, "prepare-support",
            action_request_id=request_id, resume=True, **support_prepare_fields(service))
    store = directory / "supervisor-refreshes"
    store.mkdir(mode=0o700); root = store / request_id; root.mkdir(mode=0o700)
    (root / "partial").write_text("Inert retained interrupted preparation\n")
    before = snapshot(directory)
    with pytest.raises(DraftConflict, match="interrupted before selection"):
        await owner.action(directory.parent.name, directory.name, "prepare-support",
            action_request_id=request_id, **support_prepare_fields(service))
    assert snapshot(directory) == before and list(store.iterdir()) == [root]


@pytest.mark.parametrize("action", ["prepare-support", "apply-support"])
async def test_support_late_guide_selection_change_refuses_before_custody_effect(owned, monkeypatch, action):
    owner, manager, directory, _, service, _, _ = owned
    selected, _ = prepared(owned) if action == "apply-support" else (None, None)
    before = snapshot(directory)
    async def changed(service):
        (directory.parent / "latest.json").write_text(json.dumps({"deployment_id": str(uuid.uuid4())}))
        return deepcopy(manager.observation)
    monkeypatch.setattr(manager, "observe", changed)
    fields = support_prepare_fields(service) if selected is None else support_apply_fields(selected)
    with pytest.raises(DraftConflict, match="newer deployment"):
        await owner.action(directory.parent.name, directory.name, action,
            action_request_id=str(uuid.uuid4()), **fields)
    assert snapshot(directory) == before


@pytest.mark.parametrize("mutation", ["unit", "cleanup", "live-manager", "manager-error"])
async def test_completed_support_history_does_not_hide_current_refusal(owned, monkeypatch, mutation):
    _, manager, directory, _, service, _, _ = owned
    selected, _ = prepared(owned)
    await refresh.apply(directory, selected["revision"], manager, expected_digest=selected["descriptor_sha256"])
    if mutation == "unit": Path(service["unit_path"]).write_bytes(b"Foreign service unit\n")
    elif mutation == "cleanup": deployment.update_receipt(directory, service_cleanup_confirmed=False)
    elif mutation == "live-manager": manager.observation = {"loaded": True, "pid": 2222, "state": "running"}
    else:
        async def failed(service):
            raise OSError("Sensitive synthetic manager output must not be exposed")
        monkeypatch.setattr(manager, "observe", failed)
    before = snapshot(directory)
    value = await refresh.support_projection(directory, manager)
    assert not value["available"] and not value["stopped_verified"] and value["reason"]
    assert "Sensitive" not in value["reason"] and snapshot(directory) == before


def inert_installed_candidate(owned, crossing, monkeypatch):
    """Manually populate tmp bytes; only the isolated stdlib query is mocked."""
    from launchkit import runtime_dependencies as runtime
    from launchkit.templates import install_candidate as adapter
    from test_runtime_candidates import PLATFORM, PREFIX
    bundle, _, _, _, _, _ = crossing
    destination = owned[3]
    site = destination / ".venv/lib/python3.14/site-packages"
    site.mkdir(mode=0o700, parents=True)
    (destination / ".venv/bin").mkdir(mode=0o700)
    python = destination / ".venv/bin/python"
    python.write_bytes(b"Inert selected interpreter; no execution permitted.\n")
    python.chmod(0o700)
    with zipfile.ZipFile(bundle.wheel_path) as wheel:
        for name in wheel.namelist():
            path = site / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(wheel.read(name))
    selection = deployment.read_receipt(owned[2])["runtime_candidate"]
    common = updates._candidate_binding_fields(selection)
    sdk = {"format": "forge-installed-candidate-sdk-v1", "source": "local-build-from-exact-frozen-source",
        "signed": False, "published": False, "version": "0.8.48", "site_packages": str(site),
        "payload_sha256": dict(bundle.sdk["source_payload"]),
        "metadata_sha256": {name: updates.digest(bundle.wheel_values[PREFIX + name])
            for name in ("METADATA", "WHEEL", "top_level.txt")}, **common}
    (destination / ".bin").mkdir(mode=0o700)
    for record in bundle.records:
        name = record["binary"]
        path = destination / ".bin" / name
        path.write_bytes(bundle.binary_payloads[name]); path.chmod(0o700)
        path = destination / ".bin" / (name + ".receipt.json")
        path.write_text(json.dumps({"source": "signed-candidate", "version": "0.8.48", "platform": PLATFORM,
            "archive": record["name"], "archive_sha256": record["sha256"],
            "binary_sha256": record["inner"]["sha256"], **common}))
        path.chmod(0o600)
    receipt = destination / ".venv/forge-sdk.receipt.json"
    receipt.write_text(json.dumps(sdk)); receipt.chmod(0o600)
    calls = []
    def stdlib_location(argv, **kwargs):
        assert argv[:4] == [str(python), "-I", "-S", "-c"]
        assert argv[4].startswith("import json, sysconfig, sys;") and argv[5:] == [str(destination / ".venv")]
        assert kwargs == {"check": True, "capture_output": True, "text": True,
            "timeout": adapter.STDLIB_CHECK_TIMEOUT}
        calls.append(argv)
        return subprocess.CompletedProcess(argv, 0, json.dumps({"purelib": str(site), "platlib": str(site)}))
    monkeypatch.setattr(adapter.subprocess, "run", stdlib_location)
    monkeypatch.setattr(runtime, "platform_target", lambda: PLATFORM)
    def forbidden(*args, **kwargs):
        pytest.fail("No acquisition, installation, SDK/native execution or bytecode compilation is permitted.")
    for name in ("install_candidate_sdk", "install_candidate_requirements", "install_candidate_native_tools", "check_bytecode"):
        monkeypatch.setattr(adapter, name, forbidden)
    monkeypatch.setattr(runtime, "retain_runtime", forbidden)
    for name in adapter.OVERRIDES:
        monkeypatch.delenv(name, raising=False)
    return sdk, calls


def admit_candidate(owned, crossing, monkeypatch):
    _, _, directory, destination, _, _, _ = owned
    _, target, _, candidate, _, report = crossing
    staged = updates.stage(directory, packed(candidate), report, runtime_target=target)
    identity = staged["update"]["update_id"]
    updates.activate(directory, identity)
    _, root, update = updates.transaction(directory, identity)
    # Preserve the actual admitted v4 package, fixed inventory and source pins.
    # Only the installed proof is inert fixture authority, never native work.
    sdk, _ = inert_installed_candidate(owned, crossing, monkeypatch)
    _, native = candidate_proof(update["runtime_dependency_update"], destination)
    native["sdk"] = deepcopy(sdk)
    for name in native["binaries"]:
        native["binaries"][name]["receipt_sha256"] = updates.digest(
            (destination / ".bin" / (name + ".receipt.json")).read_bytes())
        update["runtime_dependency_update"]["runtime_candidate_artifacts"]["native"][name]["receipt_sha256"] = (
            native["binaries"][name]["receipt_sha256"])
    updates.checkpoint(directory, root, update, status="completed", blockers=[],
        rollback_available=False, runtime_admission_possible=True,
        runtime_files_retained={"format": "mobkit-launch-runtime-retention-v1", "retained": True,
            "inventory_sha256": "e" * 64, "destination": str(destination),
            "backup": str(root / "runtime-before"), "previous_native": native_fixture_proof("0.8.47")},
        runtime_artifacts={"sdk": sdk, "native": native})
    deployment.update_receipt(directory, startup_candidate_runtime_proof=native)
    return root


@pytest.fixture
def candidate_owned(current_admission_owned, crossing, monkeypatch):
    owned = current_admission_owned
    admit_candidate(owned, crossing, monkeypatch)

    def forbidden(*args, **kwargs):
        pytest.fail("Supervisor refresh must not compile, acquire, install or admit candidate artifacts.")

    from launchkit import compiler
    monkeypatch.setattr(compiler, "generate_package", forbidden)
    monkeypatch.setattr(compiler, "generate_runtime_refresh", forbidden)
    monkeypatch.setattr(updates, "_candidate_artifact_expectation", forbidden)
    return owned


def maintained_fixture_update(owned, crossing):
    """One real source crossing through the normal inert update owner."""
    from launchkit.compiler import MAINTAINED_HOST_PATHS, maintained_host_export
    directory = owned[2]
    receipt = deployment.read_receipt(directory)
    _, previous = updates.checked_package(directory, "package.zip", receipt["package_sha256"])
    candidate = deepcopy(previous)
    for item in maintained_host_export():
        candidate[item.path] = item.as_bytes(), item.mode
    old = json.loads(previous["implementation-report.json"][0])
    report = deepcopy(old)
    binding = {"build_id": old["build_id"], "source_digest": old["verification"]["digest"]}
    report.update(build_id=str(uuid.uuid4()), verification={"digest": "a" * 64})
    names = sorted(set(updates._archived_framework_paths(previous)) | set(MAINTAINED_HOST_PATHS))
    inventory = {name: {"sha256": updates.digest(candidate[name][0]),
        "mode": 0o600 if name == ".env.example" else candidate[name][1]} for name in names}
    sums = "".join(f"{inventory[name]['sha256']}  {name}\n" for name in names).encode()
    inventory["SHA256SUMS"] = {"sha256": updates.digest(sums), "mode": 0o644}
    report["framework"].update(previous=binding, origin=binding, baseline_digest=fingerprint(inventory))
    candidate["implementation-report.json"] = json.dumps(report).encode(), 0o644
    candidate["SHA256SUMS"] = sums, 0o644
    result = updates.stage(directory, packed(candidate), report, runtime_target=crossing[1])
    update = result["update"]
    assert update["runtime_dependency_update"] is None and update["maintained_host_update"] is not None
    updates.activate(directory, update["update_id"])
    root = updates.update_directory(directory, update["update_id"])
    update = json.loads((root / "update.json").read_text())
    updates.checkpoint(directory, root, update, status="completed", blockers=[], rollback_available=False,
        runtime_admission_possible=True)
    return root


def next_supervisor(owned, monkeypatch):
    current = deepcopy(services.supervisor_source_files())
    current["launchkit/deployment_supervisor.py"] += b"# Another inert supervisor source revision\n"
    monkeypatch.setattr(services, "supervisor_source_files", lambda: deepcopy(current))


def current_prepared(owned):
    directory = owned[2]
    service = deployment.read_receipt(directory)["service"]
    result = refresh.prepare(directory, refresh.service_digest(service))
    return result, directory / "supervisor-refreshes" / result["revision"]


@pytest.fixture
def historical_candidate_crossing(tmp_path, monkeypatch, retained_candidate_v1):
    """Emit original v1 inventory with synthetic spec and artifact identity."""
    from test_runtime_candidates import Bundle
    retained = retained_candidate_v1
    bundle = Bundle(tmp_path / "synthetic-v1-artifacts")
    target = retained.selection.RuntimeCandidateTarget(bundle.tuple_path, bundle.root,
        retained.candidates.CandidateBinding(**asdict(bundle.binding)))
    from launchkit.runtime_selection import RuntimeCandidateTarget
    current_target = RuntimeCandidateTarget(bundle.tuple_path, bundle.root, bundle.binding)
    spec = retained.blueprint.starter_blueprint().model_dump(mode="json")
    spec["project"].update(name="Inert historical fixture", slug="inert-historical-fixture",
        description="Synthetic source-only candidate custody test.")
    spec["knowledge"]["mode"] = "none"
    for role in spec["agent_classes"]:
        role.update(provider="openai", model="gpt-6-astra")
        role["tools"]["memory"] = False
    ordinary_old = retained.compiler.generate_package(spec)
    assert retained.compiler.MOBKIT_VERSION == retained.generator.MOBKIT_VERSION == "0.8.47"
    baseline_old = {item.path: {"sha256": updates.digest(item.content.encode()), "mode": item.mode}
        for item in ordinary_old}
    previous = {item.path: (item.content.encode(), 0o755 if item.mode & 0o111 else 0o644)
        for item in ordinary_old}
    old_report = {"build_id": str(uuid.uuid4()), "guide_version": 1,
        "blueprint_hash": fingerprint(spec), "brief_hash": "b" * 64,
        "verification": {"digest": "c" * 64}, "unresolved": [],
        "framework": {"kind": "host-framework-provenance", "version": 1,
            "blueprint_digest": fingerprint(spec), "baseline_digest": fingerprint(baseline_old),
            "current_compiler_baseline_digest": fingerprint(baseline_old), "basis": "current-compiler",
            "retained_template_paths": []}}
    previous["implementation-report.json"] = json.dumps(old_report).encode(), 0o644
    previous["application/__init__.py"] = b"raise RuntimeError('Synthetic application must not execute')\n", 0o644
    previous["requirements.application.txt"] = b"# Inert fixture has no application dependencies.\n", 0o644
    source = [retained.generator.PackageFile(name, content.decode(), 0o600 if name == ".env.example" else mode)
        for name, (content, mode) in previous.items() if name != "SHA256SUMS"]
    generated = retained.compiler.generate_runtime_refresh(spec, source, runtime_target=target)
    ordinary = retained.compiler.generate_package(spec, runtime_target=target)
    baseline = {item.path: {"sha256": updates.digest(item.content.encode()), "mode": item.mode}
        for item in generated}
    current = {item.path: {"sha256": updates.digest(item.content.encode()), "mode": item.mode}
        for item in ordinary}
    binding = {"build_id": old_report["build_id"], "source_digest": old_report["verification"]["digest"]}
    framework = {"kind": "host-framework-provenance", "version": 1,
        "blueprint_digest": fingerprint(spec), "basis": "runtime-refresh", "reason": "explicit-runtime-refresh",
        "previous": binding, "origin": binding, "baseline_digest": fingerprint(baseline),
        "current_compiler_baseline_digest": fingerprint(current),
        "retained_template_paths": sorted(name for name in baseline.keys() | current.keys()
            if name != "SHA256SUMS" and baseline.get(name) != current.get(name)),
        "runtime_candidate": retained.selection.candidate_selection(target)}
    candidate = {item.path: (item.content.encode(), 0o755 if item.mode & 0o111 else 0o644)
        for item in generated}
    for name, value in previous.items():
        if name.startswith("application/") or name == "requirements.application.txt":
            candidate[name] = value
    report = {**old_report, "build_id": str(uuid.uuid4()), "verification": {"digest": "d" * 64},
        "framework": framework}
    candidate["implementation-report.json"] = json.dumps(report).encode(), 0o644
    paths = retained.updates._archived_framework_paths(candidate)
    assert len(paths) == 29 and set(paths) == set(updates._archived_framework_paths(candidate))
    assert retained.updates._archived_framework_provenance(candidate, paths)
    for name, original in (("runtime_candidates.py", "launchkit/runtime_candidates.py"),
            ("install_candidate.py", "launchkit/templates/install_candidate.py"),
            ("native_definition_continuity.py", "launchkit/native_definition_continuity.py")):
        assert candidate[name][0] == retained.source_files[original]
    assert "sdk_wheel_integrity.py" not in candidate
    return (bundle, current_target, previous, candidate, old_report, report), target


@pytest.fixture
def historical_candidate_owned(tmp_path, monkeypatch, retained_candidate_v1, historical_candidate_crossing,
        trusted_supervisor_sources):
    """Admit only synthetic original-v1 source through its original owner."""
    retained = retained_candidate_v1
    crossing, original_target = historical_candidate_crossing
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    Path.home().mkdir(mode=0o700)
    directory, destination = installation(tmp_path, crossing)
    receipt = deployment.read_receipt(directory)
    manager = Manager()
    owner = deployment.DeploymentService(tmp_path / "owner", None, None, service_manager=manager)
    selected = owner.root / receipt["guide_id"] / receipt["deployment_id"]
    selected.parent.mkdir(mode=0o700, parents=True)
    directory.rename(selected)
    directory = selected
    (selected.parent / "latest.json").write_text(json.dumps({"deployment_id": receipt["deployment_id"]}))
    (selected.parent / "latest.json").chmod(0o600)
    deployment.update_receipt(directory, execution_mode="background", status="stopped",
        service_supervisor_pid=1234, service_instance_id="synthetic-cleaned-v1-instance",
        service_cleanup_confirmed=True, runtime_pid=None, process_live=False)

    def forbidden(*args, **kwargs):
        pytest.fail("No process, SDK, native binary, application or OS manager may execute.")

    monkeypatch.setattr(asyncio, "create_subprocess_exec", forbidden)
    monkeypatch.setattr(os, "system", forbidden)
    monkeypatch.setattr(retained.services, "supervisor_source_files", lambda: deepcopy(retained.source_files))
    service = retained.services.prepare_service(directory, deployment.read_receipt(directory))
    assert service["host_files"] == {name: updates.digest(content) for name, content in retained.source_files.items()}
    owned = owner, manager, directory, destination, service, retained.source_files, trusted_supervisor_sources
    _, _, previous, candidate, _, report = crossing
    staged = retained.updates.stage(directory, packed(candidate), report, runtime_target=original_target)
    update = staged["update"]
    assert update["blockers"] == [] and update["runtime_dependency_update"]["version"] == 1
    assert len(update["runtime_dependency_update"]["protected_paths"]) == 29
    retained.updates.activate(directory, update["update_id"])
    root = retained.updates.update_directory(directory, update["update_id"])
    update = json.loads((root / "update.json").read_text())
    sdk, _ = inert_installed_candidate(owned, crossing, monkeypatch)
    _, native = candidate_proof(update["runtime_dependency_update"], destination)
    native["sdk"] = deepcopy(sdk)
    for name in native["binaries"]:
        digest = updates.digest((destination / ".bin" / (name + ".receipt.json")).read_bytes())
        native["binaries"][name]["receipt_sha256"] = digest
        update["runtime_dependency_update"]["runtime_candidate_artifacts"]["native"][name]["receipt_sha256"] = digest
    # Closed fixture journal records manually populated inert bytes, never startup.
    update = retained.updates.checkpoint(directory, root, update, status="completed", blockers=[],
        rollback_available=False, runtime_admission_possible=True,
        runtime_files_retained={"format": "mobkit-launch-runtime-retention-v1", "retained": True,
            "inventory_sha256": "e" * 64, "destination": str(destination),
            "backup": str(root / "runtime-before"), "previous_native": native_fixture_proof("0.8.47")},
        runtime_artifacts={"sdk": sdk, "native": native})
    deployment.update_receipt(directory, startup_candidate_runtime_proof=native)
    receipt = deployment.read_receipt(directory)
    assert updates._completed_runtime_dependency_transition(previous, candidate, receipt, update, root) == update["runtime_dependency_update"]
    monkeypatch.setattr(services, "supervisor_source_files", lambda: deepcopy(trusted_supervisor_sources))
    return owned, crossing


def test_original_candidate_v1_closed_history_keeps_exact_reader_and_inventory(historical_candidate_owned):
    owned, crossing = historical_candidate_owned
    directory, destination = owned[2:4]
    receipt = deployment.read_receipt(directory)
    update = receipt["update"]
    root = updates.update_directory(directory, update["update_id"])
    _, _, previous, candidate, _, _ = crossing
    descriptor = update["runtime_dependency_update"]
    assert descriptor["version"] == 1 and len(descriptor["protected_paths"]) == 29
    assert "required_supervisor_sources" not in descriptor
    assert updates.digest(candidate["runtime_candidates.py"][0]) == "78320dbc3ad7f5de4d292c12b117ce0f38f43237ebe8eac25f675635a2b7e889"
    assert "sdk_wheel_integrity.py" not in candidate
    assert updates._completed_runtime_dependency_transition(previous, candidate, receipt, update, root) == descriptor
    changed = deepcopy(candidate)
    changed["runtime_candidates.py"] = changed["runtime_candidates.py"][0] + b"# Changed retained reader\n", 0o644
    before = snapshot(directory), snapshot(destination)
    assert updates._completed_runtime_dependency_transition(previous, changed, receipt, update, root) is None
    assert (snapshot(directory), snapshot(destination)) == before


@pytest.fixture
def maintained_owned(historical_candidate_owned, monkeypatch):
    owned, crossing = historical_candidate_owned
    result, anchor = current_prepared(owned)
    asyncio.run(refresh.apply(owned[2], result["revision"], owned[1],
        expected_digest=result["descriptor_sha256"]))
    maintained_fixture_update(owned, crossing)
    next_supervisor(owned, monkeypatch)
    owned[1].calls.clear()
    return owned, anchor


async def test_retained_history_maintained_crossing_owner_prepare_apply_preserves_current_runtime(maintained_owned):
    owned, anchor = maintained_owned
    owner, manager, directory, destination, _, _, _ = owned
    receipt = deployment.read_receipt(directory)
    before, unit = snapshot(destination), Path(receipt["service"]["unit_path"]).read_bytes()
    admitted = json.loads((anchor / "before/receipt.json").read_text())["update"]
    assert admitted["runtime_dependency_update"] is not None and receipt["update"].get("runtime_artifacts") is None
    result = await owner.prepare_supervisor_refresh(receipt["guide_id"], receipt["deployment_id"],
        expected_service_digest=refresh.service_digest(receipt["service"]))
    assert refresh.inspect(directory, result["revision"])["phase"] == "prepared"
    finished = await owner.apply_supervisor_refresh(receipt["guide_id"], receipt["deployment_id"], result["revision"],
        expected_digest=result["descriptor_sha256"])
    assert finished["phase"] == "completed" and len(manager.calls) == 3
    assert snapshot(destination) == before and Path(receipt["service"]["unit_path"]).read_bytes() == unit
    actual = deployment.read_receipt(directory)
    assert actual["update"] == receipt["update"] and actual["runtime_candidate"] == receipt["runtime_candidate"]
    services.verify_service(directory, actual)


async def test_retained_history_repeated_normal_custody_reaches_original_admission(maintained_owned, monkeypatch):
    owned, anchor = maintained_owned
    before = snapshot(anchor)
    for _ in range(3):
        result, _ = current_prepared(owned)
        await refresh.apply(owned[2], result["revision"], owned[1], expected_digest=result["descriptor_sha256"])
        next_supervisor(owned, monkeypatch)
    refresh._ordinary_source(owned[2], deployment.read_receipt(owned[2]))
    assert snapshot(anchor) == before


def rebound_custody_before(directory, root, prior):
    """Rebind only tmp custody to test semantic guards beyond a byte mismatch."""
    rewrite(root / "before/receipt.json", prior)
    document = json.loads((root / "descriptor.json").read_text())
    document.update(before_receipt_sha256=updates.digest((root / "before/receipt.json").read_bytes()),
        source_fence=refresh._fence(prior))
    rewrite(root / "descriptor.json", document)
    selected = {"revision": document["revision"], "descriptor_sha256": updates.digest((root / "descriptor.json").read_bytes())}
    journal = json.loads((root / "journal.json").read_text()); journal.update(selected)
    rewrite(root / "journal.json", journal)
    deployment.update_receipt(directory, supervisor_refresh=selected)


@pytest.mark.parametrize("mutation", ["cycle", "depth", "foreign-owner", "foreign-selection", "forged-maintained-only"])
async def test_retained_history_authenticated_anchor_guards(maintained_owned, monkeypatch, mutation):
    owned, anchor = maintained_owned
    directory = owned[2]
    if mutation in {"cycle", "depth"}:
        result, anchor = current_prepared(owned)
        await refresh.apply(directory, result["revision"], owned[1], expected_digest=result["descriptor_sha256"])
        next_supervisor(owned, monkeypatch)
    prior = json.loads((anchor / "before/receipt.json").read_text())
    if mutation == "cycle": prior["supervisor_refresh"] = deepcopy(deployment.read_receipt(directory)["supervisor_refresh"])
    elif mutation == "depth": monkeypatch.setattr(refresh, "MAX_ADMISSION_CUSTODY", 1)
    elif mutation == "foreign-owner": prior["destination"] += "-foreign"
    elif mutation == "foreign-selection": prior["runtime_candidate"]["accepted"]["native_run_attempt"] = True
    else:
        # An authenticated retained receipt with a source-only descriptor still
        # cannot establish runtime admission, even at the original source.
        prior["update"]["maintained_host_update"] = {"kind": "maintained-console-host-update",
            "version": 1, "runtime_candidate": deepcopy(prior["runtime_candidate"])}
        prior["update"]["runtime_dependency_update"] = None
        rewrite(updates.update_directory(directory, prior["update"]["update_id"]) / "update.json", prior["update"])
    if mutation != "depth": rebound_custody_before(directory, anchor, prior)
    owned[1].calls.clear()
    before = snapshot(owned[3]), snapshot(directory)
    receipt = deployment.read_receipt(directory)
    with pytest.raises(DraftConflict):
        refresh.prepare(directory, refresh.service_digest(receipt["service"]))
    assert (snapshot(owned[3]), snapshot(directory)) == before and not owned[1].calls


def test_retained_history_second_maintained_crossing_keeps_closed_inventory_refusal(maintained_owned):
    owned, _ = maintained_owned
    from launchkit import compiler
    directory = owned[2]
    receipt = deployment.read_receipt(directory)
    _, files = updates.checked_package(directory, "package.zip", receipt["package_sha256"])
    names = updates._archived_framework_paths(files)
    assert set(compiler.MAINTAINED_HOST_PATHS) - set(names) == {
        "console_connect.py", "console_presentation.py", "console_shell.py", "console_shell.mjs", "console_shell.css"}
    assert not updates._archived_framework_provenance(files, names)
    candidate = deepcopy(files)
    candidate["app.py"] = files["app.py"][0] + b"\n# Another maintained host revision\n", files["app.py"][1]
    before = snapshot(directory), snapshot(owned[3])
    assert updates.maintained_host_transition(files, candidate, receipt, archived=receipt["update"]["maintained_host_update"]) is None
    assert (snapshot(directory), snapshot(owned[3])) == before


@pytest.mark.parametrize("phase", ["retaining", "retained", "installed", "publishing"])
async def test_retained_history_pending_custody_recovers_exact_transition(maintained_owned, monkeypatch, phase):
    owned, _ = maintained_owned
    directory, destination = owned[2:4]
    result, root = current_prepared(owned)
    before = snapshot(destination)
    advance = refresh._phase
    def interrupted(root_arg, journal, value):
        changed = advance(root_arg, journal, value)
        if value == phase:
            raise Crash("Inert interrupted custody publication.")
        return changed
    monkeypatch.setattr(refresh, "_phase", interrupted)
    with pytest.raises(Crash):
        await refresh.apply(directory, result["revision"], owned[1], expected_digest=result["descriptor_sha256"])
    monkeypatch.setattr(refresh, "_phase", advance)
    assert json.loads((root / "journal.json").read_text())["phase"] == phase
    assert (await refresh.apply(directory, result["revision"], owned[1],
        expected_digest=result["descriptor_sha256"]))["phase"] == "completed"
    assert snapshot(destination) == before


@pytest.mark.parametrize("mutation", ["missing-anchor", "foreign-anchor", "historical-pending", "journal",
    "package", "original-selection", "original-type", "native-proof", "sdk-proof", "destination", "unit",
    "maintained-predecessor", "startup-proof"])
def test_retained_history_refuses_broken_original_provenance_without_writes(maintained_owned, mutation):
    owned, anchor = maintained_owned
    _, manager, directory, destination, _, _, _ = owned
    receipt = deployment.read_receipt(directory)
    original = json.loads((anchor / "before/receipt.json").read_text())
    original_root = updates.update_directory(directory, original["update"]["update_id"])
    if mutation == "missing-anchor": receipt.pop("supervisor_refresh")
    elif mutation == "foreign-anchor": receipt["supervisor_refresh"]["descriptor_sha256"] = "f" * 64
    elif mutation == "historical-pending":
        journal = json.loads((anchor / "journal.json").read_text()); journal["phase"] = "prepared"
        rewrite(anchor / "journal.json", journal)
    elif mutation == "journal":
        update = deepcopy(original["update"]); update["source_digest"] = "f" * 64
        rewrite(original_root / "update.json", update)
    elif mutation == "package": rewrite(original_root / "candidate.zip", b"Not the accepted archive")
    elif mutation in {"original-selection", "original-type", "native-proof", "sdk-proof", "destination", "unit"}:
        if mutation == "original-selection": original["runtime_candidate"]["accepted"]["tuple_sha256"] = "f" * 64
        elif mutation == "original-type": original["runtime_candidate"]["accepted"]["native_run_attempt"] = True
        elif mutation == "native-proof": original["update"]["runtime_artifacts"]["native"]["binaries"]["rpc_gateway"]["binary_sha256"] = "f" * 64
        elif mutation == "sdk-proof": original["update"]["runtime_artifacts"]["sdk"]["published"] = True
        elif mutation == "destination": original["destination"] += "-foreign"
        else: original["service"]["unit_sha256"] = "f" * 64
        rewrite(anchor / "before/receipt.json", original)
    elif mutation == "maintained-predecessor":
        receipt["update"]["previous_package_sha256"] = "f" * 64
        rewrite(updates.update_directory(directory, receipt["update"]["update_id"]) / "update.json", receipt["update"])
    else: receipt["startup_candidate_runtime_proof"]["sdk"]["published"] = True
    rewrite(directory / "receipt.json", receipt)
    before = snapshot(destination), snapshot(directory)
    unit = Path(receipt["service"]["unit_path"]).read_bytes()
    with pytest.raises(DraftConflict):
        refresh.prepare(directory, refresh.service_digest(receipt["service"]))
    assert (snapshot(destination), snapshot(directory)) == before
    assert Path(receipt["service"]["unit_path"]).read_bytes() == unit and not manager.calls


@pytest.mark.parametrize("mutation", ["sdk-bytes", "native-bytes", "native-metadata"])
def test_retained_history_current_installed_runtime_drift_is_refused(maintained_owned, mutation):
    owned, _ = maintained_owned
    _, manager, directory, destination, _, _, _ = owned
    if mutation == "sdk-bytes":
        (destination / ".venv/lib/python3.14/site-packages/meerkat_mobkit/__init__.py").write_bytes(b"Changed SDK source\n")
    elif mutation == "native-bytes": (destination / ".bin/rpc_gateway").write_bytes(b"Changed native bytes\n")
    else:
        path = destination / ".bin/rpc_gateway.receipt.json"
        record = json.loads(path.read_text()); record["native_run_attempt"] = True; rewrite(path, record)
    before = snapshot(destination), snapshot(directory)
    receipt = deployment.read_receipt(directory)
    with pytest.raises(DraftConflict, match="completed installed runtime admission"):
        refresh.prepare(directory, refresh.service_digest(receipt["service"]))
    assert (snapshot(destination), snapshot(directory)) == before and not manager.calls


def test_retained_history_late_current_owner_drift_prevents_custody_writes(maintained_owned, monkeypatch):
    owned, _ = maintained_owned
    directory = owned[2]
    verify = refresh.verify_runtime
    def changed(*args, **kwargs):
        proof = verify(*args, **kwargs)
        deployment.update_receipt(directory, service_instance_id="Changed during physical verification")
        return proof
    monkeypatch.setattr(refresh, "verify_runtime", changed)
    receipt = deployment.read_receipt(directory)
    before = snapshot(directory / "supervisor-refreshes")
    with pytest.raises(DraftConflict, match="completed installed runtime admission"):
        refresh.prepare(directory, refresh.service_digest(receipt["service"]))
    assert snapshot(directory / "supervisor-refreshes") == before and not owned[1].calls


@pytest.mark.parametrize("mutation", ["owner-at-inspection", "startup-during-verify"])
def test_retained_history_current_inspection_and_retained_carrier_cannot_drift(maintained_owned, monkeypatch, mutation):
    owned, _ = maintained_owned
    directory = owned[2]
    if mutation == "owner-at-inspection":
        inspect = refresh.inspect_transaction
        def changed(*args, **kwargs):
            deployment.update_receipt(directory, service_instance_id="Changed before current transaction inspection")
            return inspect(*args, **kwargs)
        monkeypatch.setattr(refresh, "inspect_transaction", changed)
    else:
        verify = refresh.verify_runtime
        def changed(*args, **kwargs):
            proof = verify(*args, **kwargs)
            receipt = deployment.read_receipt(directory)
            receipt["startup_candidate_runtime_proof"]["sdk"]["published"] = True
            rewrite(directory / "receipt.json", receipt)
            return proof
        monkeypatch.setattr(refresh, "verify_runtime", changed)
    receipt = deployment.read_receipt(directory)
    before = snapshot(directory / "supervisor-refreshes")
    with pytest.raises(DraftConflict, match="completed installed runtime admission"):
        refresh.prepare(directory, refresh.service_digest(receipt["service"]))
    assert snapshot(directory / "supervisor-refreshes") == before and not owned[1].calls


def test_retained_history_unreferenced_admission_cannot_replace_the_named_anchor(maintained_owned, monkeypatch):
    owned, anchor = maintained_owned
    directory = owned[2]
    prior = json.loads((anchor / "before/receipt.json").read_text())
    original = updates.update_directory(directory, prior["update"]["update_id"])
    foreign_id = str(uuid.uuid4())
    foreign = updates.update_directory(directory, foreign_id)
    foreign.mkdir(mode=0o700)
    for name in ("candidate.zip", "previous.zip", "previous-receipt.json"):
        (foreign / name).write_bytes((original / name).read_bytes()); (foreign / name).chmod(0o600)
    forged = deepcopy(prior["update"])
    forged["update_id"] = foreign_id
    forged["runtime_artifacts"]["sdk"]["published"] = True
    (foreign / "update.json").write_text(json.dumps(forged)); (foreign / "update.json").chmod(0o600)
    calls = []
    select = updates.update_directory
    def named(directory_arg, identity):
        calls.append(identity)
        assert identity != foreign_id
        return select(directory_arg, identity)
    monkeypatch.setattr(updates, "update_directory", named)
    before = snapshot(directory), snapshot(owned[3])
    refresh._ordinary_source(directory, deployment.read_receipt(directory))
    assert calls and (snapshot(directory), snapshot(owned[3])) == before
    deployment.update_receipt(directory, supervisor_refresh=None)
    receipt = deployment.read_receipt(directory)
    before = snapshot(directory), snapshot(owned[3])
    with pytest.raises(DraftConflict): refresh.prepare(directory, refresh.service_digest(receipt["service"]))
    assert (snapshot(directory), snapshot(owned[3])) == before and not owned[1].calls


@pytest.mark.parametrize("mutation", ["previous-receipt-mode", "source-during-verify"])
def test_retained_history_late_source_and_original_receipt_drift_are_refused(maintained_owned, monkeypatch, mutation):
    owned, anchor = maintained_owned
    directory = owned[2]
    if mutation == "previous-receipt-mode":
        prior = json.loads((anchor / "before/receipt.json").read_text())
        (updates.update_directory(directory, prior["update"]["update_id"]) / "previous-receipt.json").chmod(0o644)
    else:
        verify = refresh.verify_runtime
        def changed(*args, **kwargs):
            proof = verify(*args, **kwargs)
            (owned[3] / "app.py").write_bytes(b"Changed installed source during physical verification\n")
            return proof
        monkeypatch.setattr(refresh, "verify_runtime", changed)
    receipt = deployment.read_receipt(directory)
    before = snapshot(directory / "supervisor-refreshes")
    with pytest.raises(DraftConflict): refresh.prepare(directory, refresh.service_digest(receipt["service"]))
    assert snapshot(directory / "supervisor-refreshes") == before and not owned[1].calls


async def test_retained_candidate_owner_refresh_preserves_admission_private_source_and_unit(candidate_owned):
    owner, manager, directory, destination, service, old, current = candidate_owned
    receipt = deployment.read_receipt(directory)
    source, unit = snapshot(destination), Path(service["unit_path"]).read_bytes()
    result = await owner.prepare_supervisor_refresh(receipt["guide_id"], receipt["deployment_id"],
        expected_service_digest=refresh.service_digest(service))
    saved = deployment.read_receipt(directory)
    root = directory / "supervisor-refreshes" / result["revision"]
    descriptor = json.loads((root / "descriptor.json").read_text())
    assert descriptor["source_fence"]["runtime_candidate"] == receipt["runtime_candidate"]
    finished = await owner.apply_supervisor_refresh(receipt["guide_id"], receipt["deployment_id"], result["revision"],
        expected_digest=result["descriptor_sha256"])
    actual = deployment.read_receipt(directory)
    assert finished["phase"] == "completed" and len(manager.calls) == 3
    assert snapshot(destination) == source and Path(service["unit_path"]).read_bytes() == unit
    assert {k: v for k, v in actual.items() if k != "service"} == {k: v for k, v in saved.items() if k != "service"}
    assert {k: v for k, v in actual["service"].items() if k not in refresh.PROTOCOL_KEYS} == {
        k: v for k, v in service.items() if k not in refresh.PROTOCOL_KEYS}
    assert {name: content for name, (content, _) in snapshot(root / "before/host").items()} == old
    assert {name: content for name, (content, _) in snapshot(directory / "service-host").items()} == current
    services.verify_service(directory, actual)


async def test_retained_candidate_does_not_invalidate_completed_ordinary_supervisor_history(current_admission_owned, crossing, monkeypatch):
    owned = current_admission_owned
    _, _, directory, _, _, _, _ = owned
    result = deployment.read_receipt(directory)["supervisor_refresh"]
    root = directory / "supervisor-refreshes" / result["revision"]
    archived = snapshot(root)
    admit_candidate(owned, crossing, monkeypatch)
    receipt = deployment.read_receipt(directory)
    assert "runtime_candidate" not in json.loads((root / "descriptor.json").read_text())["source_fence"]
    refresh._ordinary_source(directory, receipt)
    services.verify_service(directory, receipt)
    assert refresh.inspect(directory, result["revision"], expected_digest=result["descriptor_sha256"])["phase"] == "completed"
    assert snapshot(root) == archived


@pytest.mark.parametrize("mutation", ["selection", "missing-selection", "same-version-tuple", "typed-attempt",
    "framework-selection", "package-selection", "unadmitted", "pending", "native-proof", "sdk-proof", "native-team"])
def test_retained_candidate_prepare_refuses_changed_or_unadmitted_source_without_writes(candidate_owned, mutation):
    _, manager, directory, destination, service, _, _ = candidate_owned
    receipt = deployment.read_receipt(directory)
    if mutation == "selection": receipt["runtime_candidate"]["artifact_root"] += "-foreign"
    elif mutation == "missing-selection": receipt.pop("runtime_candidate")
    elif mutation == "same-version-tuple": receipt["runtime_candidate"]["accepted"]["tuple_sha256"] = "f" * 64
    elif mutation == "typed-attempt": receipt["runtime_candidate"]["accepted"]["native_run_attempt"] = True
    elif mutation in {"framework-selection", "package-selection"}:
        _, files = updates.checked_package(directory, "package.zip", receipt["package_sha256"])
        files = deepcopy(files)
        name = "implementation-report.json" if mutation == "framework-selection" else "deployment.json"
        value = json.loads(files[name][0])
        if mutation == "framework-selection": value["framework"]["runtime_candidate"]["tuple_path"] += ".foreign"
        else: value["runtimeCandidate"]["tuple_sha256"] = "f" * 64
        content = json.dumps(value).encode()
        files[name] = content, files[name][1]
        (destination / name).write_bytes(content)
        payload = packed(files)
        (directory / "package.zip").write_bytes(payload)
        receipt["package_sha256"] = updates.digest(payload)
        rewrite(destination / deployment.OWNER_FILE, deployment.ownership(receipt))
    elif mutation == "unadmitted": receipt.pop("update")
    elif mutation == "pending": receipt["update"]["status"] = "staged"
    elif mutation == "native-proof": receipt["update"]["runtime_artifacts"]["native"]["candidate_binding"]["source_sha"] = "f" * 40
    elif mutation == "sdk-proof": receipt["update"]["runtime_artifacts"]["sdk"]["published"] = True
    elif mutation == "native-team":
        receipt.update(source_kind="native-team", source_id="synthetic-team", source_digest="f" * 64)
        rewrite(destination / deployment.OWNER_FILE, deployment.ownership(receipt))
    rewrite(directory / "receipt.json", receipt)
    if mutation in {"native-proof", "sdk-proof"}:
        rewrite(directory / "updates" / receipt["update"]["update_id"] / "update.json", receipt["update"])
    before = snapshot(destination), snapshot(directory)
    unit = Path(service["unit_path"]).read_bytes()
    with pytest.raises(DraftConflict):
        refresh.prepare(directory, refresh.service_digest(service))
    assert (snapshot(destination), snapshot(directory)) == before
    assert Path(service["unit_path"]).read_bytes() == unit and not manager.calls


async def test_retained_candidate_apply_refuses_admission_drift_before_manager_or_host_change(candidate_owned):
    _, manager, directory, destination, service, _, _ = candidate_owned
    result, _ = prepared(candidate_owned)
    receipt = deployment.read_receipt(directory)
    receipt["runtime_candidate"]["tuple_path"] += ".foreign"
    rewrite(directory / "receipt.json", receipt)
    before = snapshot(destination), snapshot(directory)
    unit = Path(service["unit_path"]).read_bytes()
    with pytest.raises(DraftConflict, match="exact retained candidate"):
        await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"])
    assert (snapshot(destination), snapshot(directory)) == before
    assert Path(service["unit_path"]).read_bytes() == unit and not manager.calls


def test_completed_maintained_source_history_does_not_establish_runtime_admission(candidate_owned, monkeypatch):
    _, manager, directory, destination, service, _, _ = candidate_owned
    receipt = deployment.read_receipt(directory)
    update = {"update_id": str(uuid.uuid4()), "status": "completed",
        "package_sha256": receipt["package_sha256"], "runtime_dependency_update": None,
        "maintained_host_update": {"kind": "maintained-console-host-update", "version": 1,
            "runtime_candidate": deepcopy(receipt["runtime_candidate"])}}
    receipt["update"] = update
    receipt.pop("startup_candidate_runtime_proof", None)
    rewrite(directory / "receipt.json", receipt)
    calls = []

    def source_history(directory_arg, identity):
        # The detached inspector projection establishes completed source
        # continuity only. It contains no installed SDK/native admission proof.
        assert directory_arg == directory and identity == update["update_id"]
        calls.append(identity)
        return deepcopy(receipt), directory / "updates" / identity, deepcopy(update)

    monkeypatch.setattr(refresh, "inspect_transaction", source_history)
    before = snapshot(destination), snapshot(directory)
    unit = Path(service["unit_path"]).read_bytes()
    with pytest.raises(DraftConflict, match="completed installed runtime admission"):
        refresh.prepare(directory, refresh.service_digest(service))
    assert calls == [update["update_id"]]
    assert (snapshot(destination), snapshot(directory)) == before
    assert Path(service["unit_path"]).read_bytes() == unit and not manager.calls


async def test_retained_candidate_recovers_after_new_service_receipt_without_readmitting_runtime(candidate_owned, monkeypatch):
    _, manager, directory, destination, service, _, current = candidate_owned
    result, root = prepared(candidate_owned)
    source, unit = snapshot(destination), Path(service["unit_path"]).read_bytes()
    admission = deployment.read_receipt(directory)["runtime_candidate"]
    retained = snapshot(root / "before")
    phase = refresh._phase

    def interrupted(root, journal, value):
        if value == "completed":
            raise Crash("Fixture crash after retained candidate supervisor receipt publication.")
        return phase(root, journal, value)

    monkeypatch.setattr(refresh, "_phase", interrupted)
    with pytest.raises(Crash):
        await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"])
    monkeypatch.setattr(refresh, "_phase", phase)
    finished = await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"])
    assert finished["phase"] == "completed" and deployment.read_receipt(directory)["runtime_candidate"] == admission
    assert snapshot(destination) == source and Path(service["unit_path"]).read_bytes() == unit
    assert snapshot(root / "before") == retained
    assert {name: content for name, (content, _) in snapshot(directory / "service-host").items()} == current


@pytest.fixture
async def completed_historical_supervisor(owned, monkeypatch):
    _, manager, directory, destination, service, _, current = owned
    historical = {name: value for name, value in current.items()
                  if name != "launchkit/sdk_wheel_integrity.py"}
    current_reader = Path(services.__file__).with_name("runtime_candidates.py").read_bytes()
    assert services.digest(historical["launchkit/runtime_candidates.py"]) != services.digest(current_reader)
    monkeypatch.setattr(services, "supervisor_source_files", lambda: deepcopy(historical))
    before_source, unit = snapshot(destination), Path(service["unit_path"]).read_bytes()
    result = refresh.prepare(directory, refresh.service_digest(service))
    finished = await refresh.apply(directory, result["revision"], manager,
        expected_digest=result["descriptor_sha256"])
    assert finished["phase"] == "completed"
    root = directory / "supervisor-refreshes" / result["revision"]
    assert snapshot(destination) == before_source and Path(service["unit_path"]).read_bytes() == unit
    assert "launchkit/sdk_wheel_integrity.py" not in json.loads(
        (root / "descriptor.json").read_text())["after_manifest"]
    return owned, finished, root, historical


async def test_completed_historical_supervisor_without_later_helper_remains_readable(completed_historical_supervisor):
    owned, finished, root, historical = completed_historical_supervisor
    _, manager, directory, destination, _, _, _ = owned
    receipt = deployment.read_receipt(directory)
    before, source = snapshot(directory), snapshot(destination)
    manager_calls = list(manager.calls)
    assert refresh.inspect(directory, finished["revision"],
        expected_digest=finished["descriptor_sha256"]) == finished
    refresh.require_complete(directory, receipt)
    assert not refresh.pending_custody(directory, receipt)
    installed = services.verify_service(directory, receipt)
    assert installed["host_files"] == {name: services.digest(content) for name, content in historical.items()}
    assert snapshot(directory) == before and snapshot(destination) == source
    assert manager.calls == manager_calls


@pytest.mark.parametrize("mutation", ["bytes", "mode", "link"])
async def test_completed_historical_supervisor_archive_corruption_still_refuses(
        completed_historical_supervisor, mutation):
    owned, finished, root, _ = completed_historical_supervisor
    _, manager, directory, destination, _, _, _ = owned
    path = root / "after/host/launchkit/runtime_candidates.py"
    if mutation == "bytes":
        rewrite(path, path.read_bytes() + b"# Changed retained historical bytes\n")
    elif mutation == "mode":
        path.chmod(0o600)
    else:
        path.unlink()
        path.symlink_to(root / "after/host/launchkit/runtime_dependencies.py")
    before, source, calls = snapshot(directory), snapshot(destination), list(manager.calls)
    with pytest.raises(DraftConflict):
        refresh.inspect(directory, finished["revision"], expected_digest=finished["descriptor_sha256"])
    assert refresh.pending_custody(directory, deployment.read_receipt(directory))
    assert snapshot(directory) == before and snapshot(destination) == source and manager.calls == calls


def test_current_supervisor_reader_without_required_helper_refuses_before_publication(owned, monkeypatch):
    _, manager, directory, destination, service, _, current = owned
    missing = {name: value for name, value in current.items() if name != "launchkit/sdk_wheel_integrity.py"}
    missing["launchkit/runtime_candidates.py"] = Path(services.__file__).with_name("runtime_candidates.py").read_bytes()
    monkeypatch.setattr(services, "supervisor_source_files", lambda: deepcopy(missing))
    before = snapshot(destination), snapshot(directory / "service-host"), (directory / "receipt.json").read_bytes()
    with pytest.raises(DraftConflict, match="exact candidate startup contract"):
        refresh.prepare(directory, refresh.service_digest(service))
    assert (snapshot(destination), snapshot(directory / "service-host"), (directory / "receipt.json").read_bytes()) == before
    assert not manager.calls
    selected = services.supervisor_metadata(service, missing)
    with pytest.raises(DraftConflict, match="selected candidate runtime on startup and restart"):
        services.require_candidate_update_protocol(selected, {"maintained_host_update": {"runtime_candidate": {}}})


def test_missing_custody_verifier_refuses_before_selector_or_live_publication(owned, monkeypatch):
    _, manager, directory, destination, service, _, current = owned
    missing = {name: value for name, value in current.items() if name != "launchkit/deployment_supervisor.py"}
    monkeypatch.setattr(services, "supervisor_source_files", lambda: deepcopy(missing))
    before = snapshot(destination), snapshot(directory / "service-host"), (directory / "receipt.json").read_bytes()
    unit = Path(service["unit_path"]).read_bytes()
    with pytest.raises(DraftConflict, match="exact candidate startup contract"):
        refresh.prepare(directory, refresh.service_digest(service))
    assert (snapshot(destination), snapshot(directory / "service-host"), (directory / "receipt.json").read_bytes()) == before
    assert Path(service["unit_path"]).read_bytes() == unit and not manager.calls


def test_all_custody_directory_entries_are_synced_before_selector_publication(owned, monkeypatch):
    _, _, directory, _, _, _, _ = owned
    synced = []
    sync, write = refresh._sync, refresh.write_private

    def observed_sync(path):
        sync(path)
        synced.append(Path(path))

    def observed_write(path, value):
        if Path(path) == directory / "receipt.json" and "supervisor_refresh" in value:
            store = directory / "supervisor-refreshes"
            paths = {store, directory, *[p for p in store.rglob("*") if p.is_dir()]}
            assert paths <= set(synced), "Every frozen/staged directory entry must be durable before its selector."
        write(path, value)

    monkeypatch.setattr(refresh, "_sync", observed_sync)
    monkeypatch.setattr(refresh, "write_private", observed_write)
    result, _ = prepared(owned)
    assert result["phase"] == "prepared"


@pytest.mark.parametrize("complete", [False, True], ids=["incomplete", "completed"])
async def test_ordinary_application_stage_obeys_supervisor_custody_before_publication(owned, crossing, complete):
    _, manager, directory, destination, service, _, _ = owned
    result, _ = prepared(owned)
    if complete:
        await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"])
    _, _, previous, _, old_report, _ = crossing
    candidate = deepcopy(previous)
    report = {**old_report, "build_id": str(uuid.uuid4()), "verification": {"digest": "a" * 64}}
    candidate["application/__init__.py"] = (b"raise RuntimeError('Updated application execution forbidden')\n", 0o644)
    candidate["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    source, host, journal = snapshot(destination), snapshot(directory / "service-host"), snapshot(directory)
    unit = Path(service["unit_path"]).read_bytes()
    from launchkit.deployment_updates import stage
    if not complete:
        with pytest.raises(DraftConflict, match="Complete the exact supervisor custody"):
            stage(directory, packed(candidate), report)
        assert snapshot(directory) == journal and not (directory / "updates").exists()
    else:
        staged = stage(directory, packed(candidate), report)
        assert staged["update"]["status"] == "staged" and not staged["update"]["runtime_dependency_update"]
        assert staged["supervisor_refresh"] == deployment.read_receipt(directory)["supervisor_refresh"]
        services.verify_service(directory, deployment.read_receipt(directory))
    assert snapshot(destination) == source and snapshot(directory / "service-host") == host
    assert Path(service["unit_path"]).read_bytes() == unit


@pytest.mark.parametrize("publication", ["receipt", "journal"])
async def test_preexisting_pending_links_are_never_followed_or_replaced(owned, tmp_path, publication):
    _, manager, directory, _, service, _, _ = owned
    protected = tmp_path / "protected-private-file"
    protected.write_bytes(b"Private bytes outside supervisor custody must remain unchanged.\n")
    protected.chmod(0o600)
    original = protected.read_bytes()
    if publication == "receipt":
        pending = directory / "receipt.pending"
        pending.symlink_to(protected)
        refresh.prepare(directory, refresh.service_digest(service))
    else:
        result, root = prepared(owned)
        pending = root / "journal.pending"
        pending.symlink_to(protected)
        await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"])
    assert protected.read_bytes() == original
    assert pending.is_symlink() and pending.readlink() == protected


def test_prepare_retains_exact_64_file_history_and_unit_without_manager_or_private_changes(owned):
    _, manager, directory, destination, service, old, _ = owned
    before_source, before_host = snapshot(destination), snapshot(directory / "service-host")
    before_receipt = deployment.read_receipt(directory)
    unit = Path(service["unit_path"]).read_bytes()
    result, root = prepared(owned)
    assert result["phase"] == "prepared" and result["retained_files"] == 64
    assert snapshot(destination) == before_source and snapshot(directory / "service-host") == before_host
    assert Path(service["unit_path"]).read_bytes() == unit
    assert not manager.calls
    after = deployment.read_receipt(directory)
    after.pop("supervisor_refresh")
    assert after == before_receipt
    assert {name: content for name, (content, mode) in snapshot(root / "before/host").items()} == old
    assert all(mode == 0o400 for _, mode in snapshot(root / "before/host").values())
    with pytest.raises(DraftConflict, match="Complete the exact supervisor custody"):
        services.verify_service(directory, deployment.read_receipt(directory))


async def test_normal_owner_prepare_and_apply_preserve_identity_private_source_and_history(owned):
    owner, manager, directory, destination, service, old, current = owned
    receipt = deployment.read_receipt(directory)
    before_source, unit = snapshot(destination), Path(service["unit_path"]).read_bytes()
    result = await owner.prepare_supervisor_refresh(receipt["guide_id"], receipt["deployment_id"],
        expected_service_digest=refresh.service_digest(service))
    saved = deployment.read_receipt(directory)
    finished = await owner.apply_supervisor_refresh(receipt["guide_id"], receipt["deployment_id"], result["revision"],
        expected_digest=result["descriptor_sha256"])
    assert finished["phase"] == "completed" and len(manager.calls) == 3
    actual = deployment.read_receipt(directory)
    installed = services.verify_service(directory, actual)
    assert {k: v for k, v in installed.items() if k not in refresh.PROTOCOL_KEYS} == {
        k: v for k, v in service.items() if k not in refresh.PROTOCOL_KEYS}
    assert Path(service["unit_path"]).read_bytes() == unit
    assert {name: content for name, (content, _) in snapshot(directory / "service-host").items()} == current
    assert snapshot(destination) == before_source
    assert {k: v for k, v in actual.items() if k != "service"} == {k: v for k, v in saved.items() if k != "service"}
    root = directory / "supervisor-refreshes" / result["revision"]
    assert {name: content for name, (content, _) in snapshot(root / "before/host").items()} == old
    assert refresh.inspect(directory, result["revision"], expected_digest=result["descriptor_sha256"]) == finished


@pytest.mark.parametrize("method", ["prepare", "apply"])
@pytest.mark.parametrize("blocker", ["selected", "active"])
async def test_normal_owner_refuses_foreign_selection_or_active_operation_without_reads(owned, method, blocker):
    owner, manager, directory, destination, service, _, _ = owned
    result = prepared(owned)[0] if method == "apply" else None
    receipt = deployment.read_receipt(directory)
    identity = str(uuid.uuid4()) if blocker == "selected" else receipt["deployment_id"]
    if blocker == "active":
        owner.tasks[identity] = object()
    before = snapshot(destination), snapshot(directory)
    manager.calls.clear()
    with pytest.raises(DraftConflict, match="selected installation owner|newer deployment|active operation"):
        if method == "prepare":
            await owner.prepare_supervisor_refresh(receipt["guide_id"], identity,
                expected_service_digest=refresh.service_digest(service))
        else:
            await owner.apply_supervisor_refresh(receipt["guide_id"], identity, result["revision"],
                expected_digest=result["descriptor_sha256"])
    assert not manager.calls and (snapshot(destination), snapshot(directory)) == before
    owner.tasks.clear()


@pytest.mark.parametrize("mutation", ["service", "unit", "package", "source", "instance", "cleanup", "selected",
    "descriptor", "staged-bytes", "staged-mode", "staged-link", "retained-bytes", "update", "candidate-publication"])
async def test_exact_apply_fences_refuse_drift_without_repair_or_private_changes(owned, mutation):
    _, manager, directory, destination, _, _, _ = owned
    result, root = prepared(owned)
    receipt = deployment.read_receipt(directory)
    if mutation == "service": receipt["service"]["interpreter"] = "/foreign/python"
    elif mutation == "unit": Path(receipt["service"]["unit_path"]).write_bytes(b"foreign unit")
    elif mutation == "package": receipt["package_sha256"] = "f" * 64
    elif mutation == "source": (destination / "app.py").write_bytes(b"unaccepted source")
    elif mutation == "instance": receipt["service_instance_id"] = "foreign instance"
    elif mutation == "cleanup": receipt["service_cleanup_confirmed"] = False
    elif mutation == "selected": receipt["supervisor_refresh"]["revision"] = str(uuid.uuid4())
    elif mutation == "descriptor": rewrite(root / "descriptor.json", b"{}")
    elif mutation == "staged-bytes": (root / "staged-host/launcher.py").write_bytes(b"foreign stage")
    elif mutation == "staged-mode": (root / "staged-host/launcher.py").chmod(0o644)
    elif mutation == "staged-link":
        (root / "staged-host/launcher.py").unlink()
        (root / "staged-host/launcher.py").symlink_to(destination / "app.py")
    elif mutation == "retained-bytes": rewrite(root / "before/host/launcher.py", b"foreign retained bytes")
    elif mutation == "update": receipt["update"] = {"runtime_admission_possible": True}
    elif mutation == "candidate-publication": receipt["runtime_candidate"] = None
    rewrite(directory / "receipt.json", receipt)
    before = snapshot(destination), snapshot(directory)
    with pytest.raises(DraftConflict):
        await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"])
    assert (snapshot(destination), snapshot(directory)) == before


@pytest.mark.parametrize("observation", [
    {"loaded": True, "pid": 3456, "state": "running"},
    {"loaded": True, "pid": None, "state": "waiting"},
    {"loaded": False, "pid": None, "state": "stopped", "control_pid": 3456},
])
async def test_live_or_waiting_manager_refuses_before_any_host_move(owned, observation):
    _, manager, directory, destination, _, _, _ = owned
    result, _ = prepared(owned)
    manager.observation = observation
    before = snapshot(destination), snapshot(directory)
    with pytest.raises(DraftConflict, match="OS service is not stopped"):
        await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"])
    assert (snapshot(destination), snapshot(directory)) == before


class Crash(Exception):
    pass


@pytest.mark.parametrize("boundary", ["retaining", "old-moved", "retained", "new-moved", "installed", "publishing", "receipt-published", "completed"])
async def test_each_exact_interrupted_custody_boundary_recovers_forward_only(owned, monkeypatch, boundary):
    _, manager, directory, destination, service, old, current = owned
    result, root = prepared(owned)
    before_source, unit = snapshot(destination), Path(service["unit_path"]).read_bytes()
    retained = snapshot(root / "before")
    actual = refresh._phase

    def interrupted(root, journal, phase):
        if boundary == "old-moved" and phase == "retained" or boundary == "new-moved" and phase == "installed" or boundary == "receipt-published" and phase == "completed":
            raise Crash("Fixture crash after publication but before its journal checkpoint.")
        changed = actual(root, journal, phase)
        if boundary == phase:
            raise Crash("Fixture crash after an exact intent/checkpoint write.")
        return changed

    monkeypatch.setattr(refresh, "_phase", interrupted)
    with pytest.raises(Crash):
        await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"])
    if boundary != "completed":
        with pytest.raises(DraftConflict, match="Complete the exact supervisor custody"):
            services.verify_service(directory, deployment.read_receipt(directory))
    monkeypatch.setattr(refresh, "_phase", actual)
    finished = await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"])
    assert finished["phase"] == "completed"
    assert snapshot(root / "before") == retained
    assert {name: content for name, (content, _) in snapshot(root / "retained-host").items()} == old
    assert {name: content for name, (content, _) in snapshot(directory / "service-host").items()} == current
    assert snapshot(destination) == before_source and Path(service["unit_path"]).read_bytes() == unit
    services.verify_service(directory, deployment.read_receipt(directory))


async def test_manager_race_after_project_lease_refuses_before_host_or_receipt_publication(owned):
    _, manager, directory, destination, _, _, _ = owned
    result, _ = prepared(owned)
    actual = manager.observe

    async def raced(service):
        observation = await actual(service)
        if len(manager.calls) == 2:
            deployment.update_receipt(directory, service_instance_id="raced instance")
        return observation

    manager.observe = raced
    before_host = snapshot(directory / "service-host")
    before_source = snapshot(destination)
    with pytest.raises(DraftConflict, match="stopped service instance changed"):
        await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"])
    assert snapshot(directory / "service-host") == before_host and snapshot(destination) == before_source


async def test_prepared_source_is_used_after_current_renderer_changes(owned, monkeypatch):
    _, manager, directory, _, _, _, current = owned
    result, _ = prepared(owned)

    def forbidden():
        pytest.fail("Apply/recovery must use its retained snapshot, never rerender current source.")

    monkeypatch.setattr(services, "supervisor_source_files", forbidden)
    assert (await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"]))["phase"] == "completed"
    assert {name: content for name, (content, _) in snapshot(directory / "service-host").items()} == current


async def test_candidate_publication_after_new_receipt_never_restores_old_supervisor(owned, monkeypatch):
    _, manager, directory, destination, _, _, current = owned
    result, _ = prepared(owned)
    actual = refresh._phase

    def interrupted(root, journal, phase):
        if phase == "completed":
            raise Crash("Fixture crash after new service receipt publication.")
        return actual(root, journal, phase)

    monkeypatch.setattr(refresh, "_phase", interrupted)
    with pytest.raises(Crash):
        await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"])
    deployment.update_receipt(directory, runtime_candidate={"cannot_be_used_as_admission": True})
    monkeypatch.setattr(refresh, "_phase", actual)
    before = snapshot(destination), snapshot(directory)
    with pytest.raises(DraftConflict, match="before candidate publication"):
        await refresh.apply(directory, result["revision"], manager, expected_digest=result["descriptor_sha256"])
    assert (snapshot(destination), snapshot(directory)) == before
    assert {name: content for name, (content, _) in snapshot(directory / "service-host").items()} == current
