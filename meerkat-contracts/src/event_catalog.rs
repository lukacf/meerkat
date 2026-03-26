/// Canonical documented agent event type inventory for emitted contracts and SDK generation.
///
/// This is a contracts-owned published catalog derived from `meerkat-core` event semantics.
/// It is intentionally not re-exported from `meerkat-core`, which owns the semantic event enum
/// and discriminator mapping but not surface-facing inventory projections.
pub const KNOWN_AGENT_EVENT_TYPES: &[&str] = &[
    "run_started",
    "run_completed",
    "run_failed",
    "hook_started",
    "hook_completed",
    "hook_failed",
    "hook_denied",
    "hook_rewrite_applied",
    "hook_patch_published",
    "turn_started",
    "reasoning_delta",
    "reasoning_complete",
    "text_delta",
    "text_complete",
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
    "interaction_failed",
    "stream_truncated",
    "tool_config_changed",
];
