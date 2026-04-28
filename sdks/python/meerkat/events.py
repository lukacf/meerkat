"""Typed event hierarchy for the Meerkat streaming API.

Every event emitted by the agent loop is represented as a frozen dataclass,
enabling ``match``/``case`` pattern matching (Python 3.10+) and IDE
autocompletion on event fields.

Missing semantic fields remain absent so partially delivered streaming payloads
do not become authoritative SDK state.
Malformed known wire payloads are preserved as ``UnknownEvent`` instances with
``type="malformed_event"`` instead of being coerced into typed semantic events
with fabricated defaults.

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
from typing import TYPE_CHECKING, Any, Literal, cast

if TYPE_CHECKING:
    from .types import SkillKey

ContentInput = str | list[dict[str, Any]]


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
    prompt: ContentInput = ""


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
    error_class: str = ""
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
    is_error: bool | None = None


@dataclass(frozen=True, slots=True)
class TurnCompleted(Event):
    """An LLM turn finished."""

    stop_reason: str | None = None
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
    is_error: bool | None = None
    duration_ms: int | None = None


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
class SkillResolutionFailureReason:
    """Structured reason a skill reference could not be resolved."""

    reason_type: str | None = None
    key: SkillKey | None = None
    capability: str | None = None
    message: str = ""
    source_uuid: str = ""
    skill_name: str = ""
    existing_fingerprint: str = ""
    new_fingerprint: str = ""
    fingerprint: str = ""
    existing_source_uuid: str = ""
    mutated_source_uuid: str = ""
    event_id: str = ""
    event_kind: str = ""
    from_source_uuid: str = ""
    from_skill_name: str = ""
    to_source_uuid: str = ""
    to_skill_name: str = ""
    alias: str = ""
    raw_reason_type: str | None = None


@dataclass(frozen=True, slots=True)
class SkillResolutionFailed(Event):
    """A skill reference could not be resolved."""

    skill_key: SkillKey | None = None
    reason: SkillResolutionFailureReason | None = None
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


@dataclass(frozen=True, slots=True)
class BoundaryAppliedToolConfigChangeStatus:
    """Structured status for a tool-scope boundary apply."""

    kind: Literal["boundary_applied"] = "boundary_applied"
    base_changed: bool = False
    visible_changed: bool = False
    revision: int = 0


@dataclass(frozen=True, slots=True)
class DeferredCatalogDeltaToolConfigChangeStatus:
    """Structured status for a deferred-catalog boundary delta."""

    kind: Literal["deferred_catalog_delta"] = "deferred_catalog_delta"
    added_hidden_count: int = 0
    removed_hidden_count: int = 0
    pending_source_count: int = 0


@dataclass(frozen=True, slots=True)
class WarningFailedClosedToolConfigChangeStatus:
    """Structured status for fail-closed tool-scope warnings."""

    kind: Literal["warning_failed_closed"] = "warning_failed_closed"
    error: str = ""


@dataclass(frozen=True, slots=True)
class ExternalToolDeltaToolConfigChangeStatus:
    """Structured status for external-tool lifecycle deltas."""

    kind: Literal["external_tool_delta"] = "external_tool_delta"
    phase: Literal["pending", "applied", "draining", "forced", "failed"] = "pending"
    detail: str | None = None


ToolConfigChangeStatus = (
    BoundaryAppliedToolConfigChangeStatus
    | DeferredCatalogDeltaToolConfigChangeStatus
    | WarningFailedClosedToolConfigChangeStatus
    | ExternalToolDeltaToolConfigChangeStatus
)


@dataclass(frozen=True, slots=True)
class ToolConfigChangedPayload:
    """Payload for tool configuration change notifications."""

    operation: str | None = None
    target: str | None = None
    status: str | None = None
    status_info: ToolConfigChangeStatus | None = None
    persisted: bool | None = None
    applied_at_turn: int | None = None


@dataclass(frozen=True, slots=True)
class ToolConfigChanged(Event):
    """Live tool configuration changed for this session."""

    payload: ToolConfigChangedPayload = field(default_factory=ToolConfigChangedPayload)


# ---------------------------------------------------------------------------
# Scoped streaming attribution
# ---------------------------------------------------------------------------

@dataclass(frozen=True, slots=True)
class ScopedEvent(Event):
    """A scoped wrapper around a base agent event."""

    scope_id: str = ""
    scope_path: list[dict[str, Any]] = field(default_factory=list)
    event: Event = field(default_factory=Event)


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

_STOP_REASONS = frozenset({
    "end_turn",
    "tool_use",
    "max_tokens",
    "stop_sequence",
    "content_filter",
    "cancelled",
})

_TOOL_CONFIG_OPERATIONS = frozenset({"add", "remove", "reload"})

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
    "tool_config_changed": ToolConfigChanged,
}


def _malformed(raw: dict[str, Any], _error: str) -> UnknownEvent:
    return UnknownEvent(type="malformed_event", data=raw)


def _is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _parse_usage(raw: dict[str, Any] | None) -> Usage:
    if not isinstance(raw, dict):
        raise ValueError("missing usage")
    if not _is_number(raw.get("input_tokens")):
        raise ValueError("usage.input_tokens must be number")
    if not _is_number(raw.get("output_tokens")):
        raise ValueError("usage.output_tokens must be number")
    if raw.get("cache_creation_tokens") is not None and not _is_number(raw.get("cache_creation_tokens")):
        raise ValueError("usage.cache_creation_tokens must be number")
    if raw.get("cache_read_tokens") is not None and not _is_number(raw.get("cache_read_tokens")):
        raise ValueError("usage.cache_read_tokens must be number")
    return Usage(
        input_tokens=raw["input_tokens"],
        output_tokens=raw["output_tokens"],
        cache_creation_tokens=raw.get("cache_creation_tokens"),
        cache_read_tokens=raw.get("cache_read_tokens"),
    )


def _parse_tool_config_change_status(raw: Any) -> ToolConfigChangeStatus | None:
    if not isinstance(raw, dict):
        return None

    kind = str(raw.get("kind", ""))
    if kind == "boundary_applied":
        return BoundaryAppliedToolConfigChangeStatus(
            base_changed=raw.get("base_changed") is True,
            visible_changed=raw.get("visible_changed") is True,
            revision=raw.get("revision", 0) if isinstance(raw.get("revision"), int) else 0,
        )
    if kind == "deferred_catalog_delta":
        return DeferredCatalogDeltaToolConfigChangeStatus(
            added_hidden_count=raw.get("added_hidden_count", 0)
            if isinstance(raw.get("added_hidden_count"), int)
            else 0,
            removed_hidden_count=raw.get("removed_hidden_count", 0)
            if isinstance(raw.get("removed_hidden_count"), int)
            else 0,
            pending_source_count=raw.get("pending_source_count", 0)
            if isinstance(raw.get("pending_source_count"), int)
            else 0,
        )
    if kind == "warning_failed_closed":
        return WarningFailedClosedToolConfigChangeStatus(error=str(raw.get("error", "")))
    if kind == "external_tool_delta":
        phase = str(raw.get("phase", "pending"))
        if phase not in {"pending", "applied", "draining", "forced", "failed"}:
            phase = "pending"
        return ExternalToolDeltaToolConfigChangeStatus(
            phase=cast(Literal["pending", "applied", "draining", "forced", "failed"], phase),
            detail=str(raw["detail"]) if raw.get("detail") is not None else None,
        )
    return None


def _parse_skill_key(raw: Any) -> SkillKey | None:
    if not isinstance(raw, dict):
        return None

    from .types import SkillKey

    return SkillKey(
        source_uuid=str(raw.get("source_uuid", raw.get("sourceUuid", ""))),
        skill_name=str(raw.get("skill_name", raw.get("skillName", ""))),
    )


def _parse_optional_str(raw: Any) -> str | None:
    return raw if isinstance(raw, str) else None


def _parse_optional_bool(raw: Any) -> bool | None:
    return raw if isinstance(raw, bool) else None


def _parse_optional_int(raw: Any) -> int | None:
    return raw if isinstance(raw, int) and not isinstance(raw, bool) else None


def _parse_stop_reason(raw: Any) -> str | None:
    return raw if isinstance(raw, str) and raw in _STOP_REASONS else None


def _parse_tool_config_operation(raw: Any) -> str | None:
    return raw if isinstance(raw, str) and raw in _TOOL_CONFIG_OPERATIONS else None


def _parse_skill_resolution_failure_reason(
    raw: Any,
    fallback_message: str,
) -> SkillResolutionFailureReason | None:
    if not isinstance(raw, dict):
        return None

    reason_type = raw.get("reason_type", raw.get("reasonType"))
    if not isinstance(reason_type, str):
        return None
    known_reason_types = {
        "not_found",
        "capability_unavailable",
        "load",
        "parse",
        "source_uuid_collision",
        "source_uuid_mutation_without_lineage",
        "missing_skill_remaps",
        "remap_without_lineage",
        "unknown_skill_alias",
        "remap_cycle",
        "unknown",
    }
    normalized_reason_type = reason_type if reason_type in known_reason_types else "unknown"

    return SkillResolutionFailureReason(
        reason_type=normalized_reason_type,
        key=_parse_skill_key(raw.get("key")),
        capability=str(raw.get("capability", "")),
        message=str(raw.get("message", fallback_message)),
        source_uuid=str(raw.get("source_uuid", raw.get("sourceUuid", ""))),
        skill_name=str(raw.get("skill_name", raw.get("skillName", ""))),
        existing_fingerprint=str(
            raw.get("existing_fingerprint", raw.get("existingFingerprint", ""))
        ),
        new_fingerprint=str(raw.get("new_fingerprint", raw.get("newFingerprint", ""))),
        fingerprint=str(raw.get("fingerprint", "")),
        existing_source_uuid=str(
            raw.get("existing_source_uuid", raw.get("existingSourceUuid", ""))
        ),
        mutated_source_uuid=str(
            raw.get("mutated_source_uuid", raw.get("mutatedSourceUuid", ""))
        ),
        event_id=str(raw.get("event_id", raw.get("eventId", ""))),
        event_kind=str(raw.get("event_kind", raw.get("eventKind", ""))),
        from_source_uuid=str(raw.get("from_source_uuid", raw.get("fromSourceUuid", ""))),
        from_skill_name=str(raw.get("from_skill_name", raw.get("fromSkillName", ""))),
        to_source_uuid=str(raw.get("to_source_uuid", raw.get("toSourceUuid", ""))),
        to_skill_name=str(raw.get("to_skill_name", raw.get("toSkillName", ""))),
        alias=str(raw.get("alias", "")),
        raw_reason_type=reason_type if reason_type not in known_reason_types else None,
    )


def _require_str(raw: dict[str, Any], field_name: str) -> str:
    value = raw.get(field_name)
    if not isinstance(value, str):
        raise ValueError(f"{field_name} must be string")
    return value


def _require_number(raw: dict[str, Any], field_name: str) -> int | float:
    value = raw.get(field_name)
    if not _is_number(value):
        raise ValueError(f"{field_name} must be number")
    return value


def _require_bool(raw: dict[str, Any], field_name: str) -> bool:
    value = raw.get(field_name)
    if not isinstance(value, bool):
        raise ValueError(f"{field_name} must be boolean")
    return value


_STRING_FIELDS = {
    "session_id",
    "result",
    "error_class",
    "error",
    "delta",
    "content",
    "id",
    "name",
    "stop_reason",
    "reason",
    "budget_type",
    "hook_id",
    "point",
    "reason_code",
    "message",
    "reference",
    "interaction_id",
}

_NUMBER_FIELDS = {
    "turn_number",
    "duration_ms",
    "timeout_ms",
    "input_tokens",
    "estimated_history_tokens",
    "message_count",
    "summary_tokens",
    "messages_before",
    "messages_after",
    "used",
    "limit",
    "percent",
    "attempt",
    "max_attempts",
    "delay_ms",
    "injection_bytes",
}

_BOOL_FIELDS = {"is_error"}


def _validate_known_event(event_type: str, raw: dict[str, Any]) -> None:
    required: dict[str, tuple[str, ...]] = {
        "run_started": ("session_id", "prompt"),
        "run_completed": ("session_id", "result", "usage"),
        "run_failed": ("session_id", "error_class", "error"),
        "turn_started": ("turn_number",),
        "text_delta": ("delta",),
        "text_complete": ("content",),
        "tool_call_requested": ("id", "name"),
        "tool_result_received": ("id", "name", "is_error"),
        "turn_completed": ("stop_reason", "usage"),
        "tool_execution_started": ("id", "name"),
        "tool_execution_completed": ("id", "name", "result"),
        "tool_execution_timed_out": ("id", "name", "timeout_ms"),
        "compaction_started": ("input_tokens", "estimated_history_tokens", "message_count"),
        "compaction_completed": ("summary_tokens", "messages_before", "messages_after"),
        "compaction_failed": ("error",),
        "budget_warning": ("budget_type", "used", "limit", "percent"),
        "retrying": ("attempt", "max_attempts", "error", "delay_ms"),
        "hook_started": ("hook_id", "point"),
        "hook_completed": ("hook_id", "point", "duration_ms"),
        "hook_failed": ("hook_id", "point", "error"),
        "hook_denied": ("hook_id", "point", "reason_code", "message"),
        "hook_rewrite_applied": ("hook_id", "point", "patch"),
        "hook_patch_published": ("hook_id", "point", "envelope"),
        "skills_resolved": ("skills", "injection_bytes"),
        "skill_resolution_failed": ("reference", "error"),
        "interaction_complete": ("interaction_id", "result"),
        "interaction_failed": ("interaction_id", "error"),
        "stream_truncated": ("reason",),
    }
    if event_type == "tool_config_changed":
        payload = raw.get("payload")
        if not isinstance(payload, dict):
            raise ValueError("payload must be object")
        operation = payload.get("operation")
        if operation not in {"add", "remove", "reload"}:
            raise ValueError("payload.operation must be add, remove, or reload")
        _require_str(payload, "target")
        _require_str(payload, "status")
        _require_bool(payload, "persisted")
        return
    if event_type == "turn_completed":
        stop_reason = raw.get("stop_reason")
        if stop_reason not in {
            "end_turn",
            "tool_use",
            "max_tokens",
            "stop_sequence",
            "content_filter",
            "cancelled",
        }:
            raise ValueError("stop_reason must be known")
    if event_type == "budget_warning" and raw.get("budget_type") not in {
        "tokens",
        "time",
        "tool_calls",
    }:
        raise ValueError("budget_type must be known")

    for field_name in required.get(event_type, ()):
        if field_name == "usage":
            _parse_usage(raw.get("usage"))
        elif field_name == "prompt":
            if "prompt" not in raw:
                raise ValueError("prompt is required")
        elif field_name == "skills":
            skills = raw.get("skills")
            if not isinstance(skills, list) or not all(isinstance(skill, str) for skill in skills):
                raise ValueError("skills must be string list")
        elif field_name in {"patch", "envelope"}:
            if not isinstance(raw.get(field_name), dict):
                raise ValueError(f"{field_name} must be object")
        elif field_name in _STRING_FIELDS:
            _require_str(raw, field_name)
        elif field_name in _NUMBER_FIELDS:
            _require_number(raw, field_name)
        elif field_name in _BOOL_FIELDS:
            _require_bool(raw, field_name)


def parse_event(raw: dict[str, Any]) -> Event:
    """Parse a raw event dict (from the wire) into a typed :class:`Event`.

    Unknown event types are returned as :class:`UnknownEvent` for
    forward-compatibility with newer server versions.
    """
    if "event" in raw and ("scope_id" in raw or "scope_path" in raw):
        inner_raw = raw.get("event")
        inner = parse_event(inner_raw) if isinstance(inner_raw, dict) else UnknownEvent()
        scope_path = raw.get("scope_path", [])
        if isinstance(scope_path, list):
            scope_path = [
                {
                    key: value
                    for key, value in frame.items()
                    if key not in {"agent_runtime_id", "fence_token", "generation"}
                }
                if isinstance(frame, dict)
                else frame
                for frame in scope_path
            ]
        return ScopedEvent(
            scope_id=str(raw.get("scope_id", "")),
            scope_path=list(scope_path) if isinstance(scope_path, list) else [],
            event=inner,
        )

    event_type = raw.get("type", "")
    cls = _EVENT_MAP.get(event_type)
    if cls is None:
        return UnknownEvent(type=event_type, data=raw)

    try:
        _validate_known_event(event_type, raw)
        # Build kwargs, injecting parsed Usage where needed
        kwargs: dict[str, Any] = {}
        for f in cls.__dataclass_fields__:
            if f == "usage":
                kwargs["usage"] = _parse_usage(raw.get("usage"))
            elif f == "is_error" and cls in {ToolResultReceived, ToolExecutionCompleted}:
                kwargs["is_error"] = _parse_optional_bool(raw.get("is_error"))
            elif f == "duration_ms" and cls is ToolExecutionCompleted:
                kwargs["duration_ms"] = _parse_optional_int(raw.get("duration_ms"))
            elif f == "skill_key" and cls is SkillResolutionFailed:
                kwargs["skill_key"] = _parse_skill_key(raw.get("skill_key"))
            elif f == "reason" and cls is SkillResolutionFailed:
                kwargs["reason"] = _parse_skill_resolution_failure_reason(
                    raw.get("reason"),
                    raw["error"],
                )
            elif f == "payload" and cls is ToolConfigChanged:
                payload_raw = raw["payload"]
                assert isinstance(payload_raw, dict)
                applied_at_turn_raw = payload_raw.get("applied_at_turn")
                kwargs["payload"] = ToolConfigChangedPayload(
                    operation=payload_raw["operation"],
                    target=payload_raw["target"],
                    status=payload_raw["status"],
                    status_info=_parse_tool_config_change_status(
                        payload_raw.get("status_info")
                    ),
                    persisted=payload_raw["persisted"],
                    applied_at_turn=(
                        applied_at_turn_raw
                        if isinstance(applied_at_turn_raw, int)
                        else None
                    ),
                )
            elif f in raw:
                kwargs[f] = raw[f]
        return cls(**kwargs)
    except (AssertionError, KeyError, TypeError, ValueError):
        return _malformed(raw, "malformed known event")
