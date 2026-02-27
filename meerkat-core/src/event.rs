//! Agent events for streaming output
//!
//! These events form the streaming API for consumers.

use crate::hooks::{HookPatch, HookPatchEnvelope, HookPoint, HookReasonCode};
use crate::time_compat::SystemTime;
use crate::types::{SessionId, StopReason, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Ordering;

/// Canonical event envelope for stream transport and ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope<T> {
    pub event_id: uuid::Uuid,
    pub source_id: String,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_id: Option<String>,
    pub timestamp_ms: u64,
    pub payload: T,
}

impl<T> EventEnvelope<T> {
    /// Create a new envelope with a UUIDv7 id and current wall-clock timestamp.
    pub fn new(source_id: impl Into<String>, seq: u64, mob_id: Option<String>, payload: T) -> Self {
        let timestamp_ms = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(duration) => duration.as_millis() as u64,
            Err(_) => u64::MAX,
        };
        Self {
            event_id: uuid::Uuid::now_v7(),
            source_id: source_id.into(),
            seq,
            mob_id,
            timestamp_ms,
            payload,
        }
    }
}

/// Canonical serialized event kind for SSE/RPC discriminators.
pub fn agent_event_type(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::RunStarted { .. } => "run_started",
        AgentEvent::RunCompleted { .. } => "run_completed",
        AgentEvent::RunFailed { .. } => "run_failed",
        AgentEvent::HookStarted { .. } => "hook_started",
        AgentEvent::HookCompleted { .. } => "hook_completed",
        AgentEvent::HookFailed { .. } => "hook_failed",
        AgentEvent::HookDenied { .. } => "hook_denied",
        AgentEvent::HookRewriteApplied { .. } => "hook_rewrite_applied",
        AgentEvent::HookPatchPublished { .. } => "hook_patch_published",
        AgentEvent::TurnStarted { .. } => "turn_started",
        AgentEvent::ReasoningDelta { .. } => "reasoning_delta",
        AgentEvent::ReasoningComplete { .. } => "reasoning_complete",
        AgentEvent::TextDelta { .. } => "text_delta",
        AgentEvent::TextComplete { .. } => "text_complete",
        AgentEvent::ToolCallRequested { .. } => "tool_call_requested",
        AgentEvent::ToolResultReceived { .. } => "tool_result_received",
        AgentEvent::TurnCompleted { .. } => "turn_completed",
        AgentEvent::ToolExecutionStarted { .. } => "tool_execution_started",
        AgentEvent::ToolExecutionCompleted { .. } => "tool_execution_completed",
        AgentEvent::ToolExecutionTimedOut { .. } => "tool_execution_timed_out",
        AgentEvent::CompactionStarted { .. } => "compaction_started",
        AgentEvent::CompactionCompleted { .. } => "compaction_completed",
        AgentEvent::CompactionFailed { .. } => "compaction_failed",
        AgentEvent::BudgetWarning { .. } => "budget_warning",
        AgentEvent::Retrying { .. } => "retrying",
        AgentEvent::SkillsResolved { .. } => "skills_resolved",
        AgentEvent::SkillResolutionFailed { .. } => "skill_resolution_failed",
        AgentEvent::InteractionComplete { .. } => "interaction_complete",
        AgentEvent::InteractionFailed { .. } => "interaction_failed",
        AgentEvent::StreamTruncated { .. } => "stream_truncated",
        AgentEvent::ToolConfigChanged { .. } => "tool_config_changed",
    }
}

/// Deterministic total ordering comparator for event envelopes.
pub fn compare_event_envelopes<T>(a: &EventEnvelope<T>, b: &EventEnvelope<T>) -> Ordering {
    a.timestamp_ms
        .cmp(&b.timestamp_ms)
        .then_with(|| a.source_id.cmp(&b.source_id))
        .then_with(|| a.seq.cmp(&b.seq))
        .then_with(|| a.event_id.cmp(&b.event_id))
}

/// Payload for tool configuration change notifications.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolConfigChangedPayload {
    pub operation: ToolConfigChangeOperation,
    pub target: String,
    pub status: String,
    pub persisted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_at_turn: Option<u32>,
}

/// Operation kind for live tool configuration changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolConfigChangeOperation {
    Add,
    Remove,
    Reload,
}

/// Events emitted during agent execution
///
/// These events form the streaming API for consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentEvent {
    // === Session Lifecycle ===
    /// Agent run started
    RunStarted {
        session_id: SessionId,
        prompt: String,
    },

    /// Agent run completed successfully
    RunCompleted {
        session_id: SessionId,
        result: String,
        usage: Usage,
    },

    /// Agent run failed
    RunFailed {
        session_id: SessionId,
        error: String,
    },

    // === Hook Lifecycle ===
    /// Hook invocation started.
    HookStarted { hook_id: String, point: HookPoint },

    /// Hook invocation completed.
    HookCompleted {
        hook_id: String,
        point: HookPoint,
        duration_ms: u64,
    },

    /// Hook invocation failed.
    HookFailed {
        hook_id: String,
        point: HookPoint,
        error: String,
    },

    /// Hook denied an action.
    HookDenied {
        hook_id: String,
        point: HookPoint,
        reason_code: HookReasonCode,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
    },

    /// A rewrite patch was applied synchronously.
    HookRewriteApplied {
        hook_id: String,
        point: HookPoint,
        patch: HookPatch,
    },

    /// A background patch was published for downstream surfaces.
    HookPatchPublished {
        hook_id: String,
        point: HookPoint,
        envelope: HookPatchEnvelope,
    },

    // === LLM Interaction ===
    /// New turn started (calling LLM)
    TurnStarted { turn_number: u32 },

    /// Streaming reasoning/thinking from the model
    ReasoningDelta { delta: String },

    /// Reasoning/thinking complete for this block
    ReasoningComplete { content: String },

    /// Streaming text from the model
    TextDelta { delta: String },

    /// Text generation complete for this turn
    TextComplete { content: String },

    /// Model requested a tool call
    ToolCallRequested {
        id: String,
        name: String,
        args: Value,
    },

    /// Tool result received (injected into conversation)
    ToolResultReceived {
        id: String,
        name: String,
        is_error: bool,
    },

    /// Turn completed
    TurnCompleted {
        stop_reason: StopReason,
        usage: Usage,
    },

    // === Tool Execution ===
    /// Starting tool execution
    ToolExecutionStarted { id: String, name: String },

    /// Tool execution completed
    ToolExecutionCompleted {
        id: String,
        name: String,
        result: String,
        is_error: bool,
        duration_ms: u64,
    },

    /// Tool execution timed out
    ToolExecutionTimedOut {
        id: String,
        name: String,
        timeout_ms: u64,
    },

    // === Compaction ===
    /// Context compaction started.
    CompactionStarted {
        /// Input tokens from the last LLM call that triggered compaction.
        input_tokens: u64,
        /// Estimated total history tokens before compaction.
        estimated_history_tokens: u64,
        /// Number of messages before compaction.
        message_count: usize,
    },

    /// Context compaction completed successfully.
    CompactionCompleted {
        /// Tokens consumed by the summary.
        summary_tokens: u64,
        /// Messages before compaction.
        messages_before: usize,
        /// Messages after compaction.
        messages_after: usize,
    },

    /// Context compaction failed (non-fatal — agent continues with uncompacted history).
    CompactionFailed { error: String },

    // === Budget ===
    /// Budget warning (approaching limits)
    BudgetWarning {
        budget_type: BudgetType,
        used: u64,
        limit: u64,
        percent: f32,
    },

    // === Retry Events ===
    /// Retrying after error
    Retrying {
        attempt: u32,
        max_attempts: u32,
        error: String,
        delay_ms: u64,
    },

    // === Skill Events ===
    /// Skills resolved for this turn.
    SkillsResolved {
        skills: Vec<crate::skills::SkillId>,
        injection_bytes: usize,
    },

    /// A skill reference could not be resolved.
    SkillResolutionFailed { reference: String, error: String },

    // === Interaction-Scoped Streaming ===
    /// An interaction completed successfully (terminal event for tap subscribers).
    InteractionComplete {
        interaction_id: crate::interaction::InteractionId,
        result: String,
    },

    /// An interaction failed (terminal event for tap subscribers).
    InteractionFailed {
        interaction_id: crate::interaction::InteractionId,
        error: String,
    },

    /// Some streaming events were dropped due to channel backpressure.
    /// Best-effort marker — the terminal event is authoritative.
    StreamTruncated { reason: String },

    /// Live tool configuration changed for this session.
    ToolConfigChanged { payload: ToolConfigChangedPayload },
}

/// Scope attribution frame for multi-agent streaming.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "scope", rename_all = "snake_case")]
#[non_exhaustive]
pub enum StreamScopeFrame {
    /// Top-level primary session scope.
    Primary { session_id: String },
    /// Mob member scope for flow dispatch turns.
    MobMember {
        flow_run_id: String,
        member_ref: String,
        session_id: String,
    },
    /// Sub-agent scope nested under a parent scope.
    SubAgent {
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}

/// Attributed stream event wrapper for multi-agent streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopedAgentEvent {
    pub scope_id: String,
    pub scope_path: Vec<StreamScopeFrame>,
    pub event: AgentEvent,
}

impl ScopedAgentEvent {
    /// Build a scoped event from a scope path and payload event.
    pub fn new(scope_path: Vec<StreamScopeFrame>, event: AgentEvent) -> Self {
        let scope_id = Self::scope_id_from_path(&scope_path);
        Self {
            scope_id,
            scope_path,
            event,
        }
    }

    /// Build a primary-scoped event for a top-level session event.
    pub fn primary(session_id: impl Into<String>, event: AgentEvent) -> Self {
        Self::new(
            vec![StreamScopeFrame::Primary {
                session_id: session_id.into(),
            }],
            event,
        )
    }

    /// Convenience alias for converting a legacy event into primary scope.
    pub fn from_agent_event_primary(session_id: impl Into<String>, event: AgentEvent) -> Self {
        Self::primary(session_id, event)
    }

    /// Append one scope frame and recompute scope_id deterministically.
    pub fn append_scope(mut self, frame: StreamScopeFrame) -> Self {
        self.scope_path.push(frame);
        self.scope_id = Self::scope_id_from_path(&self.scope_path);
        self
    }

    /// Deterministic canonical selector from scope path.
    ///
    /// Formats:
    /// - `primary`
    /// - `mob:<member_ref>`
    /// - `primary/sub:<agent_id>`
    /// - `mob:<member_ref>/sub:<agent_id>`
    pub fn scope_id_from_path(path: &[StreamScopeFrame]) -> String {
        if path.is_empty() {
            return "primary".to_string();
        }
        let mut segments: Vec<String> = Vec::with_capacity(path.len());
        for frame in path {
            match frame {
                StreamScopeFrame::Primary { .. } => segments.push("primary".to_string()),
                StreamScopeFrame::MobMember { member_ref, .. } => {
                    segments.push(format!("mob:{member_ref}"));
                }
                StreamScopeFrame::SubAgent { agent_id, .. } => {
                    segments.push(format!("sub:{agent_id}"));
                }
            }
        }
        segments.join("/")
    }
}

/// Type of budget being tracked
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BudgetType {
    Tokens,
    Time,
    ToolCalls,
}

/// Configuration for formatting verbose event output.
#[derive(Debug, Clone, Copy)]
pub struct VerboseEventConfig {
    pub max_tool_args_bytes: usize,
    pub max_tool_result_bytes: usize,
    pub max_text_bytes: usize,
}

impl Default for VerboseEventConfig {
    fn default() -> Self {
        Self {
            max_tool_args_bytes: 100,
            max_tool_result_bytes: 200,
            max_text_bytes: 500,
        }
    }
}

/// Format an agent event using default verbose formatting rules.
pub fn format_verbose_event(event: &AgentEvent) -> Option<String> {
    format_verbose_event_with_config(event, &VerboseEventConfig::default())
}

/// Format an agent event using custom verbose formatting rules.
pub fn format_verbose_event_with_config(
    event: &AgentEvent,
    config: &VerboseEventConfig,
) -> Option<String> {
    match event {
        AgentEvent::TurnStarted { turn_number } => {
            Some(format!("\n━━━ Turn {} ━━━", turn_number + 1))
        }
        AgentEvent::ToolCallRequested { name, args, .. } => {
            let args_str = serde_json::to_string(args).unwrap_or_default();
            let args_preview = truncate_preview(&args_str, config.max_tool_args_bytes);
            Some(format!("  → Calling tool: {name} {args_preview}"))
        }
        AgentEvent::ToolExecutionCompleted {
            name,
            result,
            is_error,
            duration_ms,
            ..
        } => {
            let status = if *is_error { "✗" } else { "✓" };
            let result_preview = truncate_preview(result, config.max_tool_result_bytes);
            Some(format!(
                "  {status} {name} ({duration_ms}ms): {result_preview}"
            ))
        }
        AgentEvent::TurnCompleted { stop_reason, usage } => Some(format!(
            "  ── Turn complete: {:?} ({} in / {} out tokens)",
            stop_reason, usage.input_tokens, usage.output_tokens
        )),
        AgentEvent::TextComplete { content } => {
            if content.is_empty() {
                None
            } else {
                let preview = truncate_preview(content, config.max_text_bytes);
                Some(format!("  💬 Response: {preview}"))
            }
        }
        AgentEvent::ReasoningComplete { content } => {
            if content.is_empty() {
                None
            } else {
                let preview = truncate_preview(content, config.max_text_bytes);
                Some(format!("  💭 Thinking: {preview}"))
            }
        }
        AgentEvent::Retrying {
            attempt,
            max_attempts,
            error,
            delay_ms,
        } => Some(format!(
            "  ⟳ Retry {attempt}/{max_attempts}: {error} (waiting {delay_ms}ms)"
        )),
        AgentEvent::BudgetWarning {
            budget_type,
            used,
            limit,
            percent,
        } => Some(format!(
            "  ⚠ Budget warning: {:?} at {:.0}% ({}/{})",
            budget_type,
            percent * 100.0,
            used,
            limit
        )),
        AgentEvent::CompactionStarted {
            input_tokens,
            estimated_history_tokens,
            message_count,
        } => Some(format!(
            "  ⟳ Compaction started: {input_tokens} input tokens, ~{estimated_history_tokens} history tokens, {message_count} messages"
        )),
        AgentEvent::CompactionCompleted {
            summary_tokens,
            messages_before,
            messages_after,
        } => Some(format!(
            "  ✓ Compaction complete: {messages_before} → {messages_after} messages, {summary_tokens} summary tokens"
        )),
        AgentEvent::CompactionFailed { error } => {
            Some(format!("  ✗ Compaction failed (continuing): {error}"))
        }
        _ => None,
    }
}

fn truncate_preview(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }
    format!("{}...", truncate_str(input, max_bytes))
}

fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let truncate_at = s
        .char_indices()
        .take_while(|(i, _)| *i < max_bytes)
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    &s[..truncate_at]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_event_json_schema() {
        // Test all event variants serialize correctly
        let events = vec![
            AgentEvent::RunStarted {
                session_id: SessionId::new(),
                prompt: "Hello".to_string(),
            },
            AgentEvent::TextDelta {
                delta: "chunk".to_string(),
            },
            AgentEvent::TurnStarted { turn_number: 1 },
            AgentEvent::TurnCompleted {
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
            AgentEvent::ToolCallRequested {
                id: "tc_1".to_string(),
                name: "read_file".to_string(),
                args: serde_json::json!({"path": "/tmp/test"}),
            },
            AgentEvent::ToolResultReceived {
                id: "tc_1".to_string(),
                name: "read_file".to_string(),
                is_error: false,
            },
            AgentEvent::BudgetWarning {
                budget_type: BudgetType::Tokens,
                used: 8000,
                limit: 10000,
                percent: 0.8,
            },
            AgentEvent::Retrying {
                attempt: 1,
                max_attempts: 3,
                error: "Rate limited".to_string(),
                delay_ms: 1000,
            },
            AgentEvent::RunCompleted {
                session_id: SessionId::new(),
                result: "Done".to_string(),
                usage: Usage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            },
            AgentEvent::RunFailed {
                session_id: SessionId::new(),
                error: "Budget exceeded".to_string(),
            },
            AgentEvent::CompactionStarted {
                input_tokens: 120_000,
                estimated_history_tokens: 150_000,
                message_count: 42,
            },
            AgentEvent::CompactionCompleted {
                summary_tokens: 2048,
                messages_before: 42,
                messages_after: 8,
            },
            AgentEvent::CompactionFailed {
                error: "LLM request failed".to_string(),
            },
            AgentEvent::InteractionComplete {
                interaction_id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
                result: "agent response".to_string(),
            },
            AgentEvent::InteractionFailed {
                interaction_id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
                error: "LLM failure".to_string(),
            },
            AgentEvent::StreamTruncated {
                reason: "channel full".to_string(),
            },
            AgentEvent::ToolConfigChanged {
                payload: ToolConfigChangedPayload {
                    operation: ToolConfigChangeOperation::Remove,
                    target: "filesystem".to_string(),
                    status: "staged".to_string(),
                    persisted: false,
                    applied_at_turn: Some(12),
                },
            },
        ];

        for event in events {
            let json = serde_json::to_value(&event).unwrap();

            // All events should have a "type" field
            assert!(
                json.get("type").is_some(),
                "Event missing type field: {event:?}"
            );

            // Should roundtrip
            let roundtrip: AgentEvent = serde_json::from_value(json.clone()).unwrap();
            let json2 = serde_json::to_value(&roundtrip).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn test_budget_type_serialization() {
        assert_eq!(serde_json::to_value(BudgetType::Tokens).unwrap(), "tokens");
        assert_eq!(serde_json::to_value(BudgetType::Time).unwrap(), "time");
        assert_eq!(
            serde_json::to_value(BudgetType::ToolCalls).unwrap(),
            "tool_calls"
        );
    }

    #[test]
    fn test_scoped_agent_event_roundtrip() {
        let event = ScopedAgentEvent::new(
            vec![
                StreamScopeFrame::MobMember {
                    flow_run_id: "run_123".to_string(),
                    member_ref: "writer".to_string(),
                    session_id: "sid_1".to_string(),
                },
                StreamScopeFrame::SubAgent {
                    agent_id: "op_abc".to_string(),
                    tool_call_id: Some("tool_1".to_string()),
                    label: Some("fork-op_abc".to_string()),
                },
            ],
            AgentEvent::TextDelta {
                delta: "hello".to_string(),
            },
        );

        assert_eq!(event.scope_id, "mob:writer/sub:op_abc");

        let json = serde_json::to_value(&event).unwrap();
        let roundtrip: ScopedAgentEvent = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip.scope_id, "mob:writer/sub:op_abc");
        assert!(matches!(
            roundtrip.event,
            AgentEvent::TextDelta { ref delta } if delta == "hello"
        ));
    }

    #[test]
    fn test_scope_id_from_path_formats() {
        let primary = vec![StreamScopeFrame::Primary {
            session_id: "sid_x".to_string(),
        }];
        assert_eq!(ScopedAgentEvent::scope_id_from_path(&primary), "primary");

        let primary_sub = vec![
            StreamScopeFrame::Primary {
                session_id: "sid_x".to_string(),
            },
            StreamScopeFrame::SubAgent {
                agent_id: "op_1".to_string(),
                tool_call_id: None,
                label: None,
            },
        ];
        assert_eq!(
            ScopedAgentEvent::scope_id_from_path(&primary_sub),
            "primary/sub:op_1"
        );

        let mob_sub = vec![
            StreamScopeFrame::MobMember {
                flow_run_id: "run_1".to_string(),
                member_ref: "planner".to_string(),
                session_id: "sid_m".to_string(),
            },
            StreamScopeFrame::SubAgent {
                agent_id: "op_2".to_string(),
                tool_call_id: None,
                label: None,
            },
        ];
        assert_eq!(
            ScopedAgentEvent::scope_id_from_path(&mob_sub),
            "mob:planner/sub:op_2"
        );
    }

    #[test]
    fn test_event_envelope_roundtrip() {
        let envelope = EventEnvelope::new(
            "session:sid_test",
            7,
            Some("mob_1".to_string()),
            AgentEvent::TextDelta {
                delta: "hello".to_string(),
            },
        );
        let value = serde_json::to_value(&envelope).expect("serialize envelope");
        let parsed: EventEnvelope<AgentEvent> =
            serde_json::from_value(value).expect("deserialize envelope");
        assert_eq!(parsed.source_id, "session:sid_test");
        assert_eq!(parsed.seq, 7);
        assert_eq!(parsed.mob_id.as_deref(), Some("mob_1"));
        assert!(parsed.timestamp_ms > 0);
        assert!(matches!(
            parsed.payload,
            AgentEvent::TextDelta { delta } if delta == "hello"
        ));
    }

    #[test]
    fn test_compare_event_envelopes_total_order() {
        let mut a = EventEnvelope::new("a", 1, None, AgentEvent::TurnStarted { turn_number: 1 });
        let mut b = EventEnvelope::new("a", 2, None, AgentEvent::TurnStarted { turn_number: 2 });
        a.timestamp_ms = 10;
        b.timestamp_ms = 10;
        assert_eq!(compare_event_envelopes(&a, &b), Ordering::Less);
        assert_eq!(compare_event_envelopes(&b, &a), Ordering::Greater);
    }
}
