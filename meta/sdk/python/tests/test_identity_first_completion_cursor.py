"""Turn-completion contract for the Python SDK.

The defect these tests pin: `wait_for_output(baseline=<text>)` waits for the
output text to CHANGE. Two consecutive turns that both answer exactly `ACK`
are indistinguishable from no turn at all, so the call sleeps out its entire
timeout — a 900s wait reported as a "962-second turn" that never happened.

The replacement waits on a cursor: `{epoch, turns}`, where `epoch` is the
identity's lease incarnation and `turns` counts completed turns within it.
"""
import asyncio
import warnings

import pytest

from meerkat_meta.errors import CompletionCursorUnavailableError
from meerkat_meta.identity_first_models import (
    CompletionCursor,
    CompletionProgress,
    DispatchInput,
    DispatchResult,
    IdentityInspection,
    SendResult,
)
from meerkat_meta.runtime import IdentityAgentHandle, MetaRuntime

# These scripts model a gateway that returns no turn ticket, so the
# ``*_and_wait`` calls here run the explicit identity-wide fallback (the
# ticketed path is covered in test_identity_first_turn_tickets.py).
pytestmark = pytest.mark.filterwarnings(
    "ignore::meerkat_meta.errors.TurnTrackingUnavailableWarning"
)


class ScriptedTransport:
    """Transport that answers each RPC method from a scripted queue.

    The script is one list of inspection payloads, each a later moment,
    holding the last entry once exhausted, so a test can model "still
    running, still running, done" without racing a clock.

    ``meta/wait_for_completion`` models the gateway's server-side wait: it
    walks the script until an entry satisfies the wait (see
    ``_serve_completion_wait``). After a wait that resolved,
    ``meta/inspect_identity`` answers the entry the wait resolved on (the
    output a waiter reads once, at completion); otherwise it walks the script
    itself, as does ``meta/completion_cursor``. ``legacy_gateway=True``
    models a gateway predating the server-side wait and the cursor read: both
    are "method not found".
    """

    def __init__(self, *, send=None, dispatch=None, inspections=None, legacy_gateway=False):
        self.calls: list[dict] = []
        self.request_timeout = 60.0
        self._send = send or {}
        self._dispatch = dispatch or {}
        self._inspections = list(inspections or [])
        self._index = 0
        self._pinned = False
        self._legacy_gateway = legacy_gateway

    def _walk(self) -> dict:
        index = min(self._index, len(self._inspections) - 1)
        self._index += 1
        return self._inspections[index]

    def _at_end(self) -> bool:
        return self._index >= len(self._inspections)

    def send_sync(self, request):
        self.calls.append(request)
        method = request.get("method")
        if method == "meta/send":
            result = self._send
        elif method == "meta/dispatch":
            result = self._dispatch
        elif method == "meta/completion_cursor":
            if self._legacy_gateway:
                return {
                    "jsonrpc": "2.0",
                    "id": request.get("id"),
                    "error": {"code": -32601, "message": "method not found"},
                }
            entry = self._walk()
            result = {
                "identity": entry["identity"],
                "state": "active",
                "completion_cursor": entry.get("completion_cursor"),
            }
        elif method == "meta/wait_for_completion":
            if self._legacy_gateway:
                return {
                    "jsonrpc": "2.0",
                    "id": request.get("id"),
                    "error": {"code": -32601, "message": "method not found"},
                }
            result = _serve_completion_wait(self._walk, self._at_end, request.get("params") or {})
            self._pinned = result["outcome"] in ("completed", "incarnation_changed")
        elif method == "meta/inspect_identity":
            if self._pinned:
                index = min(self._index - 1, len(self._inspections) - 1)
                result = self._inspections[index]
            else:
                result = self._walk()
        else:
            result = {}
        return {"jsonrpc": "2.0", "id": request.get("id"), "result": result}

    async def send_async(self, request, *, timeout=None):
        response = self.send_sync(request)
        result = response.get("result") or {}
        if isinstance(result, dict) and result.get("outcome") == "timed_out":
            # The gateway holds a server-side wait until its deadline.
            await asyncio.sleep((request.get("params") or {}).get("timeout_ms", 0) / 1000)
        return response

    def is_running(self):
        return True

    def start(self):
        pass

    def stop(self):
        pass

    def set_callback_handler(self, handler):
        pass

    @property
    def inspect_calls(self) -> int:
        return sum(
            1 for call in self.calls if call.get("method") == "meta/inspect_identity"
        )

    @property
    def cursor_calls(self) -> int:
        return sum(
            1 for call in self.calls if call.get("method") == "meta/completion_cursor"
        )

    @property
    def wait_calls(self) -> int:
        return sum(
            1 for call in self.calls if call.get("method") == "meta/wait_for_completion"
        )


def _serve_completion_wait(walk, at_end, params: dict) -> dict:
    """Model the gateway's server-side ``meta/wait_for_completion``: walk
    the scripted cursor states (each a later moment) until one satisfies the
    wait. A script that ends unsatisfied is the typed ``timed_out``."""
    after = params.get("after")
    while True:
        entry = walk()
        cursor = entry.get("completion_cursor")
        base = {"identity": entry["identity"], "completion_cursor": cursor}
        if cursor is None:
            return {**base, "outcome": "untracked"}
        if after is not None and cursor["epoch"] != after["epoch"]:
            return {**base, "outcome": "incarnation_changed"}
        if cursor["turns"] > (after["turns"] if after is not None else 0):
            return {**base, "outcome": "completed"}
        if at_end():
            return {**base, "outcome": "timed_out"}


def _make_runtime(transport) -> MetaRuntime:
    rt = MetaRuntime.__new__(MetaRuntime)
    rt._config = None
    rt._transport = transport
    rt._running = True
    rt._rust_http_base = None
    rt._lifecycle_lock = asyncio.Lock()
    rt._shutdown_task = None
    from meerkat_meta.agent_builder import CallbackDispatcher

    rt._dispatcher = CallbackDispatcher()
    return rt


def _inspection(identity: str, preview: str | None, epoch: int, turns: int) -> dict:
    return {
        "identity": identity,
        "output_preview": preview,
        "is_final": False,
        "peer_reachable_count": 0,
        "completion_cursor": {"epoch": epoch, "turns": turns},
    }


# ---------------------------------------------------------------------------
# The production regression
# ---------------------------------------------------------------------------


class TestIdenticalConsecutiveOutput:
    @pytest.mark.asyncio
    async def test_second_identical_ack_turn_is_detected(self):
        """THE defect: two turns both answering exactly `ACK`.

        The second completion must be detected. The old text comparison could
        not see it at all.
        """
        transport = ScriptedTransport(
            send={
                "fencing_token": 7,
                "completion_baseline": {"epoch": 7, "turns": 1},
            },
            inspections=[
                # Turn 2 in flight — the PREVIOUS turn's `ACK` is still the
                # visible output.
                _inspection("triage:main", "ACK", epoch=7, turns=1),
                # Turn 2 committed. Same text, byte for byte.
                _inspection("triage:main", "ACK", epoch=7, turns=2),
            ],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        output = await handle.send_and_wait("ping", timeout=5, poll_interval=0.01)

        assert output == "ACK"
        assert transport.wait_calls == 1, "one server-side wait, no client polling"
        assert transport.cursor_calls == 0
        assert transport.inspect_calls == 1, "output is read once, at completion"

    @pytest.mark.asyncio
    async def test_text_baseline_path_cannot_see_the_identical_turn(self):
        """The deprecated path, on the same data, times out. Kept as the proof
        that the scenario really is the one that defeats a text comparison —
        if this ever passes, the cursor test above is passing for free."""
        transport = ScriptedTransport(
            inspections=[_inspection("triage:main", "ACK", epoch=7, turns=2)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeprecationWarning)
            with pytest.raises(TimeoutError):
                await handle.wait_for_output(
                    timeout=0.05, poll_interval=0.01, baseline="ACK"
                )

    @pytest.mark.asyncio
    async def test_text_baseline_path_warns_as_deprecated(self):
        transport = ScriptedTransport(
            inspections=[_inspection("triage:main", "ACK", epoch=7, turns=2)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        with pytest.warns(DeprecationWarning, match="unsound"):
            with pytest.raises(TimeoutError):
                await handle.wait_for_output(
                    timeout=0.05, poll_interval=0.01, baseline="ACK"
                )

    @pytest.mark.asyncio
    async def test_wait_for_output_after_cursor_sees_identical_turn(self):
        """The retained `wait_for_output` entry point, driven by `after`."""
        transport = ScriptedTransport(
            inspections=[
                _inspection("triage:main", "ACK", epoch=7, turns=1),
                _inspection("triage:main", "ACK", epoch=7, turns=2),
            ],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        output = await handle.wait_for_output(
            timeout=5,
            poll_interval=0.01,
            after=CompletionCursor(epoch=7, turns=1),
        )

        assert output == "ACK"
        assert transport.wait_calls == 1
        assert transport.inspect_calls == 1, "the member is read only past `after`"

    def test_after_and_baseline_are_mutually_exclusive(self):
        transport = ScriptedTransport(inspections=[_inspection("a", None, 1, 0)])
        handle = IdentityAgentHandle(_make_runtime(transport), "a")

        with pytest.raises(ValueError, match="not both"):
            asyncio.run(
                handle.wait_for_output(
                    after=CompletionCursor(epoch=1, turns=0), baseline="ACK"
                )
            )


# ---------------------------------------------------------------------------
# Waiter semantics
# ---------------------------------------------------------------------------


class TestWaitForCompletion:
    @pytest.mark.asyncio
    async def test_monotonic_cursor_across_several_turns(self):
        transport = ScriptedTransport(
            inspections=[
                _inspection("triage:main", "ACK", epoch=3, turns=turns)
                for turns in (0, 1, 2, 3, 4)
            ],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        seen = []
        for _ in range(5):
            inspection = await handle.inspect()
            seen.append(inspection.completion_cursor.turns)

        assert seen == [0, 1, 2, 3, 4]
        assert all(b > a for a, b in zip(seen, seen[1:])), "strictly increasing"

    @pytest.mark.asyncio
    async def test_stalled_turn_times_out(self):
        transport = ScriptedTransport(
            inspections=[_inspection("triage:main", "ACK", epoch=3, turns=1)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        with pytest.raises(TimeoutError, match="did not complete a turn"):
            await handle.wait_for_completion(
                CompletionCursor(epoch=3, turns=1), timeout=0.05, poll_interval=0.01
            )

    @pytest.mark.asyncio
    async def test_incarnation_change_is_reported_not_guessed(self):
        transport = ScriptedTransport(
            inspections=[_inspection("triage:main", "ACK", epoch=9, turns=0)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        with pytest.raises(RuntimeError, match="superseded runtime incarnation"):
            await handle.wait_for_completion(
                CompletionCursor(epoch=3, turns=1), timeout=5, poll_interval=0.01
            )

    @pytest.mark.asyncio
    async def test_missing_cursor_raises_instead_of_falling_back_to_text(self):
        """An older gateway must fail loudly, not silently reintroduce the bug."""
        transport = ScriptedTransport(
            send={"fencing_token": 7},
            inspections=[
                {
                    "identity": "triage:main",
                    "output_preview": "ACK",
                    "is_final": False,
                    "peer_reachable_count": 0,
                }
            ],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        with pytest.raises(RuntimeError, match="no completion_baseline"):
            await handle.send_and_wait("ping", timeout=1, poll_interval=0.01)

    @pytest.mark.asyncio
    async def test_dispatch_and_wait_threads_its_own_baseline(self):
        transport = ScriptedTransport(
            dispatch={
                "fencing_token": 4,
                "durable": True,
                "completion_baseline": {"epoch": 4, "turns": 5},
            },
            inspections=[
                _inspection("internal:main", "ACK", epoch=4, turns=5),
                _inspection("internal:main", "ACK", epoch=4, turns=6),
            ],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "internal:main")

        output = await handle.dispatch_and_wait(
            DispatchInput(content="go", origin="system"), timeout=5, poll_interval=0.01
        )

        assert output == "ACK"
        assert transport.wait_calls == 1
        assert transport.inspect_calls == 1


# ---------------------------------------------------------------------------
# Server-side, event-driven waits (#468)
# ---------------------------------------------------------------------------


class TestServerSideWaits:
    """``inspect_identity`` reads the member session (an execution snapshot
    on its session task, which can hold a staged run on meerkat 0.8.45+). The
    completion waits are one server-side ``meta/wait_for_completion`` each,
    which the gateway answers on the typed completion signal, and read the
    member once, at completion. Nothing polls."""

    @pytest.mark.asyncio
    async def test_wait_for_completion_is_one_server_wait_then_one_member_read(self):
        transport = ScriptedTransport(
            inspections=[
                _inspection("triage:main", "old", epoch=3, turns=1),
                _inspection("triage:main", "old", epoch=3, turns=1),
                _inspection("triage:main", "new", epoch=3, turns=2),
            ],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        output = await handle.wait_for_completion(
            CompletionCursor(epoch=3, turns=1), timeout=7, poll_interval=0.01
        )

        assert output == "new"
        assert [call["method"] for call in transport.calls] == [
            "meta/wait_for_completion",
            "meta/inspect_identity",
        ]
        wait = transport.calls[0]["params"]
        assert wait == {
            "identity": "triage:main",
            "after": {"epoch": 3, "turns": 1},
            "timeout_ms": wait["timeout_ms"],
        }
        assert 6000 < wait["timeout_ms"] <= 7000, "the caller's deadline rides along"

    @pytest.mark.asyncio
    async def test_a_server_timeout_is_a_timeout_error(self):
        transport = ScriptedTransport(
            inspections=[_inspection("triage:main", "ACK", epoch=3, turns=1)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        with pytest.raises(TimeoutError, match="did not complete a turn"):
            await handle.wait_for_completion(
                CompletionCursor(epoch=3, turns=1), timeout=0.05
            )
        assert transport.inspect_calls == 0

    @pytest.mark.asyncio
    async def test_wait_until_ready_is_one_server_wait_per_identity(self):
        transport = ScriptedTransport(
            inspections=[
                _inspection("triage:main", None, epoch=3, turns=0),
                _inspection("triage:main", None, epoch=3, turns=1),
            ],
        )
        rt = _make_runtime(transport)

        await rt.wait_until_ready(["triage:main"], timeout=5)

        assert transport.wait_calls == 1
        assert "after" not in transport.calls[0]["params"], "readiness has no baseline"
        assert transport.inspect_calls == 0, (
            "readiness is the cursor: an agent whose kickoff committed no text "
            "is ready without any member read"
        )

    @pytest.mark.asyncio
    async def test_wait_until_ready_names_the_identities_not_ready(self):
        transport = ScriptedTransport(
            inspections=[_inspection("x", None, epoch=3, turns=0)],
        )
        rt = _make_runtime(transport)

        with pytest.raises(TimeoutError, match=r"\['a:1', 'b:1'\]"):
            await rt.wait_until_ready(["b:1", "a:1"], timeout=0.05)

    @pytest.mark.asyncio
    async def test_wait_until_ready_falls_back_to_the_preview_for_a_live_alias(self):
        """A live alias (no identity authority) is untracked; only then does
        readiness fall back to the committed-output proxy."""
        transport = ScriptedTransport(
            inspections=[
                {"identity": "live:alias", "output_preview": None, "completion_cursor": None},
                {"identity": "live:alias", "output_preview": "hi", "completion_cursor": None},
            ],
        )
        rt = _make_runtime(transport)

        await rt.wait_until_ready(["live:alias"], timeout=5, poll_interval=0.01)

        assert transport.wait_calls == 1
        assert transport.inspect_calls == 1

    @pytest.mark.asyncio
    async def test_completion_cursor_reads_null_as_untracked(self):
        transport = ScriptedTransport(
            inspections=[{"identity": "live:alias", "completion_cursor": None}],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "live:alias")

        assert await handle.completion_cursor() is None
        assert transport.calls[-1]["params"] == {"identity": "live:alias"}

    @pytest.mark.asyncio
    async def test_a_gateway_without_server_waits_falls_back_to_polling(self):
        transport = ScriptedTransport(
            legacy_gateway=True,
            inspections=[
                _inspection("triage:main", "ACK", epoch=7, turns=1),
                _inspection("triage:main", "ACK", epoch=7, turns=2),
            ],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        output = await handle.wait_for_completion(
            CompletionCursor(epoch=7, turns=1), timeout=5, poll_interval=0.01
        )

        assert output == "ACK"
        assert await handle.completion_cursor() == CompletionCursor(epoch=7, turns=2)

    @pytest.mark.asyncio
    async def test_other_rpc_errors_are_not_mistaken_for_an_old_gateway(self):
        from meerkat_meta.errors import RpcError

        class FailingTransport(ScriptedTransport):
            def send_sync(self, request):
                if request.get("method") in (
                    "meta/completion_cursor",
                    "meta/wait_for_completion",
                ):
                    self.calls.append(request)
                    return {
                        "jsonrpc": "2.0",
                        "id": request.get("id"),
                        "error": {"code": -32001, "message": "unknown identity: nobody"},
                    }
                return super().send_sync(request)

        transport = FailingTransport(inspections=[_inspection("nobody", None, 1, 0)])
        handle = IdentityAgentHandle(_make_runtime(transport), "nobody")

        with pytest.raises(RpcError) as raised:
            await handle.completion_cursor()
        assert raised.value.code == -32001
        with pytest.raises(RpcError) as raised:
            await handle.wait_for_completion(CompletionCursor(epoch=1, turns=0), timeout=5)
        assert raised.value.code == -32001
        with pytest.raises(RpcError):
            await handle._runtime.wait_until_ready(["nobody"], timeout=5)
        assert transport.inspect_calls == 0


class TestTypedWaitOutcomes:
    """The server ends a wait typed instead of running it to the deadline."""

    def _transport(self, outcome: str, identity: str = "triage:main"):
        transport = ScriptedTransport(inspections=[_inspection(identity, None, 3, 1)])
        original = transport.send_sync

        def send_sync(request):
            if request.get("method") == "meta/wait_for_completion":
                transport.calls.append(request)
                return {
                    "jsonrpc": "2.0",
                    "id": request.get("id"),
                    "result": {
                        "identity": identity,
                        "outcome": outcome,
                        "completion_cursor": {"epoch": 3, "turns": 1},
                    },
                }
            return original(request)

        transport.send_sync = send_sync
        return transport

    @pytest.mark.asyncio
    @pytest.mark.parametrize(
        "outcome", ["run_failed", "broken", "retiring", "identity_gone", "shutting_down"]
    )
    async def test_wait_for_completion_raises_the_typed_outcome(self, outcome):
        from meerkat_meta.errors import WaitEndedError

        transport = self._transport(outcome)
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")
        with pytest.raises(WaitEndedError) as raised:
            await handle.wait_for_completion(CompletionCursor(epoch=3, turns=1), timeout=5)
        assert raised.value.outcome == outcome
        assert raised.value.identity == "triage:main"
        assert transport.inspect_calls == 0

    @pytest.mark.asyncio
    async def test_wait_until_ready_names_why_an_identity_is_not_ready(self):
        transport = self._transport("broken", identity="a:1")
        rt = _make_runtime(transport)
        with pytest.raises(TimeoutError, match=r"a:1 \(broken\)"):
            await rt.wait_until_ready(["a:1"], timeout=5)

    @pytest.mark.asyncio
    async def test_wait_for_turn_raises_on_shutdown(self):
        from meerkat_meta.errors import WaitEndedError

        transport = ScriptedTransport()
        original = transport.send_sync

        def send_sync(request):
            if request.get("method") == "meta/wait_for_turn":
                transport.calls.append(request)
                return {
                    "jsonrpc": "2.0",
                    "id": request.get("id"),
                    "result": {
                        "identity": "keeper",
                        "ticket": "t-1",
                        "state": "pending",
                        "wait": "shutting_down",
                    },
                }
            return original(request)

        transport.send_sync = send_sync
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")
        with pytest.raises(WaitEndedError) as raised:
            await handle.wait_for_turn("t-1", timeout=5)
        assert raised.value.outcome == "shutting_down"


# ---------------------------------------------------------------------------
# Cursor value semantics
# ---------------------------------------------------------------------------


class TestCompletionCursor:
    def test_progress_classification(self):
        baseline = CompletionCursor(epoch=2, turns=3)
        assert (
            baseline.progress_since(baseline) is CompletionProgress.PENDING
        ), "an unchanged cursor is Pending regardless of what the agent said"
        assert (
            CompletionCursor(epoch=2, turns=4).progress_since(baseline)
            is CompletionProgress.COMPLETED
        )
        assert (
            CompletionCursor(epoch=3, turns=0).progress_since(baseline)
            is CompletionProgress.INCARNATION_CHANGED
        )

    def test_round_trip(self):
        cursor = CompletionCursor(epoch=12, turns=34)
        assert cursor.to_dict() == {"epoch": 12, "turns": 34}
        assert CompletionCursor.from_dict(cursor.to_dict()) == cursor


# ---------------------------------------------------------------------------
# Wire mirrors: both directions, both fields
# ---------------------------------------------------------------------------


class TestModelMirrors:
    def test_identity_inspection_carries_cursor_both_ways(self):
        payload = _inspection("triage:main", "ACK", epoch=7, turns=2)

        parsed = IdentityInspection.from_dict(payload)
        assert parsed.completion_cursor == CompletionCursor(epoch=7, turns=2)
        assert parsed.to_dict()["completion_cursor"] == {"epoch": 7, "turns": 2}
        assert IdentityInspection.from_dict(parsed.to_dict()) == parsed

    def test_identity_inspection_carries_preview_unavailable_both_ways(self):
        payload = {
            "identity": "triage:main",
            "is_final": False,
            "peer_reachable_count": 0,
            "intent_managed": False,
            "preview_unavailable": "observation_deadline",
        }

        parsed = IdentityInspection.from_dict(payload)
        assert parsed.output_preview is None
        assert parsed.preview_unavailable == "observation_deadline"
        assert parsed.to_dict() == payload
        # An observed preview carries no marker, and a gateway predating the
        # field (or a null marker) reads as observed.
        observed = IdentityInspection.from_dict(
            {"identity": "triage:main", "output_preview": "ACK", "preview_unavailable": None}
        )
        assert observed.preview_unavailable is None
        assert "preview_unavailable" not in observed.to_dict()
        # Unknown future reasons pass through.
        future = IdentityInspection.from_dict(
            {"identity": "triage:main", "preview_unavailable": "some_future_reason"}
        )
        assert future.preview_unavailable == "some_future_reason"

    def test_dispatch_result_carries_baseline_both_ways(self):
        payload = {
            "fencing_token": 4,
            "durable": True,
            "completion_baseline": {"epoch": 4, "turns": 5},
        }

        parsed = DispatchResult.from_dict(payload)
        assert parsed.completion_baseline == CompletionCursor(epoch=4, turns=5)
        assert parsed.to_dict() == payload
        assert DispatchResult.from_dict(parsed.to_dict()) == parsed

    def test_send_result_carries_baseline_both_ways(self):
        payload = {"fencing_token": 4, "completion_baseline": {"epoch": 4, "turns": 5}}

        parsed = SendResult.from_dict(payload)
        assert parsed.completion_baseline == CompletionCursor(epoch=4, turns=5)
        assert parsed.to_dict() == payload
        assert SendResult.from_dict(parsed.to_dict()) == parsed

    def test_payloads_without_the_field_still_deserialize(self):
        """Backward compatibility: an older gateway's payloads must parse, and
        absence must read as `None` — never as a fabricated zero cursor, which
        a caller could mistake for 'no turns yet'."""
        inspection = IdentityInspection.from_dict(
            {"identity": "triage:main", "output_preview": "ACK", "is_final": True}
        )
        assert inspection.completion_cursor is None
        assert inspection.output_preview == "ACK"
        assert inspection.is_final is True
        assert "completion_cursor" not in inspection.to_dict()

        dispatch = DispatchResult.from_dict({"fencing_token": 2, "durable": False})
        assert dispatch.completion_baseline is None
        assert dispatch.fencing_token == 2

        send = SendResult.from_dict({"fencing_token": 2})
        assert send.completion_baseline is None
        assert send.fencing_token == 2

    def test_null_cursor_reads_as_absent(self):
        """Live aliases report `completion_cursor: null` — not tracked, which
        is not the same as zero turns."""
        inspection = IdentityInspection.from_dict(
            {"identity": "live:alias", "completion_cursor": None}
        )
        assert inspection.completion_cursor is None


class TestDispatchTextConvenience:
    @pytest.mark.asyncio
    async def test_dispatch_text_and_wait_threads_its_own_baseline(self):
        transport = ScriptedTransport(
            dispatch={
                "fencing_token": 4,
                "durable": True,
                "completion_baseline": {"epoch": 4, "turns": 0},
            },
            inspections=[
                _inspection("triage:main", None, epoch=4, turns=0),
                _inspection("triage:main", "ACK", epoch=4, turns=1),
            ],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        output = await handle.dispatch_text_and_wait(
            "New event", origin="connector", correlation_id="event-1",
            idempotency_key="connector:event-1", timeout=5, poll_interval=0.01
        )

        assert output == "ACK"
        dispatched = next(
            call for call in transport.calls if call["method"] == "meta/dispatch"
        )
        assert dispatched["params"]["dispatch_input"]["origin"] == "connector"
        assert dispatched["params"]["dispatch_input"]["correlation_id"] == "event-1"
        assert dispatched["params"]["dispatch_input"]["idempotency_key"] == "connector:event-1"


class TestPerIdentityCorrelation:
    @pytest.mark.asyncio
    async def test_another_identitys_completion_does_not_satisfy_the_wait(self):
        """Dispatch A's wait is not satisfied by dispatch B's completion.

        The cursor is per-identity, so the waiter must only ever read its own
        identity's cursor — even while a busy neighbour is completing turns.
        """
        transport = ScriptedTransport(
            dispatch={
                "fencing_token": 4,
                "durable": True,
                "completion_baseline": {"epoch": 4, "turns": 2},
            },
            # This identity's cursor never moves, whatever anyone else does.
            inspections=[_inspection("triage:main", "ACK", epoch=4, turns=2)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        with pytest.raises(TimeoutError):
            await handle.dispatch_and_wait(
                DispatchInput(content="A", origin="system"),
                timeout=0.05,
                poll_interval=0.01,
            )

        polled = {
            call["params"]["identity"]
            for call in transport.calls
            if call["method"] == "meta/wait_for_completion"
        }
        assert polled == {"triage:main"}, (
            f"the waiter must poll only its own identity, polled {polled}"
        )
        assert transport.inspect_calls == 0, "no completion, so no member read"

    @pytest.mark.asyncio
    async def test_two_identities_carry_independent_cursors(self):
        """Turns on one identity never advance another's cursor."""
        busy = ScriptedTransport(
            inspections=[_inspection("worker:alpha", "done", epoch=9, turns=7)],
        )
        quiet = ScriptedTransport(
            inspections=[_inspection("triage:main", "ACK", epoch=4, turns=1)],
        )

        busy_cursor = (
            await IdentityAgentHandle(_make_runtime(busy), "worker:alpha").inspect()
        ).completion_cursor
        quiet_cursor = (
            await IdentityAgentHandle(_make_runtime(quiet), "triage:main").inspect()
        ).completion_cursor

        assert busy_cursor == CompletionCursor(epoch=9, turns=7)
        assert quiet_cursor == CompletionCursor(epoch=4, turns=1)
        assert (
            quiet_cursor.progress_since(CompletionCursor(epoch=4, turns=1))
            is CompletionProgress.PENDING
        ), "the busy neighbour's 7 turns must not register as progress here"


# ---------------------------------------------------------------------------
# Output waits are event-driven: no client-side polling
# ---------------------------------------------------------------------------


class TestOutputWaitsAreEventDriven:
    """``wait_for_output`` (bare) and ``wait_for_output_containing`` read the
    member once, then once per completion, each completion waited for
    server-side. A client-side ``inspect`` + ``sleep`` loop would show one
    inspect per poll interval and fail these counts."""

    @pytest.mark.asyncio
    async def test_bare_wait_reads_then_waits_for_the_first_completion(self):
        transport = ScriptedTransport(
            inspections=[
                _inspection("triage:main", None, epoch=4, turns=0),
                _inspection("triage:main", "hello", epoch=4, turns=1),
            ],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        output = await handle.wait_for_output(timeout=5, poll_interval=0.001)

        assert output == "hello"
        assert transport.wait_calls == 1, "one server-side wait"
        assert transport.inspect_calls == 2, "one read before, one at the completion"

    @pytest.mark.asyncio
    async def test_containing_reads_once_per_completion(self):
        transport = ScriptedTransport(
            inspections=[
                _inspection("triage:main", "working", epoch=4, turns=1),
                _inspection("triage:main", "still working", epoch=4, turns=2),
                _inspection("triage:main", "all DONE", epoch=4, turns=3),
            ],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        output = await handle.wait_for_output_containing(
            "DONE", timeout=5, poll_interval=0.001
        )

        assert output == "all DONE"
        assert transport.wait_calls == 2
        assert transport.inspect_calls == 3

    @pytest.mark.asyncio
    async def test_no_completion_means_a_single_read_until_the_deadline(self):
        transport = ScriptedTransport(
            inspections=[_inspection("triage:main", "working", epoch=4, turns=1)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "triage:main")

        with pytest.raises(TimeoutError, match="containing 'DONE'"):
            await handle.wait_for_output_containing(
                "DONE", timeout=0.05, poll_interval=0.001
            )

        assert transport.inspect_calls == 1, "no inspect polling while nothing completes"
        assert transport.wait_calls >= 1

    @pytest.mark.asyncio
    async def test_a_live_alias_without_a_cursor_is_refused_typed(self):
        transport = ScriptedTransport(
            inspections=[
                {
                    "identity": "live:alias",
                    "output_preview": None,
                    "is_final": False,
                    "peer_reachable_count": 0,
                }
            ],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "live:alias")

        with pytest.raises(CompletionCursorUnavailableError, match="no completion cursor") as raised:
            await handle.wait_for_output(timeout=5, poll_interval=0.001)
        assert raised.value.identity == "live:alias"
        assert isinstance(raised.value, RuntimeError), "callers catching RuntimeError still do"
        assert transport.inspect_calls == 1
