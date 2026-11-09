"""meta/init: accepted, then settled (#550, part 2), against stand-in gateways.

Each stand-in is a tiny script speaking the stdio protocol, so these tests
pin the SDK side of every outcome without a real gateway:
- a legal slow startup (a callback round trip, then a settlement later than
  the SDK's request timeout) settles ready;
- a failed settlement raises the typed error carrying ``durable_effects``;
- once the init request is written, a lost acceptance, a gateway exit, an
  init deadline or a foreign init id is ``InitOutcomeUnknownError``, never a
  refusal;
- stdout closing during init leaves no unretrieved settlement exception for
  asyncio to report, and the caller's typed error arrives unchanged;
- an older gateway's single response still connects.
The real-gateway counterparts live in ``test_init_protocol_real_gateway.py``.
"""
from __future__ import annotations

import asyncio
import functools
import json
import sys
import textwrap
import time
import weakref
from pathlib import Path

import pytest

import meerkat_meta.runtime as runtime_module
import meerkat_meta
from meerkat_meta import (
    COMPOSITION_PROVENANCE_CODE,
    CompositionProvenanceError,
    GatewayStartupCleanupError,
    InitOutcomeUnknownError,
    RpcError,
    StorageResolutionError,
    TransportReaderFailedError,
    TransportTermination,
)
from meerkat_meta._transport import PersistentTransport
from meerkat_meta.builder import Meta
from meerkat_meta.runtime import MetaRuntime, _rpc_error_from_payload

from .test_transport_robustness import (
    _assert_settlements_retired,
    _loop_exception_reports,
)

# Every stand-in shares this prelude: it reads the init request, records it,
# and gives the scenario helpers to answer.
_PRELUDE = """
import json, pathlib, sys, time
record = pathlib.Path(sys.argv[0]).with_suffix(".record")
def emit(obj):
    sys.stdout.write(json.dumps(obj) + "\\n"); sys.stdout.flush()
def log(entry):
    with record.open("a") as f:
        f.write(json.dumps(entry) + "\\n")
init = json.loads(sys.stdin.readline())
log({"init": init})
params = init.get("params", {})
init_id = params.get("init_id")
def accepted(**extra):
    emit({"jsonrpc": "2.0", "id": init["id"], "result": {
        "init_state": "accepted", "init_id": init_id,
        "provider_callback_timeout_ms": 130000,
        "stdio_shutdown_handshake": True, "stdio_shutdown_horizon_ms": 30000, **extra}})
def progress(phase, for_init=None):
    emit({"jsonrpc": "2.0", "method": "meta/init_progress",
          "params": {"init_id": for_init or init_id, "phase": phase}})
def settled(for_init=None, **params):
    emit({"jsonrpc": "2.0", "method": "meta/init_settled",
          "params": {"init_id": for_init or init_id, **params}})
def serve_shutdown():
    for raw in sys.stdin:
        message = json.loads(raw)
        log({"after_init": message})
        if message.get("method") == "meta/shutdown":
            emit({"jsonrpc": "2.0", "id": message["id"],
                  "result": {"shutdown": True, "runtime_cleanup_completed": True}})
            return
"""


def _gateway(tmp_path: Path, name: str, body: str) -> Path:
    script = tmp_path / f"{name}.py"
    script.write_text(f"#!{sys.executable}\n{_PRELUDE}\n{textwrap.dedent(body)}")
    script.chmod(0o755)
    return script


def _records(script: Path) -> list[dict]:
    path = script.with_suffix(".record")
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines()]


@pytest.fixture
def short_request_timeout(monkeypatch):
    """Scale the SDK's 60 s request timeout down to 0.5 s for these tests."""
    monkeypatch.setattr(
        runtime_module,
        "PersistentTransport",
        functools.partial(PersistentTransport, timeout=0.5),
    )


async def _startup_error_behind_failed_stop(
    runtime: MetaRuntime, exit_code: int
) -> BaseException:
    """A gateway that negotiated the shutdown handshake and then exited
    leaves the runtime's stop nobody to ask, so that stop fails: the startup
    error arrives exactly, as ``startup_error`` and ``__cause__`` of the
    cleanup error, the stop's own record stays apart, and a retry has nothing
    left to clean up. Returns the original startup error."""
    with pytest.raises(GatewayStartupCleanupError) as raised:
        await runtime.connect()
    failure = raised.value
    assert failure.__cause__ is failure.startup_error
    assert "gateway shutdown failed after bounded cleanup" in str(failure.cleanup_error)
    record = TransportTermination("exited", exit_code, None, "none", "unanswered")
    assert failure.cleanup.last_termination == record
    await failure.cleanup.shutdown()
    assert runtime.last_gateway_termination == record
    assert not runtime.is_running
    return failure.startup_error


def _runtime(script: Path, *, init_deadline: float | None = None) -> MetaRuntime:
    builder = Meta.builder().gateway(str(script))
    if init_deadline is not None:
        builder.init_deadline(init_deadline)
    return MetaRuntime(builder._config)


@pytest.mark.asyncio
async def test_slow_legal_startup_settles_ready_after_the_request_timeout(
    tmp_path, short_request_timeout
):
    script = _gateway(tmp_path, "slow_ready", """
        accepted()
        progress("storage")
        # A provider callback during startup: the SDK answers it while the
        # init is still settling.
        emit({"jsonrpc": "2.0", "id": "cb-1", "method": "callback/unknown_startup_probe", "params": {}})
        log({"callback_answer": json.loads(sys.stdin.readline())})
        progress("restore")
        time.sleep(1.5)  # three times the scaled request timeout
        settled(outcome="ready", http_base_url="http://127.0.0.1:9", stdio_shutdown_handshake=True)
        serve_shutdown()
    """)
    runtime = _runtime(script)
    try:
        await runtime.connect()
        assert runtime.is_running
        assert runtime.rust_http_base_url == "http://127.0.0.1:9"
    finally:
        await runtime.shutdown()
    records = _records(script)
    init_params = records[0]["init"]["params"]
    assert init_params["init_protocol"] == "accepted_then_settled"
    assert init_params["init_id"].startswith("init-")
    answer = next(entry["callback_answer"] for entry in records if "callback_answer" in entry)
    assert answer["id"] == "cb-1"


@pytest.mark.parametrize(
    ("code", "durable_effects", "error_type"),
    [
        (-32602, "none", RpcError),
        (-32014, "possible", StorageResolutionError),
    ],
)
@pytest.mark.asyncio
async def test_failed_settlement_raises_typed_error_with_durable_effects(
    tmp_path, code, durable_effects, error_type
):
    script = _gateway(tmp_path, f"failed_{durable_effects}", f"""
        accepted()
        progress("storage")
        settled(outcome="failed", code={code}, message="refused for the test",
                durable_effects="{durable_effects}")
    """)
    runtime = _runtime(script)
    error = await _startup_error_behind_failed_stop(runtime, 0)
    assert isinstance(error, error_type)
    assert not isinstance(error, InitOutcomeUnknownError)
    assert error.code == code
    assert error.data["durable_effects"] == durable_effects


@pytest.mark.asyncio
async def test_failed_settlement_for_a_composition_refusal_carries_its_kind_and_fields(tmp_path):
    script = _gateway(tmp_path, "composition_refused", """
        accepted()
        progress("storage")
        settled(outcome="failed", code=-32019,
                message="Runtime bootstrap failed: the candidate definition differs",
                durable_effects="none",
                data={"kind": "candidate_divergent",
                      "fields": ["profiles.lead.tools.deny"]})
    """)
    runtime = _runtime(script)
    error = await _startup_error_behind_failed_stop(runtime, 0)
    assert isinstance(error, CompositionProvenanceError)
    assert error.code == COMPOSITION_PROVENANCE_CODE == -32019
    assert error.kind == "candidate_divergent"
    assert error.fields == ["profiles.lead.tools.deny"]
    assert error.data["durable_effects"] == "none"


class TestCompositionProvenanceError:
    def test_is_a_typed_rpc_error_exported_at_top_level(self):
        assert issubclass(CompositionProvenanceError, RpcError)
        assert meerkat_meta.CompositionProvenanceError is CompositionProvenanceError
        assert meerkat_meta.COMPOSITION_PROVENANCE_CODE == -32019

    def test_an_error_response_reifies_with_the_refusal(self):
        error = _rpc_error_from_payload(
            {
                "code": -32019,
                "message": "Runtime bootstrap failed: the composition diverged",
                "data": {
                    "kind": "divergent",
                    "fields": ["profiles.default.model", "profiles.worker"],
                    "manifest": "/state/mob.sqlite3.composition.json",
                },
            },
            request_id="init:1",
            method="meta/init",
        )
        assert isinstance(error, CompositionProvenanceError)
        assert error.kind == "divergent"
        assert error.fields == ["profiles.default.model", "profiles.worker"]
        assert error.data["manifest"] == "/state/mob.sqlite3.composition.json"
        assert error.method == "meta/init"

    def test_a_refusal_without_data_still_reifies(self):
        error = _rpc_error_from_payload(
            {"code": -32019, "message": "Runtime bootstrap failed"},
            request_id="init:1",
            method="meta/init",
        )
        assert isinstance(error, CompositionProvenanceError)
        assert error.kind is None
        assert error.fields == []

    def test_other_init_failures_do_not_reify_as_composition_refusals(self):
        for code in (-32603, -32014):
            error = _rpc_error_from_payload(
                {"code": code, "message": "refused", "data": {"kind": "divergent"}},
                request_id="init:1",
                method="meta/init",
            )
            assert not isinstance(error, CompositionProvenanceError)


@pytest.mark.asyncio
async def test_lost_acceptance_is_outcome_unknown(tmp_path, short_request_timeout):
    script = _gateway(tmp_path, "silent", """
        for raw in sys.stdin:  # hold the pipe open until the SDK closes it
            pass
    """)
    runtime = _runtime(script)
    with pytest.raises(InitOutcomeUnknownError) as raised:
        await runtime.connect()
    error = raised.value
    assert error.init_id == _records(script)[0]["init"]["params"]["init_id"]
    assert error.last_phase is None
    assert "no answer" in error.reason


@pytest.mark.asyncio
async def test_gateway_exit_after_acceptance_is_outcome_unknown_not_a_refusal(tmp_path):
    script = _gateway(tmp_path, "exits", """
        accepted()
        progress("prewarm")
        sys.exit(3)
    """)
    runtime = _runtime(script)
    error = await _startup_error_behind_failed_stop(runtime, 3)
    assert isinstance(error, InitOutcomeUnknownError)
    assert not isinstance(error, RpcError)
    assert error.last_phase == "prewarm"
    assert "closed its stdout" in error.reason


class _WatchedTransport(PersistentTransport):
    """A real transport that keeps a weak reference to each init settlement
    future it opens, so a test can prove that future was collected."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.settlements: list[weakref.ref] = []
        self.settled_at_close: list[bool] = []

    def open_init_watch(self, init_id):
        watch = super().open_init_watch(init_id)
        self.settlements.append(weakref.ref(watch._settlement))
        return watch

    def close_init_watch(self, watch):
        self.settled_at_close.append(watch._settlement.done())
        super().close_init_watch(watch)


class _FirstStopFailsTransport(_WatchedTransport):
    """Its first stop() stops the child, then reports a failure."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.stops = 0

    def stop(self):
        super().stop()
        self.stops += 1
        if self.stops == 1:
            raise RuntimeError("the first stop failed for the test")


class _ReaderFirstTransport(_WatchedTransport):
    """Returns the init response only after its reader stopped, so the
    reader's EOF fails the init watch before init closes it."""

    async def send_async(self, request, *, timeout=None):
        response = await super().send_async(request, timeout=timeout)
        if request.get("method") == "meta/init":
            await asyncio.to_thread(self._reader_thread.join, 5)
        return response


@pytest.fixture
def watched_transports(monkeypatch):
    """Make runtimes build the given transport class; returns its instances."""
    made: list[_WatchedTransport] = []

    def use(transport_class):
        def build(*args, **kwargs):
            transport = transport_class(*args, **kwargs)
            made.append(transport)
            return transport

        monkeypatch.setattr(runtime_module, "PersistentTransport", build)
        return made

    return use


_STOPPED_CLEANLY = TransportTermination("exited", 0, None, "none", "not_negotiated")


@pytest.mark.parametrize(
    "stop_fails", [False, True], ids=["stop_succeeds", "stop_fails"]
)
@pytest.mark.asyncio
async def test_stdout_closing_during_init_leaves_no_unretrieved_settlement(
    tmp_path, watched_transports, stop_fails
):
    """#251: the reader's EOF fails the pending init request and the init
    watch together. The caller raises the request's typed failure unchanged,
    and closing the watch retires its settlement, so asyncio never reports
    "Future exception was never retrieved" (TransportReaderFailedError)."""
    transports = watched_transports(
        _FirstStopFailsTransport if stop_fails else _WatchedTransport
    )
    script = _gateway(tmp_path, "closes_stdout", """
        import os
        os.close(1)  # stdout closes while meta/init is in flight
        for raw in sys.stdin:  # hold stdin until the SDK closes it
            pass
    """)
    with _loop_exception_reports() as reports:
        runtime = _runtime(script)
        if stop_fails:
            with pytest.raises(GatewayStartupCleanupError) as raised_cleanup:
                await runtime.connect()
            failure = raised_cleanup.value
            error = failure.startup_error
            assert failure.__cause__ is error
            assert type(failure.cleanup_error) is RuntimeError
            assert str(failure.cleanup_error) == "the first stop failed for the test"
            assert failure.cleanup.last_termination == _STOPPED_CLEANLY
            await failure.cleanup.shutdown()
            del raised_cleanup, failure
        else:
            with pytest.raises(InitOutcomeUnknownError) as raised:
                await runtime.connect()
            error = raised.value
            del raised
        assert type(error) is InitOutcomeUnknownError
        assert error.init_id == _records(script)[0]["init"]["params"]["init_id"]
        assert error.last_phase is None
        assert "closed its stdout" in error.reason
        assert type(error.__cause__) is TransportReaderFailedError
        del error
        assert runtime.last_gateway_termination == _STOPPED_CLEANLY
        assert not runtime.is_running
        (transport,) = transports
        # stop() does not join the reader thread; a reader still running its
        # EOF handling would keep the watch alive past the collection below.
        await asyncio.to_thread(transport._reader_thread.join, 5)
        await _assert_settlements_retired(transport.settlements, reports)


@pytest.mark.asyncio
async def test_fail_closed_refusal_then_stdout_close_leaves_no_unretrieved_settlement(
    tmp_path, watched_transports
):
    """A fail-closed init refusal is written just before the gateway exits.
    When the reader's EOF fails the init watch before init closes it, the
    caller still raises the typed refusal unchanged, and closing the watch
    retires the failure the reader delivered."""
    transports = watched_transports(_ReaderFirstTransport)
    script = _gateway(tmp_path, "refuses_then_closes", """
        import os
        emit({"jsonrpc": "2.0", "id": init["id"], "error": {
            "code": -32014, "message": "storage refused for the test"}})
        os.close(1)  # the fail-closed exit closes stdout right after
        for raw in sys.stdin:  # hold stdin until the SDK closes it
            pass
    """)
    with _loop_exception_reports() as reports:
        runtime = _runtime(script)
        with pytest.raises(StorageResolutionError) as raised:
            await runtime.connect()
        error = raised.value
        del raised
        assert type(error) is StorageResolutionError
        assert error.code == -32014
        assert str(error) == "storage refused for the test"
        assert error.method == "meta/init"
        assert error.__cause__ is None
        del error
        assert runtime.last_gateway_termination == _STOPPED_CLEANLY
        (transport,) = transports
        assert transport.child_ended.reason == "the gateway closed its stdout"
        assert transport.settled_at_close == [True], (
            "EOF did not reach the watch before close"
        )
        await _assert_settlements_retired(transport.settlements, reports)


@pytest.mark.asyncio
async def test_init_deadline_after_acceptance_is_outcome_unknown_and_shuts_down(tmp_path):
    script = _gateway(tmp_path, "never_settles", """
        accepted()
        progress("prewarm")
        serve_shutdown()
    """)
    runtime = _runtime(script, init_deadline=0.5)
    with pytest.raises(InitOutcomeUnknownError) as raised:
        await runtime.connect()
    assert raised.value.last_phase == "prewarm"
    assert "deadline" in raised.value.reason
    # The accepted response negotiated the handshake, so cleanup asked the
    # gateway to shut down instead of closing stdin under it.
    methods = [entry["after_init"].get("method") for entry in _records(script) if "after_init" in entry]
    assert methods == ["meta/shutdown"]


@pytest.mark.asyncio
async def test_settlement_for_another_init_is_ignored(tmp_path):
    script = _gateway(tmp_path, "foreign_then_ready", """
        accepted()
        settled(for_init="init-someone-else", outcome="failed", code=-32603,
                message="not this init", durable_effects="possible")
        progress("restore", for_init="init-someone-else")
        settled(outcome="ready", http_base_url="http://127.0.0.1:7")
        serve_shutdown()
    """)
    runtime = _runtime(script)
    try:
        await runtime.connect()
        assert runtime.rust_http_base_url == "http://127.0.0.1:7"
    finally:
        await runtime.shutdown()


@pytest.mark.asyncio
async def test_acceptance_for_a_different_init_id_is_outcome_unknown(tmp_path):
    script = _gateway(tmp_path, "wrong_id", """
        emit({"jsonrpc": "2.0", "id": init["id"], "result": {
            "init_state": "accepted", "init_id": "init-not-yours"}})
        for raw in sys.stdin:  # hold the pipe open until the SDK closes it
            pass
    """)
    runtime = _runtime(script)
    with pytest.raises(InitOutcomeUnknownError, match="different init_id"):
        await runtime.connect()


@pytest.mark.asyncio
async def test_older_gateway_single_response_still_connects(tmp_path):
    script = _gateway(tmp_path, "legacy", """
        emit({"jsonrpc": "2.0", "id": init["id"], "result": {"http_base_url": "http://127.0.0.1:5"}})
        for raw in sys.stdin:
            pass
    """)
    runtime = _runtime(script)
    try:
        await runtime.connect()
        assert runtime.is_running
        assert runtime.rust_http_base_url == "http://127.0.0.1:5"
    finally:
        await runtime.shutdown()


def test_init_deadline_must_be_positive_and_finite():
    builder = Meta.builder()
    for bad in (0, -1, float("inf"), float("nan"), True, "5"):
        with pytest.raises(ValueError):
            builder.init_deadline(bad)
    assert builder.init_deadline(None)._config.init_deadline is None
    assert builder.init_deadline(2)._config.init_deadline == 2.0


@pytest.mark.asyncio
async def test_long_prewarm_past_the_callback_bound_settles_ready(tmp_path, monkeypatch):
    """No SDK timer ends an accepted init: prewarm on a large roster is
    legitimately unbounded. The loop clock is jumped well past 130 s while
    the stand-in sits in `prewarm`; any timer the SDK armed would fire then."""
    import asyncio

    release = tmp_path / "release-prewarm"
    script = _gateway(tmp_path, "long_prewarm", f"""
        accepted()
        progress("prewarm")
        release = pathlib.Path({json.dumps(str(release))})
        while not release.exists():
            time.sleep(0.02)
        settled(outcome="ready", http_base_url="http://127.0.0.1:3")
        serve_shutdown()
    """)
    runtime = _runtime(script)
    connect = asyncio.create_task(runtime.connect())
    try:
        while runtime._transport is None or runtime._transport._init_watch is None \
                or runtime._transport._init_watch.last_phase != "prewarm":
            assert not connect.done(), connect.exception()
            await asyncio.sleep(0.01)
        loop = asyncio.get_running_loop()
        real_time = loop.time
        monkeypatch.setattr(loop, "time", lambda: real_time() + 600.0)
        for _ in range(20):
            await asyncio.sleep(0)
        assert not connect.done(), "an SDK timer ended the init during prewarm"
        monkeypatch.setattr(loop, "time", real_time)
        release.write_text("go")
        await connect
        assert runtime.rust_http_base_url == "http://127.0.0.1:3"
    finally:
        if not connect.done():
            connect.cancel()
        await runtime.shutdown()


@pytest.mark.asyncio
async def test_init_in_progress_refusal_is_typed(tmp_path):
    from meerkat_meta import INIT_IN_PROGRESS_CODE, InitInProgressError

    script = _gateway(tmp_path, "refuses_during_init", """
        emit({"jsonrpc": "2.0", "id": init["id"], "result": {"http_base_url": "http://127.0.0.1:5"}})
        request = json.loads(sys.stdin.readline())
        emit({"jsonrpc": "2.0", "id": request["id"], "error": {
            "code": -32018, "message": "meta/status refused: meta/init has not settled yet",
            "data": {"kind": "init_in_progress", "method": "meta/status"}}})
        for raw in sys.stdin:
            pass
    """)
    runtime = _runtime(script)
    try:
        await runtime.connect()
        with pytest.raises(InitInProgressError) as raised:
            await runtime._rpc("meta/status")
        assert raised.value.code == INIT_IN_PROGRESS_CODE
        assert raised.value.data["kind"] == "init_in_progress"
    finally:
        await runtime.shutdown()


class _RecordingTransport(PersistentTransport):
    """A real transport that records the request timeout each call passes."""

    instances: list["_RecordingTransport"] = []

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.timeouts: list[tuple[str, float | None]] = []
        _RecordingTransport.instances.append(self)

    async def send_async(self, request, *, timeout=None):
        self.timeouts.append((request.get("method"), timeout))
        return await super().send_async(request, timeout=timeout)


@pytest.fixture
def recording_transport(monkeypatch):
    _RecordingTransport.instances = []
    monkeypatch.setattr(runtime_module, "PersistentTransport", _RecordingTransport)
    return _RecordingTransport.instances


def test_gateway_init_timeout_must_be_positive_and_finite():
    builder = Meta.builder()
    assert builder._config.gateway_init_timeout == 60.0, "the default keeps the 60 s request timeout"
    for bad in (0, -1, -0.5, float("inf"), float("-inf"), float("nan"), True, False, None, "5"):
        with pytest.raises(ValueError):
            builder.gateway_init_timeout(bad)
    assert builder._config.gateway_init_timeout == 60.0, "a rejected value changes nothing"
    assert builder.gateway_init_timeout(120)._config.gateway_init_timeout == 120.0
    assert builder.gateway_init_timeout(2.5)._config.gateway_init_timeout == 2.5


_LEGACY_SERVING = """
    emit({"jsonrpc": "2.0", "id": init["id"], "result": {"http_base_url": "http://127.0.0.1:5"}})
    for raw in sys.stdin:
        message = json.loads(raw)
        if "id" in message and message.get("method"):
            emit({"jsonrpc": "2.0", "id": message["id"], "result": {"ok": True}})
"""


@pytest.mark.asyncio
async def test_init_uses_the_default_gateway_init_timeout(tmp_path, recording_transport):
    script = _gateway(tmp_path, "legacy_default", _LEGACY_SERVING)
    runtime = _runtime(script)
    try:
        await runtime.connect()
        (transport,) = recording_transport
        assert ("meta/init", 60.0) in transport.timeouts
    finally:
        await runtime.shutdown()


@pytest.mark.asyncio
async def test_gateway_init_timeout_reaches_only_the_init_request(tmp_path, recording_transport):
    script = _gateway(tmp_path, "legacy_configured", _LEGACY_SERVING)
    runtime = MetaRuntime(Meta.builder().gateway(str(script)).gateway_init_timeout(7.5)._config)
    try:
        await runtime.connect()
        await runtime._rpc("meta/status")
        (transport,) = recording_transport
        assert ("meta/init", 7.5) in transport.timeouts
        # An ordinary call keeps the transport's own request timeout.
        assert ("meta/status", None) in transport.timeouts
    finally:
        await runtime.shutdown()


@pytest.mark.asyncio
async def test_a_short_gateway_init_timeout_ends_init_and_still_cleans_up(tmp_path, recording_transport):
    script = _gateway(tmp_path, "silent_short", """
        for raw in sys.stdin:  # hold the pipe open until the SDK closes it
            pass
    """)
    runtime = MetaRuntime(Meta.builder().gateway(str(script)).gateway_init_timeout(0.5)._config)
    started = time.monotonic()
    with pytest.raises(InitOutcomeUnknownError) as raised:
        await runtime.connect()
    assert time.monotonic() - started < 30, "the configured timeout ends the wait, not the 60 s default"
    assert "no answer" in raised.value.reason
    assert not runtime.is_running
    (transport,) = recording_transport
    assert ("meta/init", 0.5) in transport.timeouts
    # The existing failed-bootstrap cleanup still stopped the gateway.
    assert not transport.is_running()
