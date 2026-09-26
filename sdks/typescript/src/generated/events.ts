// Generated event-type inventory for the Meerkat TypeScript SDK
// Source: artifacts/schemas/events.json (WireEvent.known_event_types)
//
// This is the single source of truth for the fail-closed event parser.
// A wire frame whose `type` is not in this set is a contract violation
// for a version-matched client and must be rejected, not coerced.

export const KNOWN_AGENT_EVENT_TYPES = [
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
  "server_tool_content",
  "turn_completed",
  "turn_usage_accounting_unmeasured",
  "turn_usage_accounting_identity_disputed",
  "tool_execution_started",
  "tool_execution_completed",
  "tool_execution_timed_out",
  "compaction_started",
  "compaction_completed",
  "compaction_failed",
  "budget_warning",
  "retrying",
  "model_fallback_staged",
  "model_fallback_committed",
  "model_fallback_skipped",
  "model_fallback_target_failed",
  "skills_resolved",
  "skill_resolution_failed",
  "interaction_complete",
  "interaction_callback_pending",
  "interaction_failed",
  "stream_truncated",
  "tool_config_changed",
  "background_job_completed",
  "transcript_rewrite_committed",
  "transcript_rewrite_audit_receipt_committed",
  "peer_content_ingested",
  "provider_cache_breakpoints_discarded",
  "boundary_append_applied",
  "boundary_appends_discarded"
] as const;

export type KnownAgentEventType = typeof KNOWN_AGENT_EVENT_TYPES[number];

/** True iff `type` is a schema-known agent event discriminant. */
export function isKnownAgentEventType(type: string): type is KnownAgentEventType {
  return (KNOWN_AGENT_EVENT_TYPES as readonly string[]).includes(type);
}
