"""Frozen background admission with inert packages and a fake OS manager.

The metadata-only fixture retains the exact current reader/compiler closure
and generic protocol while withdrawing candidate admission metadata. A missing
required reader is a separate unsupported source closure.
No installed SDK, native executable, application, or OS service is run.
"""
from copy import deepcopy
import json
from pathlib import Path

import pytest

from launchkit import deployment, deployment_service as services
from launchkit import deployment_updates as updates
from launchkit.projects import DraftConflict, fingerprint
from launchkit.templates.storage_setup import write_private
from test_background_updates import FakeManager
from test_candidate_deployment_updates import crossing, installation, packed
from test_deployment import Guide, Implementation
from test_reading_background_runtime_refresh import reading_background


CANDIDATE_ONLY_FILES = (
    "launchkit/runtime_candidates.py", "launchkit/templates/install_candidate.py",
    "launchkit/sdk_wheel_integrity.py")


def snapshot(root):
    return {str(path.relative_to(root)): (path.read_bytes(), path.stat().st_mode & 0o777)
        for path in root.rglob("*") if path.is_file() and not path.is_symlink()}


def candidate_metadata_withdrawn_snapshot(directory):
    """Withdraw only candidate admission from a qualified current source host."""
    receipt = deployment.read_receipt(directory)
    saved = deepcopy(receipt["service"])
    saved.pop("candidate_startup_protocol")
    saved.pop("candidate_worker_sha256")
    deployment.update_receipt(directory, service=saved)
    checked = services.verify_service(directory, deployment.read_receipt(directory))
    services.require_update_protocol(checked)
    return checked


def missing_runtime_reader_snapshot(directory):
    """An owned but incomplete host cannot read the selected v4 history."""
    saved = deepcopy(candidate_metadata_withdrawn_snapshot(directory))
    name = CANDIDATE_ONLY_FILES[0]
    (directory / "service-host" / name).unlink()
    saved["host_files"].pop(name)
    deployment.update_receipt(directory, service=saved)
    checked = services.verify_service(directory, deployment.read_receipt(directory))
    services.require_update_protocol(checked)
    return checked


def missing_sdk_wheel_helper_snapshot(directory):
    """Keep current reader admission but withdraw its actual source dependency."""
    receipt = deployment.read_receipt(directory)
    saved = deepcopy(receipt["service"])
    name = CANDIDATE_ONLY_FILES[2]
    (directory / "service-host" / name).unlink()
    saved["host_files"].pop(name)
    deployment.update_receipt(directory, service=saved)
    return services.verify_service(directory, deployment.read_receipt(directory))


UNSUPPORTED_SOURCE = {
    "candidate-metadata": (candidate_metadata_withdrawn_snapshot,
        "cannot verify the selected candidate runtime"),
    "required-reader": (missing_runtime_reader_snapshot,
        "cannot verify the exact current runtime history and compiler closure"),
    "reader-dependency": (missing_sdk_wheel_helper_snapshot,
        "cannot verify the exact current runtime history and compiler closure"),
}


@pytest.fixture
def candidate_background(tmp_path, monkeypatch, crossing):
    monkeypatch.setenv("HOME", str(tmp_path / "isolated-home"))
    Path.home().mkdir()
    directory, destination = installation(tmp_path, crossing)
    receipt = deployment.read_receipt(directory)
    guide = Guide()
    guide.identity = receipt["guide_id"]
    guide.state = {"id": guide.identity, "version": receipt["guide_version"],
        "proposal": json.loads(crossing[2]["blueprint.json"][0])}
    implementation = Implementation(guide)
    owner = deployment.DeploymentService(tmp_path / "private-server", guide, implementation)
    owned = owner.root / guide.identity / receipt["deployment_id"]
    owned.parent.mkdir(parents=True)
    directory.rename(owned)
    write_private(owned.parent / "latest.json", {"deployment_id": receipt["deployment_id"]})
    directory = owned
    deployment.update_receipt(directory, execution_mode="background", phase="run",
        service_supervisor_pid=4567, service_instance_id="retained-stopped-instance",
        service_cleanup_confirmed=True, progress=[], logs=[])
    frozen = services.prepare_service(directory, deployment.read_receipt(directory))
    manager = FakeManager(directory)
    owner.service_manager = manager
    _, selected, _, candidate, _, report = crossing
    staged = updates.stage(directory, packed(candidate), report, runtime_target=selected)
    identity = staged["update"]["update_id"]
    assert staged["update"]["status"] == "staged" and not staged["update"]["blockers"]
    return owner, directory, destination, identity, manager, frozen


@pytest.mark.parametrize("support", UNSUPPORTED_SOURCE)
def test_unsupported_candidate_preview_is_retained_and_prepare_does_not_migrate(candidate_background, support):
    _, directory, destination, identity, manager, _ = candidate_background
    prepare, refusal = UNSUPPORTED_SOURCE[support]
    frozen = prepare(directory)
    before = snapshot(destination), snapshot(directory / "service-host"), Path(frozen["unit_path"]).read_bytes()
    update_before = (updates.update_directory(directory, identity) / "update.json").read_bytes()
    cap = services.background_update_capabilities(directory, deployment.read_receipt(directory))
    assert cap["background_activation_supported"] is False
    assert refusal in cap["reason"]
    assert services.prepare_service(directory, deployment.read_receipt(directory)) == frozen
    assert (snapshot(destination), snapshot(directory / "service-host"),
        Path(frozen["unit_path"]).read_bytes()) == before
    assert (updates.update_directory(directory, identity) / "update.json").read_bytes() == update_before
    assert not manager.starts and not manager.stops


@pytest.mark.parametrize("action", ["apply-update", "recover-current"])
@pytest.mark.parametrize("support", UNSUPPORTED_SOURCE)
async def test_unsupported_candidate_action_refuses_before_queue_install_or_start(candidate_background, monkeypatch, action, support):
    owner, directory, destination, identity, manager, _ = candidate_background
    prepare, refusal = UNSUPPORTED_SOURCE[support]
    frozen = prepare(directory)
    if action == "recover-current":
        _, archive, update = updates.transaction(directory, identity)
        updates.checkpoint(directory, archive, update, status="recovery-needed", blockers=[])
    installed = snapshot(destination)
    host = snapshot(directory / "service-host")
    unit = Path(frozen["unit_path"]).read_bytes()
    package = (directory / "package.zip").read_bytes()
    update_before = (updates.update_directory(directory, identity) / "update.json").read_bytes()
    before = deployment.read_receipt(directory)
    def forbidden(*args, **kwargs):
        pytest.fail("An unsupported candidate must refuse before queued admission or source effects.")
    monkeypatch.setattr(deployment, "journal", forbidden)
    monkeypatch.setattr(owner, "_launch", forbidden)
    monkeypatch.setattr(updates, "activate", forbidden)
    monkeypatch.setattr(updates, "retry_candidate", forbidden)
    with pytest.raises(DraftConflict, match=refusal):
        await owner.action(owner.guide.identity, before["deployment_id"], action, update_id=identity)
    after = deployment.read_receipt(directory)
    for name in ("build_id", "package_sha256", "status", "phase", "progress", "update"):
        assert after[name] == before[name]
    assert "update_action" not in after and "runtime_candidate" not in after
    assert snapshot(destination) == installed and snapshot(directory / "service-host") == host
    assert Path(frozen["unit_path"]).read_bytes() == unit
    assert (directory / "package.zip").read_bytes() == package
    assert (updates.update_directory(directory, identity) / "update.json").read_bytes() == update_before
    assert not manager.starts and not manager.stops and not owner.tasks


def test_new_frozen_candidate_supervisor_supports_exact_staged_runtime(candidate_background):
    _, directory, destination, _, manager, frozen = candidate_background
    before = snapshot(destination), snapshot(directory / "service-host"), Path(frozen["unit_path"]).read_bytes()
    assert type(frozen["candidate_startup_protocol"]) is int
    assert frozen["candidate_worker_sha256"] == frozen["host_files"]["launchkit/deployment_worker.py"]
    assert all(name in frozen["host_files"] for name in CANDIDATE_ONLY_FILES)
    cap = services.background_update_capabilities(directory, deployment.read_receipt(directory))
    assert cap["background_activation_supported"] is True and cap["requires_stopped"] is True
    assert services.prepare_service(directory, deployment.read_receipt(directory)) == frozen
    assert (snapshot(destination), snapshot(directory / "service-host"),
        Path(frozen["unit_path"]).read_bytes()) == before
    assert not manager.starts and not manager.stops


@pytest.fixture
def current_candidate_background(candidate_background, crossing):
    """Bind the selected inert build to actual final acceptance/guide checks."""
    owner, directory, destination, _, manager, frozen = candidate_background
    _, selected, _, source, _, accepted = crossing
    candidate, report = deepcopy(source), deepcopy(accepted)
    report["brief_hash"] = fingerprint(owner.guide.state.get("brief") or {})
    candidate["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    owner.implementation.identity = report["build_id"]
    owner.implementation.digest = report["verification"]["digest"]
    staged = updates.stage(directory, packed(candidate), report, runtime_target=selected)
    assert staged["update"]["status"] == "staged" and not staged["update"]["blockers"]
    return owner, directory, destination, staged["update"]["update_id"], manager, frozen


@pytest.mark.parametrize("fence", ["fresh-stopped", "final-acceptance"])
async def test_candidate_support_changed_after_projection_refuses_before_queue(current_candidate_background, monkeypatch, fence):
    owner, directory, destination, identity, manager, frozen = current_candidate_background
    before = deployment.read_receipt(directory)
    installed, host = snapshot(destination), snapshot(directory / "service-host")
    unit, package = Path(frozen["unit_path"]).read_bytes(), (directory / "package.zip").read_bytes()
    update_before = (updates.update_directory(directory, identity) / "update.json").read_bytes()
    observations, acceptances, withdrawals = [], [], []
    actual_observe = manager.observe
    actual_status = owner.status
    actual_acceptance = owner.implementation.status
    actual_stopped = services.require_stopped_service
    observation_stage = "status-projection"

    async def observe(service):
        observations.append(observation_stage)
        return await actual_observe(service)

    async def stopped(*args, **kwargs):
        nonlocal observation_stage
        prior = observation_stage
        observation_stage = "stopped-admission"
        try:
            return await actual_stopped(*args, **kwargs)
        finally:
            observation_stage = prior

    def withdraw_support():
        saved = deepcopy(deployment.read_receipt(directory)["service"])
        saved.pop("candidate_startup_protocol")
        saved.pop("candidate_worker_sha256")
        deployment.update_receipt(directory, service=saved)
        checked = services.verify_service(directory, deployment.read_receipt(directory))
        services.require_update_protocol(checked)
        withdrawals.append("candidate-capability-withdrawn")

    async def status(guide_id, **kwargs):
        projected = await actual_status(guide_id, **kwargs)
        assert projected["update_capabilities"]["background_activation_supported"] is True
        if fence == "fresh-stopped":
            withdraw_support()
        return projected

    async def accepted(guide_id):
        checked = await actual_acceptance(guide_id)
        acceptances.append("accepted-current-build")
        if fence == "final-acceptance" and len(acceptances) == 2:
            withdraw_support()
        return checked

    def forbidden(*args, **kwargs):
        pytest.fail("Changed candidate support must refuse before queued admission or source effects.")

    monkeypatch.setattr(manager, "observe", observe)
    monkeypatch.setattr(services, "require_stopped_service", stopped)
    monkeypatch.setattr(owner, "status", status)
    monkeypatch.setattr(owner.implementation, "status", accepted)
    monkeypatch.setattr(deployment, "journal", forbidden)
    monkeypatch.setattr(owner, "_launch", forbidden)
    monkeypatch.setattr(updates, "activate", forbidden)
    monkeypatch.setattr(updates, "retry_candidate", forbidden)
    refusal = ("cannot verify the selected candidate runtime" if fence == "fresh-stopped"
        else "The installed source or runtime owner changed during inspection")
    with pytest.raises(DraftConflict, match=refusal):
        await owner.action(owner.guide.identity, before["deployment_id"], "apply-update", update_id=identity)
    assert len(withdrawals) == 1
    assert len(acceptances) == (0 if fence == "fresh-stopped" else 2)
    # Status projects both OS health and support ownership before action
    # admission. The latter then performs its original stopped-service fences.
    assert observations == ["status-projection", "status-projection"] + [
        "stopped-admission"] * (1 if fence == "fresh-stopped" else 3)
    after = deployment.read_receipt(directory)
    for name in ("build_id", "package_sha256", "status", "phase", "progress", "update"):
        assert after[name] == before[name]
    assert "update_action" not in after and "runtime_candidate" not in after
    assert snapshot(destination) == installed and snapshot(directory / "service-host") == host
    assert Path(frozen["unit_path"]).read_bytes() == unit
    assert (directory / "package.zip").read_bytes() == package
    assert (updates.update_directory(directory, identity) / "update.json").read_bytes() == update_before
    assert not manager.starts and not manager.stops and not owner.tasks


@pytest.mark.parametrize("defect", ["null", "bool", "worker", "missing-module"])
def test_candidate_support_requires_its_owned_frozen_binding(candidate_background, defect):
    _, directory, _, _, manager, _ = candidate_background
    saved = deepcopy(deployment.read_receipt(directory)["service"])
    if defect == "null":
        saved["candidate_startup_protocol"] = None
    elif defect == "bool":
        saved["candidate_startup_protocol"] = True
    elif defect == "worker":
        saved["candidate_worker_sha256"] = "f" * 64
    else:
        name = CANDIDATE_ONLY_FILES[1]
        (directory / "service-host" / name).unlink()
        saved["host_files"].pop(name)
    deployment.update_receipt(directory, service=saved)
    checked = services.verify_service(directory, deployment.read_receipt(directory))
    services.require_update_protocol(checked)
    cap = services.background_update_capabilities(directory, deployment.read_receipt(directory))
    assert cap["background_activation_supported"] is False
    refusal = ("cannot verify the exact current runtime history and compiler closure"
        if defect == "missing-module" else "cannot verify the selected candidate runtime")
    assert refusal in cap["reason"]
    assert not manager.starts and not manager.stops


async def test_official_047_with_full_source_without_candidate_metadata_reaches_queued_admission(reading_background, monkeypatch):
    owner, directory, destination, old, identity, manager, _ = reading_background
    frozen = candidate_metadata_withdrawn_snapshot(directory)
    before = snapshot(destination), snapshot(directory / "service-host"), Path(frozen["unit_path"]).read_bytes()
    descriptor = deployment.read_receipt(directory)["update"]["runtime_dependency_update"]
    assert descriptor["to_version"] == "0.8.47" and descriptor.get("runtime_candidate") is None
    assert descriptor["version"] == 4
    assert set(descriptor["required_supervisor_sources"]) == set(updates._runtime_supervisor_paths(4))
    assert all(frozen["host_files"][name] == expected
        for name, expected in descriptor["required_supervisor_sources"].items())
    cap = services.background_update_capabilities(directory, deployment.read_receipt(directory))
    assert cap["background_activation_supported"] is True
    launches = []
    def launch(root, mode, *, update_selection):
        selected = updates.read_update_admission(root, mode, update_selection)
        assert selected["binding"]["update_id"] == identity and selected["mode"] == "update"
        launches.append((root, mode))
    monkeypatch.setattr(owner, "_launch", launch)
    admitted = await owner.action(owner.guide.identity, old["deployment_id"], "apply-update", update_id=identity)
    assert launches == [(directory, "update")]
    # The returned view follows the fake manager's stopped observation. The
    # saved admission is queued; no actual worker was launched by this fixture.
    saved = deployment.read_receipt(directory)
    assert saved["status"] == "queued" and saved["phase"] == "update"
    assert admitted["update_action"] == {"action": "apply-update", "update_id": identity}
    assert admitted["update"]["status"] == "staged"
    assert (snapshot(destination), snapshot(directory / "service-host"),
        Path(frozen["unit_path"]).read_bytes()) == before
    assert not manager.starts and not manager.stops
