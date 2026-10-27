"""Actual private-pipe and SDK owner controls; no native held/Ready proof."""
from concurrent.futures import ThreadPoolExecutor
import asyncio
import dataclasses
import os
from types import SimpleNamespace

import pytest

from meerkat_toolkit.host_control import (
    ApplicationPreparation, HostBootstrapSelection, HostControlError,
    SelectedHostArtifact, ToolkitMobKitRuntime,
)
from meerkat_toolkit.application_runtime import ToolkitApplicationRuntime
from test_host_control import pipes, receive, respond


def selection():
    return HostBootstrapSelection("/selected/application",
        SelectedHostArtifact("/selected/activation.json", "a" * 64),
        SelectedHostArtifact("/selected/manifest.json", "b" * 64),
        SelectedHostArtifact("/selected/policy.json", "c" * 64))


def begin(client):
    return client._begin_application_preparation(ApplicationPreparation(
        SelectedHostArtifact("/selected/plan.json", "d" * 64), timeout=2))


def test_prepare_write_returns_before_result_and_adopts_once_on_same_pipe():
    client, reader, writer = pipes()
    try:
        attempt = begin(client)
        first = receive(reader)
        assert first == {"version": 1, "id": 1, "command": {"kind": "prepare_application",
            "plan": {"path": "/selected/plan.json", "sha256": "d" * 64}}}
        assert attempt.manifest is None
        manifest = {"path": "/selected/completed/manifest.json", "sha256": "e" * 64}
        respond(writer, first, {"kind": "application_prepared", "manifest": manifest})
        assert dataclasses.asdict(attempt.observe()) == manifest
        assert dataclasses.asdict(attempt.observe()) == manifest
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(attempt.adopt, selection())
            second = receive(reader)
            assert second == {"version": 1, "id": 2, "command": {
                "kind": "bootstrap", "selection": dataclasses.asdict(selection())}}
            assert not pending.done()
            respond(writer, second, {"kind": "bootstrap_accepted"})
            pending.result(timeout=2)
        attempt.adopt(selection())
        with pytest.raises(HostControlError, match="another final selection"):
            attempt.adopt(dataclasses.replace(selection(), deployment_root="/other"))
    finally:
        client.close()
        assert reader.read() == b"", "no duplicate preparation/adoption or automatic retry"
        reader.close()
        writer.close()


@pytest.mark.parametrize("reply", [
    {"kind": "bootstrap_accepted"},
    {"kind": "application_prepared", "manifest": {"path": "relative", "sha256": "a" * 64}},
    {"kind": "application_prepared", "manifest": {"path": "/ok", "sha256": "invalid"}},
    {"kind": "application_prepared", "manifest": {"path": "/ok", "sha256": "a" * 64}, "ready": True},
])
def test_wrong_phase_or_manifest_cannot_adopt(reply):
    client, reader, writer = pipes()
    try:
        attempt = begin(client)
        first = receive(reader)
        respond(writer, first, reply)
        with pytest.raises(HostControlError):
            attempt.observe()
        with pytest.raises(HostControlError):
            attempt.adopt(selection())
    finally:
        client.close()
        assert reader.read() == b""
        reader.close()
        writer.close()


def test_published_selection_survives_lost_adoption_reply_without_resend():
    client, reader, writer = pipes()
    try:
        attempt = begin(client)
        first = receive(reader)
        respond(writer, first, {"kind": "application_prepared", "manifest": {
            "path": "/selected/complete.json", "sha256": "e" * 64}})
        attempt.observe()
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(attempt.adopt, selection())
            assert receive(reader)["id"] == 2
            writer.close()
            with pytest.raises(HostControlError):
                pending.result(timeout=2)
        assert attempt.selection == selection()
        with pytest.raises(HostControlError):
            attempt.adopt(selection())
        assert attempt.selection == selection()
    finally:
        client.close()
        assert reader.read() == b""
        reader.close()
        writer.close()


def test_fifo_plan_refuses_before_native_runtime_or_finalizer_entry(tmp_path, monkeypatch):
    path = tmp_path / "plan.fifo"
    os.mkfifo(path)
    preparation = ApplicationPreparation(SelectedHostArtifact(str(path), "a" * 64), timeout=0.1)
    entered = []

    def parent_init(*_args, **_kwargs):
        entered.append("native_runtime")
        raise AssertionError("invalid plan reached native runtime construction")

    async def finalizer(_manifest):
        entered.append("finalizer")
        raise AssertionError("invalid plan reached application finalization")

    monkeypatch.setattr(ToolkitMobKitRuntime, "__init__", parent_init)
    executor = ThreadPoolExecutor(max_workers=1)
    pending = executor.submit(ToolkitApplicationRuntime,
        SimpleNamespace(gateway_bin="/not-started"), preparation, finalizer)
    try:
        with pytest.raises(HostControlError, match="plan must be a regular file"):
            pending.result(timeout=2)
    finally:
        # Against the old blocking open, release this exact FIFO before joining
        # the worker. A failed timeout must not strand an executor or read.
        rescue = None
        try:
            if not pending.done():
                rescue = os.open(path, os.O_RDWR | os.O_NONBLOCK)
        finally:
            try:
                executor.shutdown(wait=True)
            finally:
                if rescue is not None:
                    os.close(rescue)
        assert pending.done()
        assert not entered


def test_native_ready_is_not_replaced_by_private_adoption(monkeypatch):
    async def scenario():
        native_ready, adopted = asyncio.Event(), asyncio.Event()
        native_value = object()
        async def native(_self, _transport):
            await native_ready.wait()
            return native_value
        monkeypatch.setattr(ToolkitMobKitRuntime, "_run_init", native)
        attempt = SimpleNamespace(settlement=None)
        runtime = object.__new__(ToolkitApplicationRuntime)
        async def finalize(actual):
            assert actual is attempt
            adopted.set()
        runtime._publish_and_adopt = finalize
        runtime._application_closing = False
        transport = SimpleNamespace(preparation=attempt, host_control=None)
        pending = asyncio.create_task(runtime._run_init(transport))
        await adopted.wait()
        await asyncio.sleep(0)
        assert not pending.done()
        assert attempt.settlement is None
        native_ready.set()
        assert await pending is native_value
        assert attempt.settlement.native is native_value
        assert attempt.settlement.finalization is None
    asyncio.run(scenario())


def test_cancel_joins_both_entered_operations_and_retains_all_failures(monkeypatch):
    async def scenario():
        entered, native_entered, released = asyncio.Event(), asyncio.Event(), asyncio.Event()
        native_error = RuntimeError("native cleanup failed")
        final_error = RuntimeError("published but adoption reply lost")
        close_error = RuntimeError("private close failed")
        async def native(_self, _transport):
            native_entered.set()
            await released.wait()
            raise native_error
        monkeypatch.setattr(ToolkitMobKitRuntime, "_run_init", native)
        attempt = SimpleNamespace(settlement=None, selection=None)
        runtime = object.__new__(ToolkitApplicationRuntime)
        async def finalize(actual):
            actual.selection = selection()
            entered.set()
            await released.wait()
            raise final_error
        async def close(_transport):
            runtime._application_closing = True
            raise close_error
        runtime._publish_and_adopt = finalize
        runtime._close_preparation = close
        runtime._application_closing = False
        pending = asyncio.create_task(runtime._run_init(SimpleNamespace(preparation=attempt)))
        await entered.wait()
        await native_entered.wait()
        pending.cancel()
        await asyncio.sleep(0)
        assert not pending.done(), "entered native and application owners remain joined"
        released.set()
        with pytest.raises(asyncio.CancelledError):
            await pending
        assert attempt.selection == selection(), "published desired state is not rolled back"
        assert attempt.settlement.native is native_error
        assert attempt.settlement.finalization is final_error
        assert attempt.settlement.private_close is close_error
    asyncio.run(scenario())


@pytest.mark.parametrize("cancel", [True, False])
def test_observed_loss_refuses_ready_manifest_before_finalizer_entry(monkeypatch, cancel):
    # Acknowledgements order the ready manifest behind the already-observed
    # failure boundary. Neither an event-loop sleep nor finalizer replacement
    # stands in for the production closing check in _publish_and_adopt.
    async def scenario():
        observed, native_entered, fail_native = asyncio.Event(), asyncio.Event(), asyncio.Event()
        closing_entered, release_manifest = asyncio.Event(), asyncio.Event()
        loop = asyncio.get_running_loop()
        native_error = RuntimeError("native initialization failed")
        def observe():
            loop.call_soon_threadsafe(observed.set)
            asyncio.run_coroutine_threadsafe(release_manifest.wait(), loop).result(timeout=2)
            return SelectedHostArtifact("/selected/complete.json", "e" * 64)
        async def native(_self, _transport):
            native_entered.set()
            await fail_native.wait()
            raise native_error
        monkeypatch.setattr(ToolkitMobKitRuntime, "_run_init", native)
        runtime = object.__new__(ToolkitApplicationRuntime)
        runtime._application_closing = False
        entered = []
        async def finalizer(manifest):
            entered.append(manifest)
            raise AssertionError("publication entered after observed cancellation/failure")
        runtime._application_finalizer = finalizer
        async def close(_transport):
            assert runtime._application_closing, "closing is published before scheduling cleanup"
            closing_entered.set()
            release_manifest.set()
            fail_native.set()
        runtime._close_preparation = close
        attempt = SimpleNamespace(settlement=None, observe=observe)
        pending = asyncio.create_task(runtime._run_init(SimpleNamespace(preparation=attempt)))
        await observed.wait()
        await native_entered.wait()
        if cancel:
            pending.cancel()
        else:
            fail_native.set()
        await closing_entered.wait()
        with pytest.raises(asyncio.CancelledError if cancel else RuntimeError):
            await pending
        assert not entered
        assert attempt.settlement.native is native_error
        assert isinstance(attempt.settlement.finalization, HostControlError)
        assert attempt.settlement.private_close is None
    asyncio.run(scenario())
