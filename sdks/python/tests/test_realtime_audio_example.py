from __future__ import annotations

import asyncio
import importlib.util
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace
from typing import Any

import pytest


@pytest.fixture(scope="module")
def realtime_example() -> ModuleType:
    repo_root = Path(__file__).resolve().parents[3]
    example_path = repo_root / "examples" / "036-realtime-audio-py" / "main.py"
    spec = importlib.util.spec_from_file_location("example_036_realtime_audio_py", example_path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class OpenInfo:
    def __init__(self, capabilities: dict[str, Any]):
        self.capabilities = capabilities


class ProbeConnection:
    def __init__(self) -> None:
        self.frames: list[tuple[str, dict[str, Any] | None]] = []

    async def send_input(self, frame: dict[str, Any]) -> None:
        self.frames.append(("input", frame))

    async def commit_turn(self) -> None:
        self.frames.append(("commit", None))


class FrameConnection:
    def __init__(self, frames: list[dict[str, Any] | None]):
        self.frames = list(frames)

    async def recv(self) -> dict[str, Any] | None:
        if not self.frames:
            await asyncio.sleep(10)
        return self.frames.pop(0)


def test_text_probe_accepts_text_only_realtime_capabilities(realtime_example: ModuleType) -> None:
    text_only = OpenInfo({"input_kinds": ["text"], "output_kinds": ["text"]})

    realtime_example.require_text_capabilities(text_only)

    with pytest.raises(RuntimeError, match="audio input is unavailable"):
        realtime_example.require_audio_capabilities(text_only)


def test_text_probe_channel_uses_explicit_commit(realtime_example: ModuleType) -> None:
    text_probe_args = SimpleNamespace(text_probe=True)
    audio_args = SimpleNamespace(text_probe=False)

    text_probe_channel = realtime_example.build_realtime_channel(object(), "mob-1", text_probe_args)
    audio_channel = realtime_example.build_realtime_channel(object(), "mob-1", audio_args)

    assert text_probe_channel.turning_mode == "explicit_commit"
    assert audio_channel.turning_mode == "provider_managed"


@pytest.mark.asyncio
async def test_text_probe_waits_for_turn_completion(realtime_example: ModuleType) -> None:
    stop_event = asyncio.Event()
    signals = realtime_example.TextProbeSignals()
    connection = ProbeConnection()
    printer = realtime_example.TranscriptPrinter()

    task = asyncio.create_task(
        realtime_example.run_text_probe(connection, printer, stop_event, signals, timeout_s=2.0)
    )
    await asyncio.sleep(0)
    signals.turn_completed.set()

    await task

    assert stop_event.is_set()
    assert [kind for kind, _frame in connection.frames] == ["input", "commit"]


@pytest.mark.asyncio
async def test_text_probe_rejects_close_before_completion(realtime_example: ModuleType) -> None:
    stop_event = asyncio.Event()
    signals = realtime_example.TextProbeSignals()
    connection = ProbeConnection()
    printer = realtime_example.TranscriptPrinter()

    task = asyncio.create_task(
        realtime_example.run_text_probe(connection, printer, stop_event, signals, timeout_s=2.0)
    )
    await asyncio.sleep(0)
    signals.channel_closed.set()

    with pytest.raises(RuntimeError, match="closed before a tool or turn completion"):
        await task


@pytest.mark.asyncio
async def test_realtime_receiver_surfaces_channel_error(realtime_example: ModuleType) -> None:
    stop_event = asyncio.Event()
    output_queue: asyncio.Queue[bytes | None] = asyncio.Queue()
    connection = FrameConnection(
        [{"type": "channel.error", "code": "authentication_failed", "message": "bad key"}]
    )

    with pytest.raises(RuntimeError, match="authentication_failed: bad key"):
        await realtime_example.realtime_receiver(
            connection,
            output_queue,
            realtime_example.TranscriptPrinter(),
            stop_event,
        )

    assert stop_event.is_set()
    assert await output_queue.get() is None


@pytest.mark.asyncio
async def test_background_task_failure_is_surfaced(realtime_example: ModuleType) -> None:
    stop_event = asyncio.Event()

    async def fail_device() -> None:
        raise RuntimeError("device missing")

    failing = asyncio.create_task(fail_device(), name="microphone")

    with pytest.raises(RuntimeError, match="microphone failed: device missing"):
        await realtime_example.wait_for_stop_or_task_failure(stop_event, [failing])


@pytest.mark.asyncio
async def test_background_task_failure_wins_stop_race(realtime_example: ModuleType) -> None:
    stop_event = asyncio.Event()
    stop_event.set()

    async def fail_probe() -> None:
        raise RuntimeError("closed before completion")

    failing = asyncio.create_task(fail_probe(), name="text probe")
    await asyncio.sleep(0)

    with pytest.raises(RuntimeError, match="text probe failed: closed before completion"):
        await realtime_example.wait_for_stop_or_task_failure(stop_event, [failing])


@pytest.mark.asyncio
async def test_background_task_failure_after_stop_is_surfaced(realtime_example: ModuleType) -> None:
    stop_event = asyncio.Event()

    async def close_channel() -> None:
        stop_event.set()

    async def fail_after_close() -> None:
        await stop_event.wait()
        raise RuntimeError("closed before completion")

    closer = asyncio.create_task(close_channel(), name="realtime receiver")
    failing = asyncio.create_task(fail_after_close(), name="text probe")

    with pytest.raises(RuntimeError, match="text probe failed: closed before completion"):
        await realtime_example.wait_for_stop_or_task_failure(stop_event, [closer, failing])
