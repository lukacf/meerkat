"""Compaction diagnostics preserve the runtime's tagged reason object."""

from copy import deepcopy

import pytest

from meerkat import MeerkatClient
from meerkat.events import CompactionFailed, ScopedEvent, UnknownEvent, parse_event


REASONS = [
    {"kind": "empty_summary"},
    {"kind": "llm_failed", "error_class": "llm_failed", "message": "summary provider unavailable"},
    {"kind": "transcript_rewrite_failed", "message": "invalid rebuild"},
    {"kind": "future_reason", "details": {"retained": [1, 2]}},
]


@pytest.mark.parametrize("reason", REASONS)
def test_compaction_failure_preserves_tagged_reason(reason):
    raw = {"type": "compaction_failed", "reason": deepcopy(reason)}
    parsed = parse_event(raw)
    assert isinstance(parsed, CompactionFailed)
    assert parsed.reason == reason

    scoped = parse_event({"scope_id": "primary", "event": deepcopy(raw)})
    assert isinstance(scoped, ScopedEvent)
    assert isinstance(scoped.event, CompactionFailed)
    assert scoped.event.reason == reason

    envelope = MeerkatClient._parse_agent_event_envelope({
        "event_id": "00000000-0000-4000-8000-000000000010",
        "source": {"type": "session", "session_id": "00000000-0000-4000-8000-000000000001"},
        "seq": 7,
        "timestamp_ms": 1,
        "payload": deepcopy(raw),
    })
    assert isinstance(envelope.payload, CompactionFailed)
    assert envelope.payload.reason == reason


@pytest.mark.parametrize("reason", ["empty_summary", None, [], {}, {"kind": 3}, {"kind": None}])
def test_compaction_failure_rejects_malformed_reason(reason):
    parsed = parse_event({"type": "compaction_failed", "reason": reason})
    assert isinstance(parsed, UnknownEvent)
    assert parsed.type == "malformed_event"


def test_compaction_failure_requires_reason():
    parsed = parse_event({"type": "compaction_failed"})
    assert isinstance(parsed, UnknownEvent)
    assert parsed.type == "malformed_event"
