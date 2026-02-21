//! Wire run result type.

use serde::{Deserialize, Serialize};

use super::WireUsage;
use meerkat_core::{RunResult, SchemaWarning, SessionId};

/// Canonical run result for wire protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireRunResult {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub text: String,
    pub turns: u32,
    pub tool_calls: u32,
    pub usage: WireUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
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
            structured_output: r.structured_output,
            schema_warnings: r.schema_warnings,
            skill_diagnostics: r.skill_diagnostics,
        }
    }
}
