"""Inert support GET responsiveness and exact late read fences.

Only owned tmp fixtures are inspected. No installed application, native binary,
service manager or provider is executed by these checks.
"""
import asyncio
from copy import deepcopy
import json
from pathlib import Path
import threading

import pytest

from launchkit import deployment, deployment_supervisor as refresh
from launchkit.projects import DraftConflict
from test_deployment_supervisor import (owned, crossing, candidate_owned,
    maintained_owned, prepared, snapshot, rewrite, trusted_supervisor_sources,
    current_admission_owned, historical_candidate_owned, historical_candidate_crossing,
    retained_candidate_v1)


async def test_support_read_keeps_loop_responsive(owned, monkeypatch):
    _, manager, directory, _, _, _, _ = owned
    original = refresh._ordinary_source
    entered, release, finished = threading.Event(), threading.Event(), threading.Event()

    def slow_read(*args):
        entered.set()
        release.wait(2)
        try:
            return original(*args)
        finally:
            finished.set()

    monkeypatch.setattr(refresh, "_ordinary_source", slow_read)
    projection = asyncio.create_task(refresh.support_projection(directory, manager))
    try:
        # A busy runner need not start the worker within a fixed 10ms window.
        # Observe its actual entry, then require a loop turn while it is held.
        async with asyncio.timeout(30):
            while not entered.is_set():
                await asyncio.sleep(0)
        await asyncio.sleep(0)
        responsive = not finished.is_set() and not projection.done()
    finally:
        release.set()
        value = await projection
    assert responsive, "Support validation blocked the event loop before this heartbeat."
    assert value["stopped_verified"] is True and value["prepare_available"] is True


async def test_support_read_cancellation_drains_its_only_worker(owned, monkeypatch):
    _, manager, directory, destination, _, _, _ = owned
    original = refresh._ordinary_source
    entered, release, finished = threading.Event(), threading.Event(), threading.Event()
    before = snapshot(directory), snapshot(destination)

    def held_read(*args):
        entered.set()
        release.wait(2)
        try:
            return original(*args)
        finally:
            finished.set()

    monkeypatch.setattr(refresh, "_ordinary_source", held_read)
    projection = asyncio.create_task(refresh.support_projection(directory, manager))
    try:
        async with asyncio.timeout(30):
            while not entered.is_set():
                await asyncio.sleep(0.005)
        projection.cancel()
        await asyncio.sleep(0.01)
        assert not projection.done() and not finished.is_set()
        projection.cancel()
        await asyncio.sleep(0.01)
        assert not projection.done() and not finished.is_set()
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError):
        await projection
    assert finished.is_set() and len(manager.calls) == 1
    assert (snapshot(directory), snapshot(destination)) == before


@pytest.mark.parametrize("phase", ["none", "prepared", "completed"])
async def test_support_read_uses_one_source_proof_for_exact_custody(owned, monkeypatch, phase):
    _, manager, directory, destination, _, _, _ = owned
    if phase != "none":
        selected, _ = prepared(owned)
        if phase == "completed":
            await refresh.apply(directory, selected["revision"], manager,
                expected_digest=selected["descriptor_sha256"])
    original, calls = refresh._ordinary_source, []
    loop_thread = threading.get_ident()
    callback_threads = []

    def observed(*args):
        calls.append(threading.get_ident())
        return original(*args)

    monkeypatch.setattr(refresh, "_ordinary_source", observed)
    before = snapshot(directory), snapshot(destination)
    value = await refresh.support_projection(directory, manager,
        require_current=lambda: callback_threads.append(threading.get_ident()))
    assert len(calls) == 1 and calls[0] != loop_thread
    assert callback_threads and set(callback_threads) == {loop_thread}
    assert value["stopped_verified"] is True
    assert value["prepare_available"] is (phase == "none")
    assert value["apply_available"] is (phase == "prepared")
    assert (snapshot(directory), snapshot(destination)) == before


async def test_support_read_checks_retained_candidate_physical_proof_once(maintained_owned, monkeypatch):
    owned, _ = maintained_owned
    _, manager, directory, destination, _, _, _ = owned
    original, calls = refresh.verify_runtime, []

    def physical(*args, **kwargs):
        calls.append((args, kwargs))
        return original(*args, **kwargs)

    monkeypatch.setattr(refresh, "verify_runtime", physical)
    before = snapshot(directory), snapshot(destination)
    value = await refresh.support_projection(directory, manager)
    assert value["stopped_verified"] is True and value["prepare_available"] is True
    assert value["selected"]["phase"] == "completed" and len(calls) == 1
    assert (snapshot(directory), snapshot(destination)) == before


@pytest.mark.parametrize("mutation", ["status", "runtime", "cleanup", "build", "service", "selector",
    "startup-proof", "package", "installed-source", "unit", "custody-phase", "custody-bytes"])
async def test_support_read_refuses_changes_at_worker_handback(owned, monkeypatch, mutation):
    _, manager, directory, destination, service, _, _ = owned
    _, root = prepared(owned)
    validation = refresh.await_validation
    expected = []

    async def changed(function, *args, **kwargs):
        result = await validation(function, *args, **kwargs)
        if function is refresh._ordinary_source:
            receipt = deployment.read_receipt(directory)
            if mutation == "status": receipt["status"] = "running"
            elif mutation == "runtime": receipt["runtime_pid"] = 4321
            elif mutation == "cleanup": receipt["service_cleanup_confirmed"] = False
            elif mutation == "build": receipt["build_id"] = "foreign-build"
            elif mutation == "service": receipt["service"]["interpreter"] += "-foreign"
            elif mutation == "selector": receipt["supervisor_refresh"]["descriptor_sha256"] = "f" * 64
            elif mutation == "startup-proof": receipt["startup_candidate_runtime_proof"] = {"untrusted": True}
            elif mutation == "package":
                package = directory / "package.zip"
                package.write_bytes(package.read_bytes() + b"changed")
            elif mutation == "installed-source":
                source = destination / "deployment.json"
                source.write_bytes(source.read_bytes() + b"\n")
            elif mutation == "unit": Path(service["unit_path"]).write_bytes(b"Foreign unit\n")
            elif mutation == "custody-phase":
                journal = json.loads((root / "journal.json").read_text())
                journal["phase"] = "retaining"
                rewrite(root / "journal.json", journal)
            else:
                source = root / "after/host/launchkit/deployment_worker.py"
                rewrite(source, source.read_bytes() + b"# Changed archived custody\n")
            if mutation in {"status", "runtime", "cleanup", "build", "service", "selector", "startup-proof"}:
                deployment.write_private(directory / "receipt.json", receipt)
            expected.append((snapshot(directory), snapshot(destination), Path(service["unit_path"]).read_bytes()))
        return result

    monkeypatch.setattr(refresh, "await_validation", changed)
    value = await refresh.support_projection(directory, manager)
    assert not value["available"] and not value["stopped_verified"] and value["reason"]
    assert len(expected) == 1
    assert (snapshot(directory), snapshot(destination), Path(service["unit_path"]).read_bytes()) == expected[0]


@pytest.mark.parametrize("mutation", ["sdk", "native"])
async def test_support_read_still_verifies_installed_runtime_after_manager_await(candidate_owned, monkeypatch, mutation):
    _, manager, directory, destination, _, _, _ = candidate_owned
    observe = manager.observe
    changed = []

    async def drift(service):
        result = await observe(service)
        if mutation == "sdk":
            source = destination / ".venv/lib/python3.14/site-packages/meerkat_mobkit/__init__.py"
        else:
            source = destination / ".bin/rpc_gateway"
        source.write_bytes(source.read_bytes() + b"# Changed installed bytes\n")
        changed.append((snapshot(directory), snapshot(destination)))
        return result

    monkeypatch.setattr(manager, "observe", drift)
    value = await refresh.support_projection(directory, manager)
    assert not value["available"] and not value["stopped_verified"] and value["reason"]
    assert len(changed) == 1 and (snapshot(directory), snapshot(destination)) == changed[0]


async def test_support_read_late_guide_selection_refuses_after_worker_without_writes(owned, monkeypatch):
    _, manager, directory, destination, _, _, _ = owned
    validation = refresh.await_validation
    switched = False
    loop_thread = threading.get_ident()

    async def changed(function, *args, **kwargs):
        nonlocal switched
        result = await validation(function, *args, **kwargs)
        if function is refresh._ordinary_source:
            switched = True
        return result

    def current():
        assert threading.get_ident() == loop_thread
        if switched:
            raise DraftConflict("A newer deployment is selected.")

    monkeypatch.setattr(refresh, "await_validation", changed)
    before = snapshot(directory), snapshot(destination)
    value = await refresh.support_projection(directory, manager, require_current=current)
    assert not value["available"] and not value["stopped_verified"]
    assert (snapshot(directory), snapshot(destination)) == before


@pytest.mark.parametrize("observation", [
    {"loaded": True, "pid": 4321, "state": "running"},
    {"loaded": True, "pid": None, "control_pid": 4321, "state": "stopped"},
    {"loaded": True, "pid": None, "control_group": "owned-group", "state": "inactive"},
    {"loaded": True, "pid": None, "state": "waiting"},
])
async def test_support_read_rejects_manager_work_even_with_clean_receipt(owned, observation):
    _, manager, directory, destination, _, _, _ = owned
    manager.observation = observation
    before = snapshot(directory), snapshot(destination)
    value = await refresh.support_projection(directory, manager)
    assert not value["available"] and not value["stopped_verified"]
    assert (snapshot(directory), snapshot(destination)) == before
