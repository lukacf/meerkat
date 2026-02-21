"""Typed event hierarchy for the Meerkat streaming API.

Every event emitted by the agent loop is represented as a frozen dataclass,
enabling ``match``/``case`` pattern matching (Python 3.10+) and IDE
autocompletion on event fields.

Example::

    async for event in session.stream("Explain monads"):
        match event:
            case TextDelta(delta=chunk):
                print(chunk, end="", flush=True)
            case ToolCallRequested(name=name):
                print(f"\\nCalling tool: {name}")
            case TurnCompleted(usage=u):
                print(f"\\nTokens: {u.input_tokens} in / {u.output_tokens} out")
            case _:
                pass
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


# ---------------------------------------------------------------------------
# Shared value types (also re-exported from types.py)
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class Usage:
    """Token usage for a single LLM turn."""

    input_tokens: int = 0
    output_tokens: int = 0
    cache_creation_tokens: int | None = None
    cache_read_tokens: int | None = None


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
    """Agent run completed successfully."""

    session_id: str = ""
    result: str = ""
    usage: Usage = field(default_factory=Usage)


@dataclass(frozen=True, slots=True)
class RunFailed(Event):
    """Agent run failed."""

    session_id: str = ""
    error: str = ""


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
    """A tool result was fed back to the LLM."""

    id: str = ""
    name: str = ""
    is_error: bool = False


@dataclass(frozen=True, slots=True)
class TurnCompleted(Event):
    """An LLM turn finished."""

    stop_reason: str = ""
    usage: Usage = field(default_factory=Usage)


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
    """A tool finished executing."""

    id: str = ""
    name: str = ""
    result: str = ""
    is_error: bool = False
    duration_ms: int = 0


@dataclass(frozen=True, slots=True)
class ToolExecutionTimedOut(Event):
    """A tool execution exceeded its timeout."""

    id: str = ""
    name: str = ""
    timeout_ms: int = 0


# ---------------------------------------------------------------------------
# Compaction
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class CompactionStarted(Event):
    """Context compaction has begun."""

    input_tokens: int = 0
    estimated_history_tokens: int = 0
    message_count: int = 0


@dataclass(frozen=True, slots=True)
class CompactionCompleted(Event):
    """Context compaction finished."""

    summary_tokens: int = 0
    messages_before: int = 0
    messages_after: int = 0


@dataclass(frozen=True, slots=True)
class CompactionFailed(Event):
    """Context compaction failed."""

    error: str = ""


# ---------------------------------------------------------------------------
# Budget
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class BudgetWarning(Event):
    """A budget threshold has been crossed."""

    budget_type: str = ""
    used: int = 0
    limit: int = 0
    percent: float = 0.0


# ---------------------------------------------------------------------------
# Retry
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class Retrying(Event):
    """An LLM request is being retried after a transient failure."""

    attempt: int = 0
    max_attempts: int = 0
    error: str = ""
    delay_ms: int = 0


# ---------------------------------------------------------------------------
# Hooks
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class HookStarted(Event):
    """A hook invocation started."""

    hook_id: str = ""
    point: str = ""


@dataclass(frozen=True, slots=True)
class HookCompleted(Event):
    """A hook invocation completed."""

    hook_id: str = ""
    point: str = ""
    duration_ms: int = 0


@dataclass(frozen=True, slots=True)
class HookFailed(Event):
    """A hook invocation failed."""

    hook_id: str = ""
    point: str = ""
    error: str = ""


@dataclass(frozen=True, slots=True)
class HookDenied(Event):
    """A hook denied the current operation."""

    hook_id: str = ""
    point: str = ""
    reason_code: str = ""
    message: str = ""
    payload: Any = None


@dataclass(frozen=True, slots=True)
class HookRewriteApplied(Event):
    """A hook rewrote part of the request or response."""

    hook_id: str = ""
    point: str = ""
    patch: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class HookPatchPublished(Event):
    """A hook patch envelope was published."""

    hook_id: str = ""
    point: str = ""
    envelope: dict[str, Any] = field(default_factory=dict)


# ---------------------------------------------------------------------------
# Skills
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class SkillsResolved(Event):
    """Skills were resolved for this turn."""

    skills: list[str] = field(default_factory=list)
    injection_bytes: int = 0


@dataclass(frozen=True, slots=True)
class SkillResolutionFailed(Event):
    """A skill reference could not be resolved."""

    reference: str = ""
    error: str = ""


# ---------------------------------------------------------------------------
# Interaction (comms)
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class InteractionComplete(Event):
    """An interaction completed successfully."""

    interaction_id: str = ""
    result: str = ""


@dataclass(frozen=True, slots=True)
class InteractionFailed(Event):
    """An interaction failed."""

    interaction_id: str = ""
    error: str = ""


# ---------------------------------------------------------------------------
# Stream management
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class StreamTruncated(Event):
    """The event stream was truncated."""

    reason: str = ""


# ---------------------------------------------------------------------------
# Unknown / forward-compat
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class UnknownEvent(Event):
    """An event type not recognised by this SDK version.

    The ``type`` field contains the wire discriminator and ``data`` holds
    the raw dict so callers can still inspect it.
    """

    type: str = ""
    data: dict[str, Any] = field(default_factory=dict)


# ---------------------------------------------------------------------------
# Wire → typed parser
# ---------------------------------------------------------------------------

_USAGE_DEFAULTS = Usage()

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
    "tool_execution_timed_out": ToolExecutionTimedOut,
    "compaction_started": CompactionStarted,
    "compaction_completed": CompactionCompleted,
    "compaction_failed": CompactionFailed,
    "budget_warning": BudgetWarning,
    "retrying": Retrying,
    "hook_started": HookStarted,
    "hook_completed": HookCompleted,
    "hook_failed": HookFailed,
    "hook_denied": HookDenied,
    "hook_rewrite_applied": HookRewriteApplied,
    "hook_patch_published": HookPatchPublished,
    "skills_resolved": SkillsResolved,
    "skill_resolution_failed": SkillResolutionFailed,
    "interaction_complete": InteractionComplete,
    "interaction_failed": InteractionFailed,
    "stream_truncated": StreamTruncated,
}


def _parse_usage(raw: dict[str, Any] | None) -> Usage:
    if not raw:
        return _USAGE_DEFAULTS
    return Usage(
        input_tokens=raw.get("input_tokens", 0),
        output_tokens=raw.get("output_tokens", 0),
        cache_creation_tokens=raw.get("cache_creation_tokens"),
        cache_read_tokens=raw.get("cache_read_tokens"),
    )


def parse_event(raw: dict[str, Any]) -> Event:
    """Parse a raw event dict (from the wire) into a typed :class:`Event`.

    Unknown event types are returned as :class:`UnknownEvent` for
    forward-compatibility with newer server versions.
    """
    event_type = raw.get("type", "")
    cls = _EVENT_MAP.get(event_type)
    if cls is None:
        return UnknownEvent(type=event_type, data=raw)

    # Build kwargs, injecting parsed Usage where needed
    kwargs: dict[str, Any] = {}
    for f in cls.__dataclass_fields__:
        if f == "usage":
            kwargs["usage"] = _parse_usage(raw.get("usage"))
        elif f in raw:
            kwargs[f] = raw[f]
    return cls(**kwargs)
