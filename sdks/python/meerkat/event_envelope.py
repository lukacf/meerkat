"""Strict parsing for canonical agent-event envelopes."""

from __future__ import annotations

from typing import Any
from uuid import UUID

from .errors import MeerkatError
from .events import parse_event
from .types import EventEnvelope, EventSourceIdentity

MAX_U64 = (1 << 64) - 1


def _invalid(reason: str, raw: Any) -> MeerkatError:
    return MeerkatError(
        "INVALID_RESPONSE",
        f"Invalid agent event envelope: {reason}",
        details=raw,
    )


def _require_record(raw: Any, field: str, envelope: Any) -> dict[str, Any]:
    if not isinstance(raw, dict):
        raise _invalid(f"{field} must be an object", envelope)
    return raw


def _require_non_empty_string(raw: dict[str, Any], field: str, envelope: Any) -> str:
    value = raw.get(field)
    if not isinstance(value, str) or not value:
        raise _invalid(f"{field} must be a non-empty string", envelope)
    return value


def _require_uuid(raw: dict[str, Any], field: str, envelope: Any) -> str:
    value = _require_non_empty_string(raw, field, envelope)
    try:
        parsed = UUID(value)
    except ValueError as error:
        raise _invalid(f"{field} must be a canonical UUID", envelope) from error
    if str(parsed) != value:
        raise _invalid(f"{field} must be a canonical UUID", envelope)
    return value


def _require_unsigned_integer(raw: dict[str, Any], field: str, envelope: Any) -> int:
    value = raw.get(field)
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or value < 0
        or value > MAX_U64
    ):
        raise _invalid(f"{field} must be an unsigned 64-bit integer", envelope)
    return value


def parse_event_source_identity(raw: Any, envelope: Any) -> EventSourceIdentity:
    """Parse the required discriminated event-source identity."""

    record = _require_record(raw, "source", envelope)
    source_type = _require_non_empty_string(record, "type", envelope)
    if source_type == "session":
        return EventSourceIdentity(
            type=source_type,
            session_id=_require_uuid(record, "session_id", envelope),
        )
    if source_type == "runtime":
        return EventSourceIdentity(
            type=source_type,
            runtime_id=_require_non_empty_string(record, "runtime_id", envelope),
        )
    if source_type == "interaction":
        return EventSourceIdentity(
            type=source_type,
            interaction_id=_require_uuid(record, "interaction_id", envelope),
        )
    if source_type == "callback":
        return EventSourceIdentity(type=source_type)
    if source_type == "external":
        return EventSourceIdentity(
            type=source_type,
            source_id=_require_non_empty_string(record, "source_id", envelope),
        )
    raise _invalid(f"source.type has unsupported value {source_type!r}", envelope)


def parse_agent_event_envelope(raw: Any) -> EventEnvelope:
    """Parse a full wire envelope without fabricating identity/order facts."""

    record = _require_record(raw, "envelope", raw)
    payload = _require_record(record.get("payload"), "payload", raw)
    mob_id: str | None = None
    if record.get("mob_id") is not None:
        mob_id = _require_non_empty_string(record, "mob_id", raw)
    return EventEnvelope(
        event_id=_require_uuid(record, "event_id", raw),
        source=parse_event_source_identity(record.get("source"), raw),
        seq=_require_unsigned_integer(record, "seq", raw),
        timestamp_ms=_require_unsigned_integer(record, "timestamp_ms", raw),
        payload=parse_event(payload),
        mob_id=mob_id,
    )
