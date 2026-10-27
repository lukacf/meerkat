import asyncio
import json
import stat
from types import SimpleNamespace

import pytest
from meerkat_mobkit.models import SessionBuildOptions

from launchkit.implementation import read_receipt
from launchkit.implementation_timing import ImplementationTiming, TimedDraft, timing_projection
from launchkit.implementation_worker import native_turn


class Clock:
    value = 1_000_000_000

    def __call__(self):
        return self.value

    def advance(self, milliseconds):
        self.value += milliseconds * 1_000_000


def test_monotonic_measurement_survives_worker_handoff_and_records_overlapping_work(tmp_path):
    clock = Clock()
    preparation = ImplementationTiming(tmp_path, started_ns=clock(), clock=clock)
    clock.advance(125)
    preparation.preparation()
    worker = ImplementationTiming(tmp_path, clock=clock)
    with worker.measure("builder_native", identity="implementation:main"):
        worker.admitted("implementation:main", "native-build-one")
        with worker.measure("tool", identity="implementation:main", tool="write_project_file"):
            clock.advance(30)
        clock.advance(200)
    worker.finish("completed")
    metrics = timing_projection(tmp_path, terminal=True)
    assert metrics["finished"] and not metrics["partial"]
    assert metrics["elapsed_ms"] == 355
    phases = {row["phase"]: row for row in metrics["phases"]}
    assert phases["preparation"]["duration_ms"] == 125
    assert phases["tool"]["duration_ms"] == 30
    assert phases["builder_native"]["duration_ms"] == 230
    assert sum(row["duration_ms"] for row in metrics["phases"]) > metrics["elapsed_ms"]
    assert metrics["overlap"]
    stored = json.loads(worker.path.read_text())
    tool = next(row for row in stored["spans"] if row.get("tool"))
    assert tool["interaction_id"] == "native-build-one"
    assert "origin_ns" not in metrics and "spans" not in metrics
    assert stat.S_IMODE(worker.path.stat().st_mode) == 0o600


@pytest.mark.asyncio
@pytest.mark.parametrize("tool,identity,phase", [
    ("publish_acceptance", "acceptance:main", "reviewer_native"),
    ("edit_project_file", "implementation:main", "builder_native"),
])
async def test_actual_sdk_timed_callbacks_preserve_schemas_results_parallel_work_and_errors(tmp_path, tool, identity, phase):
    clock = Clock()
    timing = ImplementationTiming(tmp_path, clock=clock)
    draft = SessionBuildOptions()
    timed = TimedDraft(draft, timing, identity)
    entered, release = asyncio.Event(), asyncio.Event()
    args = {"private": "never include this in timing"}
    result = {"ok": False, "evidence": "preserve the actual callback rejection"}

    async def callback(received):
        assert received is args
        entered.set()
        await release.wait()
        return result

    schema = {"type": "object", "properties": {"private": {"type": "string"}}, "required": ["private"]}
    timed.register_tool(tool, callback, description="Exact registered description", input_schema=schema)
    wire = draft.to_dict()["tools"][0]
    assert wire["input_schema"] == schema
    with timing.measure(phase, identity=identity):
        timing.admitted(identity, "native-operation-one")
        first = asyncio.create_task(draft.tool_handlers[tool](args))
        await entered.wait()
        second = asyncio.create_task(draft.tool_handlers[tool](args))
        await asyncio.sleep(0)
        clock.advance(50)
        release.set()
        assert await first is result and await second is result
    stored = timing.path.read_text()
    assert args["private"] not in stored and result["evidence"] not in stored
    metrics = timing_projection(tmp_path)
    tools = next(row for row in metrics["phases"] if row["phase"] == "tool")
    assert tools["count"] == 2 and tools["duration_ms"] == 100

    failure = ValueError("private exception that must remain absent from metrics")
    async def fails(received):
        raise failure
    timed.register_tool("check_acceptance", fails, description="Check", input_schema={"type": "object"})
    with pytest.raises(ValueError) as caught:
        await draft.tool_handlers["check_acceptance"]({})
    assert caught.value is failure
    last = json.loads(timing.path.read_text())["spans"][-1]
    assert last["status"] == "failed" and str(failure) not in timing.path.read_text()


@pytest.mark.asyncio
async def test_native_admission_and_typed_failure_keep_exact_receipt_and_measure_interruption(tmp_path, monkeypatch):
    from launchkit import implementation_worker as worker
    from launchkit.templates.storage_setup import write_private

    write_private(tmp_path / "receipt.json", {"status": "running", "ready_to_package": False})
    clock = Clock()
    timing = ImplementationTiming(tmp_path, clock=clock)
    async def request(client, method, url, **kwargs):
        if method == "post":
            clock.advance(25)
            return SimpleNamespace(json=lambda: {"interaction_id": "native-admitted", "cursor": "40"})
        clock.advance(250)
        return SimpleNamespace(json=lambda: {"frames": [{"cursor": "41", "kind": "interaction_failed",
            "interaction_id": "native-admitted", "payload": {"error": "original native failure"}}]})
    monkeypatch.setattr(worker, "_native_request", request)
    with pytest.raises(worker.NativeInteractionFailure, match="original native failure"):
        await native_turn(None, "http://127.0.0.1:9999", tmp_path, asyncio.Event(),
            "implementation:main", "private prompt", "private-key", timing=timing)
    receipt = read_receipt(tmp_path)
    assert receipt["native_admission"]["interaction_id"] == "native-admitted"
    assert receipt["status"] == "running" and receipt["ready_to_package"] is False
    stored = json.loads(timing.path.read_text())
    assert [row["phase"] for row in stored["spans"]] == ["native_admission", "builder_native"]
    assert stored["spans"][-1]["status"] == "failed"
    assert stored["spans"][-1]["duration_ms"] == 275
    assert "private prompt" not in timing.path.read_text() and "original native failure" not in timing.path.read_text()
    assert timing_projection(tmp_path, terminal=True)["partial"]

    with pytest.raises(asyncio.CancelledError):
        with timing.measure("reviewer_native", identity="acceptance:main"):
            clock.advance(20)
            raise asyncio.CancelledError
    assert json.loads(timing.path.read_text())["spans"][-1]["status"] == "interrupted"


def test_diagnostics_write_failure_cannot_replace_callback_result(tmp_path, monkeypatch):
    timing = ImplementationTiming(tmp_path)
    def unavailable(*args):
        raise OSError("metrics disk unavailable")
    monkeypatch.setattr("launchkit.implementation_timing.write_private", unavailable)
    actual = object()
    with timing.measure("tool", identity="implementation:main", tool="read_project_file"):
        result = actual
    assert result is actual and timing.disabled


@pytest.mark.asyncio
@pytest.mark.parametrize("total", [{}, {"count": "one", "duration_ms": 12},
    {"count": 1, "duration_ms": float("inf")}, {"count": 1, "duration_ms": -3}])
async def test_malformed_restored_totals_cannot_replace_actual_callback_result(tmp_path, total):
    initial = ImplementationTiming(tmp_path)
    initial.preparation()
    saved = json.loads(initial.path.read_text())
    saved["totals"]["tool"] = total
    initial.path.write_text(json.dumps(saved))
    restored = ImplementationTiming(tmp_path)
    draft = SessionBuildOptions()
    actual = {"ok": False, "original": "A truthful callback rejection"}
    async def callback(args):
        return actual
    TimedDraft(draft, restored, "implementation:main").register_tool("publish_implementation", callback,
        description="Publish", input_schema={"type": "object"})
    assert await draft.tool_handlers["publish_implementation"]({}) is actual
    restored.finish("completed")
    assert timing_projection(tmp_path, terminal=True)["partial"]


@pytest.mark.parametrize("value", [[], {"version": 1, "build_id": "another-build"},
    {"version": 1, "elapsed_ms": float("nan")}, {"version": 1, "elapsed_ms": True}])
def test_legacy_invalid_or_wrong_build_metrics_are_not_invented(tmp_path, value):
    assert timing_projection(tmp_path) is None
    (tmp_path / "timing.json").write_text(json.dumps(value))
    assert timing_projection(tmp_path) is None
