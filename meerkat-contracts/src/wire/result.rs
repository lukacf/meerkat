//! Wire run result type.

use serde::{Deserialize, Serialize};

use super::WireUsage;
use meerkat_core::{ExtractionError, RunResult, SchemaWarning, SessionId, TurnTerminalCauseKind};

/// Typed tool-error classification carried on the wire.
///
/// Added in Wave B (V7). The classification is distinct from `NotFound` so
/// that policy-denied tool calls surface as `AccessDenied` end-to-end —
/// SDK clients, REST handlers, and RPC callers can map `AccessDenied` to
/// HTTP 403 and `NotFound` to HTTP 404 without sniffing message strings.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[allow(dead_code)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireToolErrorClass {
    NotFound,
    AccessDenied,
    InvalidArguments,
    Timeout,
    Internal,
}

impl WireToolErrorClass {
    /// Stable wire string used across REST/RPC error payloads.
    #[allow(dead_code)]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::AccessDenied => "access_denied",
            Self::InvalidArguments => "invalid_arguments",
            Self::Timeout => "timeout",
            Self::Internal => "internal",
        }
    }
}

impl From<&meerkat_core::error::ToolError> for WireToolErrorClass {
    fn from(value: &meerkat_core::error::ToolError) -> Self {
        use meerkat_core::error::ToolError;
        match value {
            ToolError::NotFound { .. } => Self::NotFound,
            ToolError::AccessDenied { .. } => Self::AccessDenied,
            ToolError::InvalidArguments { .. } => Self::InvalidArguments,
            ToolError::Timeout { .. } => Self::Timeout,
            _ => Self::Internal,
        }
    }
}

/// Canonical run result for wire protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRunResult {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub text: String,
    pub turns: u32,
    pub tool_calls: u32,
    pub usage: WireUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_cause_kind: Option<TurnTerminalCauseKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extraction_error: Option<ExtractionError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_warnings: Option<Vec<SchemaWarning>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_diagnostics: Option<meerkat_core::skills::SkillRuntimeDiagnostics>,
}

impl From<RunResult> for WireRunResult {
    fn from(r: RunResult) -> Self {
        Self {
            session_id: r.session_id,
            session_ref: None,
            text: r.text,
            turns: r.turns,
            tool_calls: r.tool_calls,
            usage: r.usage.into(),
            terminal_cause_kind: r.terminal_cause_kind,
            structured_output: r.structured_output,
            extraction_error: r.extraction_error,
            schema_warnings: r.schema_warnings,
            skill_diagnostics: r.skill_diagnostics,
        }
    }
}

/// A single external tool call the agent is blocked on while
/// [`WireCallbackPending`] is in effect.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WirePendingToolCall {
    /// Provider-issued tool-use identifier that the callback result must
    /// reference. This is a required top-level protocol field; it is never
    /// inferred from or injected into `args`.
    pub tool_use_id: String,
    /// Name of the external tool the agent is waiting on.
    pub tool_name: String,
    /// Raw arguments the agent passed to the tool.
    #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
    pub args: serde_json::Value,
}

/// Canonical typed contract for a turn that suspended on external tool
/// callbacks (`AgentError::CallbackPending`).
///
/// Pending state is a terminal-control fact, not a successful run: the agent
/// is parked awaiting tool results that the caller must supply before
/// resuming. Surfaces (CLI, MCP) serialize this contract instead of
/// hand-building a success-looking JSON envelope, so the `status` discriminant
/// and pending-tool list are schema-emitted and consistent across surfaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireCallbackPending {
    /// Stable discriminant for callers that route on a flat status string.
    /// Always serializes to `"pending_tool_call"`.
    pub status: WireCallbackPendingStatus,
    /// Session the pending turn belongs to.
    pub session_id: SessionId,
    /// Resolved external session reference, when the surface assigns one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    /// Whether the session was created by the request that suspended.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub session_created: bool,
    /// Whether the session can be resumed once tool results are supplied.
    pub resumable: bool,
    /// The external tool calls the agent is blocked on.
    pub pending_tool_calls: Vec<WirePendingToolCall>,
}

impl WireCallbackPending {
    /// Build a pending contract for the complete blocked callback set.
    pub fn many(
        session_id: SessionId,
        session_ref: Option<String>,
        session_created: bool,
        resumable: bool,
        pending_tool_calls: Vec<WirePendingToolCall>,
    ) -> Self {
        Self {
            status: WireCallbackPendingStatus::PendingToolCall,
            session_id,
            session_ref,
            session_created,
            resumable,
            pending_tool_calls,
        }
    }

    /// Build a pending contract for a single blocked tool call.
    pub fn single(
        session_id: SessionId,
        session_ref: Option<String>,
        session_created: bool,
        resumable: bool,
        tool_use_id: String,
        tool_name: String,
        args: serde_json::Value,
    ) -> Self {
        Self::many(
            session_id,
            session_ref,
            session_created,
            resumable,
            vec![WirePendingToolCall {
                tool_use_id,
                tool_name,
                args,
            }],
        )
    }
}

/// Discriminant for [`WireCallbackPending`]. A dedicated enum so the wire
/// `status` field is a typed closed value rather than a free-form string.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireCallbackPendingStatus {
    PendingToolCall,
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::skills::{SkillRuntimeDiagnostics, SourceHealthSnapshot, SourceHealthState};

    #[test]
    fn wire_run_result_preserves_unhealthy_skill_diagnostics()
    -> Result<(), Box<dyn std::error::Error>> {
        let run = RunResult {
            text: "ok".to_string(),
            session_id: SessionId::new(),
            usage: Default::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: Some(SkillRuntimeDiagnostics {
                source_health: SourceHealthSnapshot {
                    state: SourceHealthState::Unhealthy,
                    invalid_ratio: 0.8,
                    invalid_count: 8,
                    total_count: 10,
                    failure_streak: 10,
                    handshake_failed: true,
                },
                quarantined: vec![],
                collection_fault: None,
            }),
        };

        let wire: WireRunResult = run.into();
        let state = wire
            .skill_diagnostics
            .as_ref()
            .map(|value| value.source_health.state);
        assert_eq!(state, Some(SourceHealthState::Unhealthy));

        let json = serde_json::to_value(&wire)?;
        assert_eq!(
            json["skill_diagnostics"]["source_health"]["state"],
            "unhealthy"
        );
        Ok(())
    }

    #[test]
    fn wire_pending_tool_call_requires_top_level_tool_use_id() -> Result<(), serde_json::Error> {
        let call = WirePendingToolCall {
            tool_use_id: "call-1".to_string(),
            tool_name: "external".to_string(),
            args: serde_json::json!({"question": "approve?"}),
        };
        let json = serde_json::to_value(&call)?;
        assert_eq!(json["tool_use_id"], "call-1");
        assert!(json["args"].get("tool_use_id").is_none());

        let missing = serde_json::json!({
            "tool_name": "external",
            "args": {"question": "approve?"}
        });
        assert!(
            serde_json::from_value::<WirePendingToolCall>(missing).is_err(),
            "tool_use_id is a required wire field"
        );
        Ok(())
    }

    #[cfg(feature = "schema")]
    #[test]
    fn wire_pending_tool_call_schema_requires_tool_use_id() -> Result<(), serde_json::Error> {
        let schema = serde_json::to_value(schemars::schema_for!(WirePendingToolCall))?;
        assert!(
            schema["required"]
                .as_array()
                .is_some_and(|required| required.iter().any(|field| field == "tool_use_id"))
        );
        Ok(())
    }
}
