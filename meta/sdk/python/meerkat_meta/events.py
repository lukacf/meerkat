"""Typed event models for Meta SDK streaming.

Agent events are frozen dataclasses that support structural pattern matching::

    async for event in handle.subscribe_agent("agent-1"):
        match event:
            case TextDelta(delta=chunk):
                print(chunk, end="", flush=True)
            case RunCompleted(result=text):
                print(f"\\nDone: {text}")
            case RunFailed(error=err):
                print(f"Error: {err}")

Mob events wrap agent events with source attribution::

    async for event in handle.subscribe_mob():
        print(f"[{event.member_id}] {event.event.event_type}")
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any, AsyncIterator

from ._sse import SseEvent


# ---------------------------------------------------------------------------
# Base event
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class Event:
    """Base class for all agent events.

    Subclasses are frozen dataclasses whose positional ``__match_args__``
    enable clean structural pattern matching.
    """


# ---------------------------------------------------------------------------
# Session lifecycle
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class RunStarted(Event):
    """Agent run has started."""
    session_id: str = ""
    prompt: str = ""


@dataclass(frozen=True, slots=True)
class RunCompleted(Event):
    """Agent run completed successfully.

    ``structured_output`` is populated when the run was launched with a
    response schema and extraction succeeded; it carries the typed
    value parsed from ``result``. Absent (``None``) when the run had no
    schema, or schema extraction failed (in which case ``result`` is
    still the raw text).
    """
    session_id: str = ""
    result: str = ""
    structured_output: Any = None


@dataclass(frozen=True, slots=True)
class RunFailed(Event):
    """Agent run failed.

    ``error_report`` is meerkat's typed failure fact: ``class``, an optional
    ``reason`` object whose ``reason_type`` is the stable discriminator
    (``llm_auth_error``, ``llm_rate_limited``, ...), and ``message``. meerkat
    >= 0.7 carries no flat ``error`` string on the wire, so ``error`` is
    derived from ``error_report["message"]`` when the payload omits it; a
    payload that does carry ``error`` keeps it.
    """
    session_id: str = ""
    error: str = ""
    error_report: dict[str, Any] = field(default_factory=dict)

    def __post_init__(self) -> None:
        if not self.error and isinstance(self.error_report, dict):
            message = self.error_report.get("message")
            if isinstance(message, str):
                object.__setattr__(self, "error", message)

    @property
    def error_class(self) -> str:
        """``error_report["class"]`` (``llm``, ``tool``, ``auth``, ...), or ``""``."""
        value = self.error_report.get("class") if isinstance(self.error_report, dict) else None
        return value if isinstance(value, str) else ""

    @property
    def reason_type(self) -> str:
        """``error_report["reason"]["reason_type"]``, or ``""`` without a typed reason."""
        reason = self.error_report.get("reason") if isinstance(self.error_report, dict) else None
        value = reason.get("reason_type") if isinstance(reason, dict) else None
        return value if isinstance(value, str) else ""


# ---------------------------------------------------------------------------
# Turn / LLM
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class TurnStarted(Event):
    """A new LLM turn has begun."""
    turn_number: int = 0


@dataclass(frozen=True, slots=True)
class TextDelta(Event):
    """An incremental text chunk from the LLM."""
    delta: str = ""


@dataclass(frozen=True, slots=True)
class TextComplete(Event):
    """Full assistant text for the current turn."""
    content: str = ""


@dataclass(frozen=True, slots=True)
class ToolCallRequested(Event):
    """The LLM wants to invoke a tool."""
    id: str = ""
    name: str = ""
    args: Any = None


@dataclass(frozen=True, slots=True)
class ToolResultReceived(Event):
    """A tool result was fed back to the LLM.

    The only event that carries the tool's output: ``result`` is the text
    of its content. :class:`ToolExecutionCompleted` for the same ``id``
    carries the execution facts. An event from an older server, which
    carried the output on the completion instead, reads ``result`` as
    ``""``.
    """
    id: str = ""
    name: str = ""
    is_error: bool = False
    result: str = ""


@dataclass(frozen=True, slots=True)
class TurnCompleted(Event):
    """An LLM turn finished."""
    stop_reason: str = ""


# ---------------------------------------------------------------------------
# Tool execution
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class ToolExecutionStarted(Event):
    """A tool began executing."""
    id: str = ""
    name: str = ""


@dataclass(frozen=True, slots=True)
class ToolExecutionCompleted(Event):
    """A tool finished executing: whether it failed, and how long it took.

    It carries no output. The output is on :class:`ToolResultReceived`
    for the same ``id``. An event from an older server that still carries
    ``result`` parses, and the field is ignored.
    """
    id: str = ""
    name: str = ""
    is_error: bool = False
    duration_ms: int = 0


# ---------------------------------------------------------------------------
# Catch-all
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class UnknownEvent(Event):
    """An event type not recognized by this SDK version."""
    type: str = ""
    data: dict[str, Any] = field(default_factory=dict)


# ---------------------------------------------------------------------------
# Event map + parser
# ---------------------------------------------------------------------------

_EVENT_MAP: dict[str, type[Event]] = {
    "run_started": RunStarted,
    "run_completed": RunCompleted,
    "run_failed": RunFailed,
    "turn_started": TurnStarted,
    "text_delta": TextDelta,
    "text_complete": TextComplete,
    "tool_call_requested": ToolCallRequested,
    "tool_result_received": ToolResultReceived,
    "turn_completed": TurnCompleted,
    "tool_execution_started": ToolExecutionStarted,
    "tool_execution_completed": ToolExecutionCompleted,
}


def parse_agent_event(raw: dict[str, Any]) -> Event:
    """Parse a raw event dict into a typed :class:`Event`.

    Unknown event types are returned as :class:`UnknownEvent` for
    forward-compatibility with newer server versions.
    """
    event_type = raw.get("type", "")
    cls = _EVENT_MAP.get(event_type)
    if cls is None:
        return UnknownEvent(type=event_type, data=raw)

    kwargs: dict[str, Any] = {}
    for f in cls.__dataclass_fields__:
        if f in raw:
            kwargs[f] = raw[f]
    return cls(**kwargs)


# ---------------------------------------------------------------------------
# Mob-level event (wraps agent event with source attribution)
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class MobEvent:
    """A mob-level attributed event from the runtime.

    Wraps an agent :class:`Event` with the ``member_id`` of the agent
    that produced it.
    """
    member_id: str
    event: Event
    timestamp_ms: int = 0

    @classmethod
    def from_sse(cls, sse: SseEvent) -> MobEvent:
        """Parse from a mob SSE event (attributed envelope)."""
        try:
            raw_any = json.loads(sse.data)
        except (json.JSONDecodeError, TypeError):
            raw_any = {}
        # Defensive: the wire CONTRACT is a dict, but a buggy gateway
        # or wire-version skew can deliver a string / list / null. Pre-fix,
        # those returned an `UnknownEvent` with the raw value silently
        # discarded. Wrap them so consumers can recover the original.
        if not isinstance(raw_any, dict):
            return cls(
                member_id="",
                event=UnknownEvent(type="non_dict_payload", data={"raw": raw_any}),
                timestamp_ms=0,
            )
        raw: dict[str, Any] = raw_any
        member_id = raw.get("member_id", raw.get("source", ""))
        payload = raw.get("payload", raw)
        if isinstance(payload, dict):
            event = parse_agent_event(payload)
        else:
            event = UnknownEvent(type="non_dict_payload", data={"raw": payload})
        return cls(
            member_id=member_id,
            event=event,
            timestamp_ms=raw.get("timestamp_ms", 0),
        )


# ---------------------------------------------------------------------------
# Agent-level event (typed wrapper for per-agent SSE stream)
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class AgentEvent:
    """A per-agent event from the runtime.

    The ``event`` field contains the typed :class:`Event` subclass.
    Use pattern matching on it::

        match agent_event.event:
            case TextDelta(delta=chunk): ...
    """
    event_type: str
    event: Event

    @classmethod
    def from_sse(cls, sse: SseEvent, agent_id: str = "") -> AgentEvent:
        """Parse from an agent SSE event."""
        try:
            raw = json.loads(sse.data)
        except (json.JSONDecodeError, TypeError):
            raw = {}
        if isinstance(raw, dict):
            parsed = parse_agent_event(raw)
        else:
            # Pre-fix non-dict payloads dropped the raw value. Preserve
            # it on UnknownEvent so consumers can debug forward-compat
            # events instead of seeing an empty payload.
            parsed = UnknownEvent(type="non_dict_payload", data={"raw": raw})
        return cls(event_type=sse.event, event=parsed)


# ---------------------------------------------------------------------------
# EventStream (generic typed async iterator)
# ---------------------------------------------------------------------------

class EventStream:
    """Typed async iterator wrapping raw SSE events into domain events."""

    def __init__(self, source: AsyncIterator[SseEvent], event_cls: type, **kwargs: Any):
        self._source = source
        self._event_cls = event_cls
        self._kwargs = kwargs

    def __aiter__(self) -> EventStream:
        return self

    async def __anext__(self) -> MobEvent | AgentEvent:
        event = await self._source.__anext__()
        return self._event_cls.from_sse(event, **self._kwargs)
