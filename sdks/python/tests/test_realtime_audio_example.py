from __future__ import annotations

import asyncio
import importlib.util
import sys
from pathlib import Path
from types import ModuleType
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


def test_text_probe_accepts_text_only_realtime_capabilities(realtime_example: ModuleType) -> None:
    text_only = OpenInfo({"input_kinds": ["text"], "output_kinds": ["text"]})

    realtime_example.require_text_capabilities(text_only)

    with pytest.raises(RuntimeError, match="audio input is unavailable"):
        realtime_example.require_audio_capabilities(text_only)


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
async def test_background_task_failure_is_surfaced(realtime_example: ModuleType) -> None:
    stop_event = asyncio.Event()

    async def fail_device() -> None:
        raise RuntimeError("device missing")

    failing = asyncio.create_task(fail_device(), name="microphone")

    with pytest.raises(RuntimeError, match="microphone failed: device missing"):
        await realtime_example.wait_for_stop_or_task_failure(stop_event, [failing])
