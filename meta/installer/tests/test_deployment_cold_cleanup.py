"""Offline temporary-package regressions, with mocked process observations."""
import asyncio
from contextlib import contextmanager
import json
from types import SimpleNamespace
from unittest.mock import AsyncMock
import uuid

import pytest

from launchkit import deployment
from launchkit.deployment import read_receipt, update_receipt, OWNER_FILE
from launchkit.projects import DraftConflict
from test_deployment import Service
from test_deployment_failed_cleanup import bound_failed

@pytest.fixture(autouse=True)
def synthetic_process_absence_only(monkeypatch):
    """Mock process observation for this module's disposable package fixtures."""
    calls = []

    def absent(pid, number):
        calls.append((pid, number))
        assert number == 0
        if pid == 99999999:
            raise ProcessLookupError
        return None

    monkeypatch.setattr(deployment.os, "kill", absent)
    return calls


async def cold_fixture(tmp_path, status="interrupted"):
    held, directory, target, initial, cleanup = await bound_failed(tmp_path)
    cleanup = {**cleanup, "worker_exit_confirmed": False}
    update_receipt(directory, status=status, process_live=False, runtime_pid=None,
                   interactive_cleanup=cleanup)
    # A fresh service has no inherited process handles, tasks or admission facts.
    cold = Service(held.root.parent, held.guide, held.implementation)
    return cold, directory, target, initial, cleanup


def preserved(target, directory):
    return {path: path.read_bytes() for path in [target / "app.py", target / "deployment.json",
        target / OWNER_FILE, target / ".env", target / "data/records.json",
        target / ".mobkit/state/native.json", directory / "package.zip"]}


@pytest.mark.parametrize("status", ["failed", "interrupted"])
async def test_explicit_cold_stop_records_absence_without_inventing_exit_code(tmp_path, status,
        synthetic_process_absence_only):
    service, directory, target, initial, cleanup = await cold_fixture(tmp_path, status)
    saved = preserved(target, directory)
    before = (directory / "receipt.json").read_bytes()
    assert (await service.status(service.guide.identity))["cleanup_recovery"]["available"] is False
    assert (directory / "receipt.json").read_bytes() == before
    stopped = await service.action(service.guide.identity, initial["deployment_id"], "stop")
    result = stopped["interactive_cleanup"]
    assert stopped["status"] == "stopped" and stopped["runtime_pid"] is None
    assert stopped["process_live"] is False
    assert result["worker_exit_confirmed"] is False
    assert result["supervisor_absence_confirmed"] is True
    assert result["supervisor_absence_scope"] == "cold-host-owned-native-teardown"
    assert result["supervisor_absence_observed_at"]
    assert {key: result[key] for key in cleanup} == cleanup
    assert deployment.interactive_cleanup_matches(stopped)
    assert all(path.read_bytes() == body for path, body in saved.items())
    assert synthetic_process_absence_only and all(pair == (99999999, 0) for pair in synthetic_process_absence_only)


async def test_canonical_restart_uses_same_installed_source_then_discards_old_proof(tmp_path):
    service, directory, target, initial, cleanup = await cold_fixture(tmp_path)
    saved = preserved(target, directory)
    try:
        returned = await service.action(service.guide.identity, initial["deployment_id"], "restart")
        assert returned["status"] == "queued" and returned["phase"] == "start"
        assert returned["build_id"] == initial["build_id"]
        assert returned["package_sha256"] == initial["package_sha256"]
        assert returned["interactive_lifecycle_id"] != cleanup["lifecycle_id"]
        assert returned["interactive_cleanup"] is None
        assert returned["runtime_pid"] is None and returned["health"] == "unknown"
        assert all(path.read_bytes() == body for path, body in saved.items())
        service.implementation.package.assert_not_awaited()
    finally:
        await service.shutdown()


@pytest.mark.parametrize("field,value", [
    ("confirmed", False), ("lifecycle_id", str(uuid.uuid4())), ("build_id", str(uuid.uuid4())),
    ("package_sha256", "b" * 64), ("source_digest", "b" * 64), ("owner_sha256", "b" * 64),
    ("startup_attempt_id", "different-attempt"), ("supervisor_pid", True),
    ("return_code", -9), ("boundary", "worker-not-admitted"), ("worker_exit_confirmed", None),
])
async def test_absent_supervisor_cannot_promote_invalid_native_teardown(tmp_path, field, value):
    service, directory, target, initial, cleanup = await cold_fixture(tmp_path)
    update_receipt(directory, interactive_cleanup={**cleanup, field: value})
    before = (directory / "receipt.json").read_bytes()
    saved = preserved(target, directory)
    for action in ("stop", "retry", "restart"):
        with pytest.raises((DraftConflict, ValueError)):
            await service.action(service.guide.identity, initial["deployment_id"], action)
    assert (directory / "receipt.json").read_bytes() == before
    assert all(path.read_bytes() == body for path, body in saved.items())


@pytest.mark.parametrize("change", ["source", "owner", "package", "lease", "selected", "service"])
async def test_cold_reconciliation_cannot_bypass_source_or_ownership_fences(tmp_path, change):
    service, directory, target, initial, _ = await cold_fixture(tmp_path)
    if change == "source": (target / "app.py").write_text("edited source")
    if change == "owner": (target / OWNER_FILE).write_text("{}")
    if change == "package": (directory / "package.zip").write_bytes(b"changed")
    if change == "selected": (directory.parent / "latest.json").write_text(json.dumps({"deployment_id": str(uuid.uuid4())}))
    if change == "service": update_receipt(directory, service={"manager": "foreign"})
    before = (directory / "receipt.json").read_bytes()
    saved = preserved(target, directory)
    if change == "lease":
        with deployment.project_lock(target), pytest.raises((DraftConflict, ValueError)):
            await service._stop_failed_interactive(service.guide.identity, initial["deployment_id"])
    else:
        with pytest.raises((DraftConflict, ValueError)):
            await service._stop_failed_interactive(service.guide.identity, initial["deployment_id"])
    assert (directory / "receipt.json").read_bytes() == before
    assert all(path.read_bytes() == body for path, body in saved.items())


@pytest.mark.parametrize("observation", ["live", "permission", "overflow"])
async def test_live_or_uncertain_supervisor_never_records_absence(tmp_path, monkeypatch, observation):
    service, directory, _, initial, _ = await cold_fixture(tmp_path)
    def observe(*_):
        if observation == "permission": raise PermissionError("synthetic observation failure")
        if observation == "overflow": raise OverflowError("synthetic observation failure")
    monkeypatch.setattr(deployment.os, "kill", observe)
    before = (directory / "receipt.json").read_bytes()
    with pytest.raises(DraftConflict):
        await service.action(service.guide.identity, initial["deployment_id"], "stop")
    assert (directory / "receipt.json").read_bytes() == before


@pytest.mark.parametrize("known", ["admission", "killed-process", "task"])
async def test_current_host_facts_cannot_be_overridden_by_cold_reconciliation(tmp_path, known):
    service, directory, _, initial, cleanup = await cold_fixture(tmp_path)
    identity = initial["deployment_id"]
    if known == "admission":
        service.worker_admissions[identity] = {"lifecycle_id": cleanup["lifecycle_id"], "spawn_started": True}
    elif known == "killed-process":
        service.processes[identity] = SimpleNamespace(pid=cleanup["supervisor_pid"], returncode=-9,
            wait=AsyncMock(return_value=-9))
    else:
        task = asyncio.create_task(asyncio.sleep(0))
        service.tasks[identity] = task
    before = (directory / "receipt.json").read_bytes()
    try:
        with pytest.raises(DraftConflict, match="cleanup is unconfirmed"):
            await service.action(service.guide.identity, identity, "stop")
        assert (directory / "receipt.json").read_bytes() == before
    finally:
        if known == "task": await task
        service.tasks.clear()
        service.processes.clear()
        service.worker_admissions.clear()


async def test_receipt_change_at_project_lease_boundary_prevents_promotion(tmp_path, monkeypatch):
    service, directory, _, initial, cleanup = await cold_fixture(tmp_path)
    original = deployment.project_lock
    @contextmanager
    def changed(destination):
        with original(destination):
            update_receipt(directory, interactive_cleanup={**cleanup, "source_digest": "b" * 64})
            yield
    monkeypatch.setattr(deployment, "project_lock", changed)
    with pytest.raises(DraftConflict):
        await service.action(service.guide.identity, initial["deployment_id"], "stop")
    final = read_receipt(directory)
    assert final["status"] == "interrupted"
    assert final["interactive_cleanup"]["worker_exit_confirmed"] is False
    assert "supervisor_absence_confirmed" not in final["interactive_cleanup"]


async def test_failed_absence_receipt_write_does_not_publish_stopped_or_completion(tmp_path, monkeypatch):
    service, directory, _, initial, _ = await cold_fixture(tmp_path)
    before = (directory / "receipt.json").read_bytes()
    original = deployment.write_private
    def failed(path, body):
        if body.get("interactive_cleanup", {}).get("supervisor_absence_confirmed") is True:
            raise OSError("synthetic write failure")
        return original(path, body)
    monkeypatch.setattr(deployment, "write_private", failed)
    with pytest.raises(OSError):
        await service.action(service.guide.identity, initial["deployment_id"], "stop")
    assert (directory / "receipt.json").read_bytes() == before


async def test_reconciled_absence_is_rechecked_if_supervisor_reappears(tmp_path, monkeypatch):
    service, directory, target, initial, _ = await cold_fixture(tmp_path)
    stopped = await service.action(service.guide.identity, initial["deployment_id"], "stop")
    assert stopped["interactive_cleanup"]["supervisor_absence_confirmed"] is True
    before = (directory / "receipt.json").read_bytes()
    monkeypatch.setattr(deployment.os, "kill", lambda *_: None)
    service._launch = lambda *_: pytest.fail("a now-live supervisor must not be replaced")
    with pytest.raises(DraftConflict, match="has not confirmed absence"):
        await service.settings_options(service.guide.identity, initial["deployment_id"])
    with pytest.raises(DraftConflict, match="has not confirmed absence"):
        await service.action(service.guide.identity, initial["deployment_id"], "restart")
    assert (directory / "receipt.json").read_bytes() == before
