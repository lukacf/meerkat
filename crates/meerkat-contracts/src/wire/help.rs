//! Help surface wire types.

use serde::{Deserialize, Serialize};

use super::WireRunResult;

/// Execution posture requested by a help caller.
///
/// Today all help surfaces are advisory. `plan_execution` is a forward-compatible
/// hook for callers that want an executable plan without permitting Meerkat to
/// perform the execution yet.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum HelpExecutionMode {
    #[default]
    ExplainOnly,
    PlanExecution,
}

/// Request body for dedicated help surfaces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HelpRequest {
    /// The user question about Meerkat usage.
    pub question: String,
    /// Optional inert prompt payload for future execution-oriented help.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Advisory execution posture. Defaults to explanation only.
    #[serde(default)]
    pub execution_mode: HelpExecutionMode,
    /// Optional model override for the help session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional provider override for the help session (`anthropic`, `openai`,
    /// `gemini`, or `self_hosted`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Optional max-token override for the help session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

/// Dedicated help surfaces return the same run result as other session starts.
pub type HelpResponse = WireRunResult;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_request_defaults_to_explain_only() -> serde_json::Result<()> {
        let request: HelpRequest =
            serde_json::from_str(r#"{"question":"How do I add an MCP server?"}"#)?;
        assert_eq!(request.execution_mode, HelpExecutionMode::ExplainOnly);
        assert!(request.prompt.is_none());
        Ok(())
    }
}
