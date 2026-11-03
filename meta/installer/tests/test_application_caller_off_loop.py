"""Joined filesystem custody controls; the synthetic gateway never executes."""
import asyncio
import base64
import json
from pathlib import Path
import threading

import pytest

from launchkit import connect_installer_caller as caller
from test_application_inventory import Fixture


def block_phase(monkeypatch, f, phase, block):
    read, verify, write = caller.ExactFile.read, caller.ExactFile.verify, caller._write_new
    reads = 0

    def observed_read(pin, **kwargs):
        if phase == "input" and pin == f.compiler_input and not reads:
            block()
        return read(pin, **kwargs)

    def observed_verify(pin, **kwargs):
        nonlocal reads
        if pin == f.gateway:
            reads += 1
        if ((phase == "gateway" and pin == f.gateway and reads == 1)
                or (phase == "post" and pin == f.gateway and reads == 2)):
            block()
        return verify(pin, **kwargs)

    def observed_write(path, value):
        if (phase == "claim" and Path(path) == f.attempt
                or phase == "completion" and str(path) == str(f.attempt) + ".outcome.json"):
            block()
        return write(path, value)

    monkeypatch.setattr(caller.ExactFile, "read", observed_read)
    monkeypatch.setattr(caller.ExactFile, "verify", observed_verify)
    monkeypatch.setattr(caller, "_write_new", observed_write)


def operation(f):
    return f.caller.lower_application_inventory(f.compiler_input,
        attempt_path=f.attempt, source_files=(f.source,))


@pytest.mark.parametrize("phase", ["input", "gateway", "post", "claim", "completion"])
@pytest.mark.asyncio
async def test_filesystem_sections_leave_event_loop_responsive(tmp_path, monkeypatch, phase):
    f = Fixture(tmp_path)
    loop = asyncio.get_running_loop()
    progressed, observed = threading.Event(), []

    def blocked_io():
        loop.call_soon_threadsafe(progressed.set)
        observed.append(progressed.wait(0.2))

    block_phase(monkeypatch, f, phase, blocked_io)
    original_current = f.caller.require_current

    def current_on_owner_loop():
        assert asyncio.get_running_loop() is loop
        original_current()

    f.caller.require_current = current_on_owner_loop
    result = await operation(f)
    assert observed and all(observed), "The loop must run while the original filesystem section waits."
    assert result.status == "receipt_verified" and result.receipt == f.receipt
    assert len(f.calls) == 1
    assert f.completion()["receipt"] == f.receipt


@pytest.mark.parametrize("phase", ["claim", "post", "completion"])
@pytest.mark.asyncio
async def test_repeated_cancellation_joins_exact_entered_worker(tmp_path, monkeypatch, phase):
    f = Fixture(tmp_path)
    entered, release, exited = threading.Event(), threading.Event(), threading.Event()

    def held_io():
        entered.set()
        try:
            assert release.wait(2), "The controlled worker must be released by its owner."
        finally:
            exited.set()

    block_phase(monkeypatch, f, phase, held_io)
    task = asyncio.create_task(operation(f))
    try:
        assert await asyncio.to_thread(entered.wait, 30)
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done(), "Waiter cancellation cannot release the live filesystem owner."
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
    finally:
        release.set()
        if not task.done():
            await asyncio.gather(task, return_exceptions=True)
    assert exited.is_set()
    entered_record = json.loads(f.attempt.read_bytes())
    assert entered_record["input_files"] == [pin.pin() for pin in (f.gateway, f.compiler_input, f.source)]
    saved = f.completion()
    assert len(f.calls) == (0 if phase == "claim" else 1)
    if phase != "completion":
        assert saved["status"] == "outcome_unknown" and saved["error_type"] == "CancelledError"
    if phase != "claim":
        assert base64.b64decode(saved["process"]["stdout_base64"]) == f.process.stdout
    with pytest.raises(caller.CustodyError):
        await operation(f)
    assert f.attempt.read_bytes() == (json.dumps(entered_record, ensure_ascii=True, allow_nan=False,
        separators=(",", ":")) + "\n").encode()


@pytest.mark.parametrize("phase", ["input", "claim", "post", "completion"])
@pytest.mark.asyncio
async def test_currentness_changed_during_worker_cannot_return_current_result(tmp_path, monkeypatch, phase):
    f = Fixture(tmp_path)
    loop = asyncio.get_running_loop()
    changed = threading.Event()

    def change_on_loop():
        f.current = False
        changed.set()

    def held_io():
        loop.call_soon_threadsafe(change_on_loop)
        assert changed.wait(1)

    block_phase(monkeypatch, f, phase, held_io)
    if phase == "input":
        with pytest.raises(caller.CustodyError):
            await operation(f)
        assert not f.calls and not f.attempt.exists()
    else:
        result = await operation(f)
        assert result.status in {"source_changed", "outcome_unknown"}
        assert result.artifact is None and result.selection is None and result.setup_bundle is None
        assert len(f.calls) == (0 if phase == "claim" else 1)
        assert json.loads(f.attempt.read_bytes())["status"] == "entered"
        assert f.completion()["process"] is None if phase == "claim" else f.completion()["process"] is not None
