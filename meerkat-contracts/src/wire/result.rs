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
            structured_output: None,
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
}
