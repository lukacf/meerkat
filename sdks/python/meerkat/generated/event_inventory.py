"""Generated canonical event-type inventory for the Meerkat Python SDK.

Source: artifacts/schemas/events.json (WireEvent.known_event_types)

Single source of truth for the fail-closed event parser: a wire frame
whose ``type`` is not in this set is a contract violation for a
version-matched client and must be rejected, not coerced.
"""

from __future__ import annotations

KNOWN_AGENT_EVENT_TYPES: frozenset[str] = frozenset({
    "run_started",
    "run_completed",
    "extraction_succeeded",
    "extraction_failed",
    "run_failed",
    "hook_started",
    "hook_completed",
    "hook_failed",
    "hook_denied",
    "turn_started",
    "reasoning_delta",
    "reasoning_complete",
    "text_delta",
    "text_complete",
    "assistant_image_appended",
    "tool_call_requested",
    "tool_result_received",
    "turn_completed",
    "tool_execution_started",
    "tool_execution_completed",
    "tool_execution_timed_out",
    "compaction_started",
    "compaction_completed",
    "compaction_failed",
    "budget_warning",
    "retrying",
    "skills_resolved",
    "skill_resolution_failed",
    "interaction_complete",
    "interaction_callback_pending",
    "interaction_failed",
    "stream_truncated",
    "tool_config_changed",
    "background_job_completed",
    "transcript_rewrite_committed",
})


def is_known_agent_event_type(event_type: str) -> bool:
    """True iff ``event_type`` is a schema-known agent event discriminant."""
    return event_type in KNOWN_AGENT_EVENT_TYPES
