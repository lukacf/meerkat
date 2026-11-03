"""Awaited manager observations cannot restore a replaced frozen supervisor.

Only isolated owned source/receipt bytes and a fake manager are exercised. No
operating-system service, SDK, native executable, application or provider runs.
"""
import asyncio
from copy import deepcopy
from pathlib import Path

import pytest

from launchkit import deployment, deployment_service as services
from test_deployment_service import owned


def snapshot(root):
    return {str(path.relative_to(root)): (path.read_bytes(), path.stat().st_mode & 0o777)
        for path in root.rglob("*") if path.is_file() and not path.is_symlink()}


def replace_fixture_supervisor(directory, original):
    """Publish inert replacement source and an independently valid inventory."""
    worker = directory / "service-host/launchkit/deployment_worker.py"
    worker.write_bytes(worker.read_bytes() + b"\n# Inert replacement supervisor fixture; never execute.\n")
    replacement = deepcopy(original)
    replacement["host_files"]["launchkit/deployment_worker.py"] = services.digest(worker.read_bytes())
    replacement["update_worker_sha256"] = replacement["host_files"]["launchkit/deployment_worker.py"]
    replacement["candidate_worker_sha256"] = replacement["host_files"]["launchkit/deployment_worker.py"]
    replacement["public_summary"] = {"manager": replacement["manager"], "state": "stopped",
        "pid": None, "definition_verified": True, "persistence": replacement["persistence"]}
    deployment.journal(directory, "The inert replacement supervisor fixture is retained.",
        service=replacement, status="stopped", phase="run", health="unknown", process_live=False,
        runtime_pid=None, bootstrap=None, console_url=None,
        service_observation={"owned": True, "state": "stopped", "pid": None,
            "checked_at": "replacement-fixture-observation"})
    checked = services.verify_service(directory, deployment.read_receipt(directory))
    services.require_update_protocol(checked)
    assert services.service_binding(checked) != services.service_binding(original)
    return checked


@pytest.mark.parametrize("query_result", ["success", "error"])
async def test_awaited_old_observation_preserves_replacement_binding_and_journal(owned, monkeypatch, query_result):
    _, manager, directory, frozen = owned
    deployment.update_receipt(directory, status="stopped", phase="run", service_cleanup_confirmed=True,
        service_supervisor_pid=None, runtime_pid=None, bootstrap=None, progress=[])
    destination = Path(deployment.read_receipt(directory)["destination"])
    installed, unit = snapshot(destination), Path(frozen["unit_path"]).read_bytes()
    started, resume = asyncio.Event(), asyncio.Event()
    observed_bindings = []

    async def observe(service):
        observed_bindings.append(services.service_binding(service))
        manager.calls.append(("observe", service["label"]))
        started.set()
        await resume.wait()
        if query_result == "error":
            raise RuntimeError("The old fake manager query failed after source replacement.")
        return {"loaded": False, "pid": None, "state": "stopped"}

    async def forbidden(*args, **kwargs):
        pytest.fail("A status observation cannot spawn an OS/native/application process.")

    monkeypatch.setattr(manager, "observe", observe)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", forbidden)
    task = asyncio.create_task(services.background_status(directory, manager))
    try:
        await asyncio.wait_for(started.wait(), timeout=30)
        replacement = replace_fixture_supervisor(directory, frozen)
        published = (directory / "receipt.json").read_bytes()
        host = snapshot(directory / "service-host")
        resume.set()
        result = await asyncio.wait_for(task, timeout=2)
    finally:
        resume.set()
        if not task.done():
            task.cancel()
        await asyncio.gather(task, return_exceptions=True)

    assert observed_bindings == [services.service_binding(frozen)]
    assert services.service_binding(result["service"]) == services.service_binding(replacement)
    assert result["health"] == "unknown" and result["process_live"] is False and result["console_url"] is None
    assert (directory / "receipt.json").read_bytes() == published
    canonical = deployment.read_receipt(directory)
    assert canonical["service"] == replacement
    assert canonical["detail"] == "The inert replacement supervisor fixture is retained."
    assert canonical["progress"][-1]["message"] == canonical["detail"]
    assert canonical["service_observation"]["checked_at"] == "replacement-fixture-observation"
    assert services.verify_service(directory, canonical) == replacement
    assert snapshot(directory / "service-host") == host and snapshot(destination) == installed
    assert Path(frozen["unit_path"]).read_bytes() == unit
    assert not any(call[0] in {"start", "stop"} for call in manager.calls)


async def test_stable_binding_records_ordinary_owned_stopped_observation(owned, monkeypatch):
    _, manager, directory, frozen = owned
    before = deployment.update_receipt(directory, status="stopped", phase="run", service_cleanup_confirmed=True,
        service_supervisor_pid=None, runtime_pid=None, bootstrap=None,
        progress=[{"at": "retained-fixture-time", "message": "Retained ordinary progress."}],
        detail="Retained ordinary detail.")
    destination = Path(before["destination"])
    installed, host, unit = snapshot(destination), snapshot(directory / "service-host"), Path(frozen["unit_path"]).read_bytes()

    async def forbidden(*args, **kwargs):
        pytest.fail("A status observation cannot spawn an OS/native/application process.")

    monkeypatch.setattr(asyncio, "create_subprocess_exec", forbidden)
    result = await services.background_status(directory, manager)
    canonical = deployment.read_receipt(directory)
    assert services.service_binding(canonical["service"]) == services.service_binding(frozen)
    assert canonical["service"]["public_summary"]["definition_verified"] is True
    assert canonical["service_observation"]["owned"] is True
    assert canonical["service_observation"]["state"] == "stopped" and canonical["service_observation"]["pid"] is None
    assert canonical["progress"] == before["progress"] and canonical["detail"] == before["detail"]
    assert result["status"] == "stopped" and result["health"] == "unavailable"
    assert result["process_live"] is False and result["console_url"] is None
    assert services.verify_service(directory, canonical)
    assert snapshot(destination) == installed and snapshot(directory / "service-host") == host
    assert Path(frozen["unit_path"]).read_bytes() == unit
    assert not any(call[0] in {"start", "stop"} for call in manager.calls)
