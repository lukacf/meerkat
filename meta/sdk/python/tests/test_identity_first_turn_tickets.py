"""Per-admission completion for the Python SDK.

The defect: ``send_and_wait`` waited until the identity-wide completion cursor
passed the send's baseline, then returned the session's latest
``output_preview``. A concurrent delivery (a peer message, a scheduled turn,
a fork completion wake) completing first satisfied the wait, and the caller
got someone else's output.

``send_and_wait`` / ``dispatch*_and_wait`` now send with ``track_turn`` and
wait on the returned ticket through ``meta/turn_result``, which reports THAT
turn and its own output.
"""
import asyncio
import warnings

import pytest

from meerkat_meta.errors import (
    CompletionCursorUnavailableError,
    MetaError,
    TurnFailedError,
    TurnNotDeliveredError,
    TurnOutputTruncatedWarning,
    TurnOutputUnavailableError,
    TurnOutputUnavailableWarning,
    TurnTrackingUnavailableError,
    TurnTrackingUnavailableWarning,
    TurnUnknownError,
    TurnWaitTimeoutError,
    WaitEndedError,
)
from meerkat_meta.identity_first_models import (
    AwaitedTurn,
    CompletionCursor,
    DispatchInput,
    DispatchResult,
    SendResult,
    TurnOutputStatus,
    TurnResult,
    TurnState,
    TurnUnavailable,
)
from meerkat_meta.runtime import IdentityAgentHandle, MetaRuntime


class TicketTransport:
    """Answers each RPC from a script.

    ``sends`` is consumed one entry per ``meta/send`` (or dispatch) call;
    ``turn_results`` maps a ticket to the states ``meta/turn_result`` walks
    through, one per poll, holding the last. ``inspections`` does the same for
    the identity-wide cursor polls (``meta/completion_cursor``): it models
    the cursor moving for someone else's turn. ``meta/inspect_identity``
    answers the entry the latest cursor poll saw (the output a waiter reads
    once, at completion), or walks the script itself before any cursor poll.
    """

    def __init__(self, *, sends=None, turn_results=None, inspections=None):
        self.calls: list[dict] = []
        self.request_timeout = 60.0
        self._sends = list(sends or [])
        self._turn_results = {k: list(v) for k, v in (turn_results or {}).items()}
        self._turn_polls: dict[str, int] = {}
        self._inspections = list(inspections or [])
        self._inspect_index = 0
        self._cursor_reads = 0

    def _walk_inspections(self) -> dict:
        index = min(self._inspect_index, len(self._inspections) - 1)
        self._inspect_index += 1
        return self._inspections[index]

    def send_sync(self, request):
        self.calls.append(request)
        method = request.get("method")
        params = request.get("params") or {}
        if method in ("meta/send", "meta/dispatch"):
            result = self._sends.pop(0)
        elif method == "meta/turn_result":
            ticket = params["ticket"]
            script = self._turn_results.get(ticket, [{"state": "unknown"}])
            index = min(self._turn_polls.get(ticket, 0), len(script) - 1)
            self._turn_polls[ticket] = self._turn_polls.get(ticket, 0) + 1
            result = {"identity": params["identity"], "ticket": ticket, **script[index]}
        elif method == "meta/wait_for_turn":
            # The gateway's server-side wait: answer once the ticket leaves
            # pending, or at the deadline (the script ends still pending).
            ticket = params["ticket"]
            script = self._turn_results.get(ticket, [{"state": "unknown"}])
            index = min(self._turn_polls.get(ticket, 0), len(script) - 1)
            while index < len(script) - 1 and script[index].get("state") == "pending":
                index += 1
            self._turn_polls[ticket] = index + 1
            result = {"identity": params["identity"], "ticket": ticket, **script[index]}
            result["wait"] = "timed_out" if result.get("state") == "pending" else "settled"
        elif method == "meta/wait_for_completion":
            from .test_identity_first_completion_cursor import _serve_completion_wait
            result = _serve_completion_wait(
                self._walk_inspections,
                lambda: self._inspect_index >= len(self._inspections),
                params,
            )
            self._cursor_reads += 1
        elif method == "meta/completion_cursor":
            self._cursor_reads += 1
            entry = self._walk_inspections()
            result = {
                "identity": entry["identity"],
                "state": "active",
                "completion_cursor": entry.get("completion_cursor"),
            }
        elif method == "meta/inspect_identity":
            if self._cursor_reads:
                index = min(self._inspect_index - 1, len(self._inspections) - 1)
                result = self._inspections[index]
            else:
                result = self._walk_inspections()
        else:
            result = {}
        return {"jsonrpc": "2.0", "id": request.get("id"), "result": result}

    async def send_async(self, request, *, timeout=None):
        await asyncio.sleep(0)
        response = self.send_sync(request)
        result = response.get("result") or {}
        if isinstance(result, dict) and (
            result.get("outcome") == "timed_out" or result.get("wait") == "timed_out"
        ):
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

    def params_of(self, method: str) -> list[dict]:
        return [call.get("params") or {} for call in self.calls if call["method"] == method]

    def waited_turns(self) -> list[dict]:
        """The ``meta/wait_for_turn`` calls, minus their deadline."""
        return [
            {"identity": params["identity"], "ticket": params["ticket"]}
            for params in self.params_of("meta/wait_for_turn")
        ]


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


def _sent(ticket: str | None, *, turns: int = 0, unavailable: str | None = None) -> dict:
    result = {"fencing_token": 3, "completion_baseline": {"epoch": 3, "turns": turns}}
    if ticket is not None:
        result["turn"] = {"ticket": ticket}
    elif unavailable is not None:
        result["turn"] = None
        result["turn_unavailable"] = {"code": unavailable, "reason": f"because {unavailable}"}
    return result


def _completed(output: str | None, *, status: str | None = None, truncated: bool = False) -> dict:
    return {
        "state": "completed",
        "output_status": status or ("text" if output is not None else "empty"),
        "output": output,
        "output_truncated": truncated,
    }


def _inspection(preview: str | None, turns: int) -> dict:
    return {
        "identity": "keeper",
        "output_preview": preview,
        "is_final": False,
        "peer_reachable_count": 0,
        "completion_cursor": {"epoch": 3, "turns": turns},
    }


PENDING = {"state": "pending"}


class TestSendAndWaitWaitsForItsOwnTurn:
    @pytest.mark.asyncio
    async def test_a_foreign_completion_does_not_satisfy_the_wait(self):
        """THE defect: the identity's cursor moves past the baseline for a
        foreign turn while this send's turn is still running. The wait must
        return this turn's output, not the foreign preview."""
        transport = TicketTransport(
            sends=[_sent("ticket-a", turns=0)],
            turn_results={"ticket-a": [PENDING, PENDING, _completed("A's reply")]},
            # The identity-wide cursor has already moved for a foreign turn.
            inspections=[_inspection("foreign reply", turns=1)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        output = await handle.send_and_wait("alpha", timeout=5, poll_interval=0.001)

        assert output == "A's reply"
        assert transport.params_of("meta/send")[0]["track_turn"] is True
        assert transport.params_of("meta/inspect_identity") == [], (
            "a ticketed wait must not read the identity-wide cursor at all"
        )
        assert transport.params_of("meta/turn_result") == [], "no client polling"
        assert transport.waited_turns() == [{"identity": "keeper", "ticket": "ticket-a"}]
        assert transport.params_of("meta/wait_for_completion") == []

    @pytest.mark.asyncio
    async def test_concurrent_sends_each_get_their_own_output(self):
        transport = TicketTransport(
            sends=[_sent("ticket-a"), _sent("ticket-b")],
            turn_results={
                "ticket-a": [PENDING, PENDING, PENDING, _completed("A's reply")],
                "ticket-b": [PENDING, _completed("B's reply")],
            },
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        outputs = await asyncio.gather(
            handle.send_and_wait("alpha", timeout=5, poll_interval=0.001),
            handle.send_and_wait("beta", timeout=5, poll_interval=0.001),
        )

        assert outputs == ["A's reply", "B's reply"]

    @pytest.mark.asyncio
    async def test_the_old_cursor_wait_returns_the_foreign_preview(self):
        """Control: the identity-wide wait on the same data returns the
        foreign turn's preview. If this ever stops holding, the test above is
        passing for free."""
        transport = TicketTransport(inspections=[_inspection("foreign reply", turns=1)])
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        output = await handle.wait_for_completion(
            CompletionCursor(epoch=3, turns=0), timeout=5, poll_interval=0.001,
        )

        assert output == "foreign reply"

    @pytest.mark.asyncio
    async def test_dispatch_and_wait_waits_on_its_ticket(self):
        transport = TicketTransport(
            sends=[
                {**_sent("ticket-d"), "durable": True},
                {**_sent("ticket-t"), "durable": True},
            ],
            turn_results={
                "ticket-d": [_completed("dispatched reply")],
                "ticket-t": [_completed("text reply")],
            },
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        dispatched = await handle.dispatch_and_wait(
            DispatchInput(content="work", origin="system"), timeout=5, poll_interval=0.001,
        )
        texted = await handle.dispatch_text_and_wait(
            "more work", correlation_id="chat-1", idempotency_key="evt-1",
            timeout=5, poll_interval=0.001,
        )

        assert (dispatched, texted) == ("dispatched reply", "text reply")
        params = transport.params_of("meta/dispatch")
        assert all(p["track_turn"] is True for p in params)
        # The text helper carries the caller's dedup pair AND tracks the turn.
        assert params[1]["dispatch_input"]["idempotency_key"] == "evt-1"
        assert params[1]["dispatch_input"]["correlation_id"] == "chat-1"

    @pytest.mark.asyncio
    @pytest.mark.parametrize("through_handle", [False, True])
    @pytest.mark.parametrize("with_pair", [False, True])
    @pytest.mark.parametrize("tracked", [False, True])
    async def test_dispatch_text_preserves_idempotency_and_tracking(
        self, through_handle, with_pair, tracked,
    ):
        ticket = "06d4f5fc-43b4-430c-9a2c-1c08e1f1be34"
        transport = TicketTransport(sends=[{**_sent(ticket if tracked else None), "durable": True}])
        runtime = _make_runtime(transport)
        pair = (
            {"correlation_id": "lab-event-1", "idempotency_key": "lab:event-1"}
            if with_pair else {}
        )
        tracking = {"track_turn": True} if tracked else {}
        content = "Lab closed.\nKeep both paragraphs."

        if through_handle:
            result = await runtime.agent("keeper").dispatch_text(content, **pair, **tracking)
        else:
            result = await runtime.dispatch_text("keeper", content, **pair, **tracking)

        assert len(transport.calls) == 1
        assert transport.params_of("meta/dispatch") == [{
            "identity": "keeper",
            "dispatch_input": {"content": content, "origin": "system", **pair},
            **tracking,
        }]
        assert result.turn_ticket == (ticket if tracked else None)

    @pytest.mark.asyncio
    @pytest.mark.parametrize("with_pair", [False, True])
    async def test_dispatch_text_and_wait_preserves_idempotency_and_own_ticket(self, with_pair):
        ticket = "4a9bc1cc-0b66-4af6-bce4-926359b1dd8f"
        pair = (
            {"correlation_id": "lab-event-2", "idempotency_key": "lab:event-2"}
            if with_pair else {}
        )
        transport = TicketTransport(
            sends=[{**_sent(ticket), "durable": True}],
            turn_results={ticket: [_completed("Lab notice accepted")]},
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        output = await handle.dispatch_text_and_wait(
            "Lab notice", origin="connector", timeout=5, poll_interval=0.001, **pair,
        )

        assert output == "Lab notice accepted"
        assert transport.params_of("meta/dispatch") == [{
            "identity": "keeper",
            "dispatch_input": {"content": "Lab notice", "origin": "connector", **pair},
            "track_turn": True,
        }]
        assert transport.waited_turns() == [{"identity": "keeper", "ticket": ticket}]
        assert transport.params_of("meta/inspect_identity") == []


class TestWaitForTurn:
    @pytest.mark.asyncio
    async def test_a_failed_turn_raises_typed(self):
        transport = TicketTransport(
            turn_results={"ticket-a": [{"state": "failed", "error": "the model refused"}]},
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        with pytest.raises(TurnFailedError) as raised:
            await handle.wait_for_turn("ticket-a", timeout=5, poll_interval=0.001)
        assert raised.value.reason == "the model refused"
        assert raised.value.ticket == "ticket-a"

    @pytest.mark.asyncio
    async def test_an_unknown_ticket_raises_typed_and_never_guesses(self):
        transport = TicketTransport(turn_results={"ticket-a": [{"state": "unknown"}]})
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        with pytest.raises(TurnUnknownError):
            await handle.wait_for_turn("ticket-a", timeout=5, poll_interval=0.001)

    @pytest.mark.asyncio
    async def test_a_pending_turn_times_out(self):
        transport = TicketTransport(turn_results={"ticket-a": [PENDING]})
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        with pytest.raises(TimeoutError):
            await handle.wait_for_turn("ticket-a", timeout=0.02, poll_interval=0.001)

    @pytest.mark.asyncio
    async def test_the_result_is_typed(self):
        transport = TicketTransport(turn_results={
            "ticket-a": [_completed("A's reply")],
            "ticket-e": [_completed(None)],
            "ticket-n": [_completed(None, status="no_own_result")],
            "ticket-t": [_completed("long[truncated]", truncated=True)],
        })
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        text = await handle.wait_for_turn("ticket-a", timeout=5, poll_interval=0.001)
        assert (text.output_status, text.output) == (TurnOutputStatus.TEXT, "A's reply")
        empty = await handle.wait_for_turn("ticket-e", timeout=5, poll_interval=0.001)
        assert (empty.output_status, empty.output) == (TurnOutputStatus.EMPTY, None)
        folded = await handle.wait_for_turn("ticket-n", timeout=5, poll_interval=0.001)
        assert folded.output_status is TurnOutputStatus.NO_OWN_RESULT
        cut = await handle.wait_for_turn("ticket-t", timeout=5, poll_interval=0.001)
        assert cut.output_truncated is True

    @pytest.mark.asyncio
    async def test_send_and_wait_never_returns_partial_or_absent_output_silently(self):
        transport = TicketTransport(
            sends=[_sent("ticket-t"), _sent("ticket-n"), _sent("ticket-u"), _sent("ticket-e")],
            turn_results={
                "ticket-t": [_completed("long[truncated]", truncated=True)],
                "ticket-n": [_completed(None, status="no_own_result")],
                "ticket-u": [_completed(None, status="unavailable")],
                "ticket-e": [_completed(None)],
            },
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        with pytest.warns(TurnOutputTruncatedWarning):
            assert await handle.send_and_wait("t", timeout=5, poll_interval=0.001) == (
                "long[truncated]"
            )
        with pytest.warns(TurnOutputUnavailableWarning, match="no_own_result"):
            assert await handle.send_and_wait("n", timeout=5, poll_interval=0.001) is None
        with pytest.warns(TurnOutputUnavailableWarning, match="unavailable"):
            assert await handle.send_and_wait("u", timeout=5, poll_interval=0.001) is None
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            # The turn committed no text: None, and nothing to warn about.
            assert await handle.send_and_wait("e", timeout=5, poll_interval=0.001) is None

    @pytest.mark.asyncio
    async def test_wait_for_output_by_turn_returns_that_turns_output(self):
        transport = TicketTransport(
            turn_results={
                "ticket-a": [PENDING, _completed("A's reply")],
                "ticket-e": [_completed(None)],
                "ticket-n": [_completed(None, status="no_own_result")],
            },
            inspections=[_inspection("foreign reply", turns=5)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        assert await handle.wait_for_output(
            turn="ticket-a", timeout=5, poll_interval=0.001,
        ) == "A's reply"
        with pytest.raises(TurnOutputUnavailableError) as raised:
            await handle.wait_for_output(turn="ticket-e", timeout=5, poll_interval=0.001)
        assert raised.value.status == "empty"
        with pytest.raises(TurnOutputUnavailableError) as raised:
            await handle.wait_for_output(turn="ticket-n", timeout=5, poll_interval=0.001)
        assert raised.value.status == "no_own_result"
        with pytest.raises(ValueError, match="alone"):
            await handle.wait_for_output(
                turn="ticket-a", after=CompletionCursor(epoch=3, turns=0),
            )


class TestAttribution:
    """An untracked delivery never raises by default: it keeps the
    identity-wide wait, typed as non-attributed with its ``turn_unavailable``
    code and warned. Only ``require_attribution=True`` raises, for every
    code. Nothing is silent and nothing is resent."""

    @pytest.mark.asyncio
    async def test_a_tracked_outcome_is_attributed(self):
        transport = TicketTransport(
            sends=[_sent("t-own")], turn_results={"t-own": [_completed("own reply")]},
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        outcome = await handle.send_and_wait_outcome("alpha", timeout=5, poll_interval=0.001)

        assert isinstance(outcome, AwaitedTurn)
        assert (outcome.text, outcome.attributed, outcome.ticket) == ("own reply", True, "t-own")
        assert outcome.output_status is TurnOutputStatus.TEXT
        assert isinstance(outcome.admission, SendResult)
        assert outcome.untracked_code is None

    @pytest.mark.asyncio
    async def test_an_old_gateway_waits_identity_wide_typed_as_not_attributed(self):
        """A gateway that predates turn tickets returns no ``turn``: the
        default still waits on the identity-wide cursor, warns, and types the
        result as not attributed to this send."""
        transport = TicketTransport(
            sends=[_sent(None, turns=0)],
            inspections=[_inspection("latest reply", turns=1)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        with pytest.warns(TurnTrackingUnavailableWarning, match="predates turn tickets"):
            outcome = await handle.send_and_wait_outcome("alpha", timeout=5, poll_interval=0.001)

        assert (outcome.text, outcome.attributed, outcome.ticket) == ("latest reply", False, None)
        assert outcome.untracked_code is None
        assert isinstance(outcome.admission, SendResult)
        assert transport.params_of("meta/turn_result") == []

    @pytest.mark.asyncio
    async def test_the_default_mode_keeps_the_plain_text_api_working(self):
        """``autonomous_host`` is the default member mode and structurally
        untrackable: ``*_and_wait`` keeps returning text there (warned), so an
        upgrade does not break existing callers."""
        transport = TicketTransport(
            sends=[_sent(None, turns=0, unavailable="autonomous_host")],
            inspections=[_inspection("latest reply", turns=1)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        with pytest.warns(TurnTrackingUnavailableWarning, match="not attributed"):
            output = await handle.dispatch_text_and_wait("alpha", timeout=5, poll_interval=0.001)

        assert output == "latest reply"
        assert len(transport.params_of("meta/dispatch")) == 1, "delivered exactly once"

    @pytest.mark.asyncio
    @pytest.mark.parametrize(
        "code", ["autonomous_host", "externally_bound", "host_human_input",
                 "bridge_cannot_report_output", "runtime_refused",
                 "session_rotated", "a_future_code"],
    )
    async def test_every_untracked_code_waits_identity_wide_not_attributed(self, code):
        transport = TicketTransport(
            sends=[_sent(None, turns=0, unavailable=code)],
            inspections=[_inspection("latest reply", turns=1)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        with pytest.warns(TurnTrackingUnavailableWarning, match=code):
            outcome = await handle.dispatch_and_wait_outcome(
                DispatchInput(content="alpha", origin="system"), timeout=5, poll_interval=0.001,
            )
        assert outcome.attributed is False
        assert outcome.untracked_code == code
        assert isinstance(outcome.admission, DispatchResult)

    @pytest.mark.asyncio
    @pytest.mark.parametrize("unavailable", ["autonomous_host", "runtime_refused", None])
    async def test_require_attribution_raises_in_every_untracked_case(self, unavailable):
        transport = TicketTransport(
            sends=[_sent(None, turns=0, unavailable=unavailable)],
            inspections=[_inspection("latest reply", turns=1)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        with warnings.catch_warnings():
            warnings.simplefilter("error", TurnTrackingUnavailableWarning)
            with pytest.raises(TurnTrackingUnavailableError) as raised:
                await handle.send_and_wait(
                    "alpha", timeout=5, poll_interval=0.001, require_attribution=True,
                )
        assert raised.value.code == unavailable
        assert isinstance(raised.value.admission, SendResult)
        assert len(transport.params_of("meta/send")) == 1
        assert transport.params_of("meta/inspect_identity") == []

    @pytest.mark.asyncio
    async def test_an_undelivered_send_raises_instead_of_waiting(self):
        sent = _sent(None, turns=0)
        sent["turn"] = None
        sent["turn_unavailable"] = {
            "code": "not_delivered", "reason": "no session bridge", "delivered": False,
        }
        transport = TicketTransport(sends=[sent])
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        with pytest.raises(TurnNotDeliveredError) as raised:
            await handle.send_and_wait("alpha", timeout=5, poll_interval=0.001)
        assert isinstance(raised.value, MetaError)
        assert isinstance(raised.value.admission, SendResult)
        assert (raised.value.code, raised.value.reason) == ("not_delivered", "no session bridge")
        assert transport.params_of("meta/inspect_identity") == []

    @pytest.mark.asyncio
    async def test_plain_send_does_not_request_tracking(self):
        transport = TicketTransport(sends=[_sent(None)])
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")

        result = await handle.send("alpha")

        assert "track_turn" not in transport.params_of("meta/send")[0]
        assert result.turn_ticket is None


class _CursorOutcomeTransport(TicketTransport):
    """Answers the identity-wide ``meta/wait_for_completion`` with a fixed
    typed outcome, to drive the default (untracked) wait's failure paths."""

    def __init__(self, *, sends, outcome: str, cursor: dict | None):
        super().__init__(sends=sends)
        self._outcome = outcome
        self._cursor = cursor

    def send_sync(self, request):
        if request.get("method") == "meta/wait_for_completion":
            self.calls.append(request)
            params = request.get("params") or {}
            result = {
                "identity": params.get("identity"),
                "outcome": self._outcome,
                "completion_cursor": self._cursor,
            }
            return {"jsonrpc": "2.0", "id": request.get("id"), "result": result}
        return super().send_sync(request)


class TestDefaultPathCustody:
    """Every failure after an untracked admission keeps its own type and
    still carries the exact admission (and no ticket), with one dispatch."""

    async def _fail(self, transport):
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", TurnTrackingUnavailableWarning)
            with pytest.raises(BaseException) as raised:
                await handle.dispatch_text_and_wait("alpha", timeout=0.2, poll_interval=0.001)
        assert len(transport.params_of("meta/dispatch")) == 1, "dispatched exactly once"
        return raised.value

    @pytest.mark.asyncio
    async def test_a_wait_that_ended_carries_the_admission(self):
        error = await self._fail(_CursorOutcomeTransport(
            sends=[_sent(None, unavailable="autonomous_host")],
            outcome="broken", cursor={"epoch": 3, "turns": 0},
        ))
        assert isinstance(error, WaitEndedError)
        assert error.outcome == "broken"
        assert isinstance(error.admission, DispatchResult)
        assert error.ticket is None

    @pytest.mark.asyncio
    async def test_the_cursor_deadline_is_a_timeout_carrying_the_admission(self):
        error = await self._fail(_CursorOutcomeTransport(
            sends=[_sent(None, unavailable="autonomous_host")],
            outcome="timed_out", cursor={"epoch": 3, "turns": 0},
        ))
        assert isinstance(error, TimeoutError)
        assert isinstance(error, TurnWaitTimeoutError)
        assert isinstance(error.admission, DispatchResult)
        assert error.ticket is None

    @pytest.mark.asyncio
    async def test_a_missing_baseline_carries_the_admission(self):
        sent = _sent(None, unavailable="autonomous_host")
        del sent["completion_baseline"]
        error = await self._fail(_CursorOutcomeTransport(
            sends=[sent], outcome="completed", cursor={"epoch": 3, "turns": 1},
        ))
        assert type(error) is RuntimeError
        assert "no completion_baseline" in str(error)
        assert isinstance(error.admission, DispatchResult)
        assert error.ticket is None

    @pytest.mark.asyncio
    async def test_an_incarnation_change_carries_the_admission(self):
        error = await self._fail(_CursorOutcomeTransport(
            sends=[_sent(None, unavailable="autonomous_host")],
            outcome="incarnation_changed", cursor={"epoch": 4, "turns": 0},
        ))
        assert type(error) is RuntimeError
        assert "superseded runtime incarnation" in str(error)
        assert isinstance(error.admission, DispatchResult)
        assert error.ticket is None

    @pytest.mark.asyncio
    async def test_an_untracked_alias_carries_the_admission(self):
        error = await self._fail(_CursorOutcomeTransport(
            sends=[_sent(None, unavailable="autonomous_host")],
            outcome="untracked", cursor=None,
        ))
        assert type(error) is CompletionCursorUnavailableError
        assert isinstance(error, RuntimeError)
        assert isinstance(error.admission, DispatchResult)
        assert error.ticket is None


class TestWarningsAsErrorsCustody:
    """A caller that escalates the SDK's warnings to errors still gets the
    admission on the raised warning, and the warnings still name the
    caller's own line."""

    @pytest.mark.asyncio
    async def test_the_untracked_wait_warning_raised_as_an_error_carries_the_admission(self):
        transport = TicketTransport(
            sends=[_sent(None, turns=0, unavailable="autonomous_host")],
            inspections=[_inspection("latest reply", turns=1)],
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")
        with warnings.catch_warnings():
            warnings.simplefilter("error", TurnTrackingUnavailableWarning)
            with pytest.raises(TurnTrackingUnavailableWarning) as raised:
                await handle.dispatch_text_and_wait("alpha", timeout=5, poll_interval=0.001)
        assert isinstance(raised.value.admission, DispatchResult)
        assert raised.value.ticket is None
        assert len(transport.params_of("meta/dispatch")) == 1

    @pytest.mark.asyncio
    @pytest.mark.parametrize(
        ("completed", "category"),
        [
            (_completed("cut", truncated=True), TurnOutputTruncatedWarning),
            (_completed(None, status="no_own_result"), TurnOutputUnavailableWarning),
        ],
    )
    async def test_an_output_warning_raised_as_an_error_carries_the_admission(
        self, completed, category,
    ):
        transport = TicketTransport(
            sends=[_sent("t-own")], turn_results={"t-own": [completed]},
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "keeper")
        with warnings.catch_warnings():
            warnings.simplefilter("error", category)
            with pytest.raises(category) as raised:
                await handle.send_and_wait("alpha", timeout=5, poll_interval=0.001)
        assert isinstance(raised.value.admission, SendResult)
        assert raised.value.ticket == "t-own"
        assert len(transport.params_of("meta/send")) == 1

    @pytest.mark.asyncio
    async def test_warnings_still_name_the_callers_line(self):
        untracked = TicketTransport(
            sends=[_sent(None, turns=0, unavailable="autonomous_host")],
            inspections=[_inspection("latest reply", turns=1)],
        )
        truncated = TicketTransport(
            sends=[_sent("t-own")],
            turn_results={"t-own": [_completed("cut", truncated=True)]},
        )
        with warnings.catch_warnings(record=True) as seen:
            warnings.simplefilter("always")
            await IdentityAgentHandle(_make_runtime(untracked), "keeper").send_and_wait(
                "alpha", timeout=5, poll_interval=0.001,
            )
            await IdentityAgentHandle(_make_runtime(truncated), "keeper").send_and_wait_outcome(
                "alpha", timeout=5, poll_interval=0.001,
            )
        sdk_warnings = [
            w for w in seen
            if issubclass(w.category, (TurnTrackingUnavailableWarning, TurnOutputTruncatedWarning))
        ]
        assert len(sdk_warnings) == 2
        assert all(w.filename == __file__ for w in sdk_warnings), [w.filename for w in sdk_warnings]


class TestModels:
    def test_results_carry_the_ticket_and_round_trip(self):
        sent = SendResult.from_dict(_sent("ticket-a", turns=2))
        assert sent.turn_ticket == "ticket-a"
        assert SendResult.from_dict(sent.to_dict()) == sent
        unavailable = DispatchResult.from_dict(
            {**_sent(None, unavailable="no bridge"), "durable": False}
        )
        assert unavailable.turn_ticket is None
        assert unavailable.turn_unavailable == TurnUnavailable(
            code="no bridge", reason="because no bridge", delivered=True
        )
        assert DispatchResult.from_dict(unavailable.to_dict()) == unavailable
        # Older payloads without the flag: only not_delivered means undelivered.
        assert TurnUnavailable.from_wire({"code": "not_delivered", "reason": "x"}).delivered is False
        assert TurnUnavailable.from_wire({"code": "autonomous_host", "reason": "x"}).delivered is True

    def test_turn_result_parses_every_state(self):
        completed = TurnResult.from_dict(
            {"identity": "keeper", "ticket": "t", **_completed("hi"),
             "completion_cursor": {"epoch": 3, "turns": 4}}
        )
        assert completed.state is TurnState.COMPLETED
        assert completed.output_status is TurnOutputStatus.TEXT
        assert completed.output == "hi"
        assert completed.completion_cursor == CompletionCursor(epoch=3, turns=4)
        assert TurnResult.from_dict(completed.to_dict()) == completed
        failed = TurnResult.from_dict({"identity": "keeper", "ticket": "t",
                                       "state": "failed", "error": "boom"})
        assert (failed.state, failed.error) == (TurnState.FAILED, "boom")
        assert TurnResult.from_dict({"state": "surprise"}).state is TurnState.UNKNOWN
        future = TurnResult.from_dict({"state": "completed", "output_status": "later", "output": "x"})
        assert future.output_status is TurnOutputStatus.UNAVAILABLE
        assert future.output is None, "text is only read for the text status"
