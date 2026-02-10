//! Composable request fragments.
//!
//! Protocol crates inline the fields they support and provide accessor
//! methods returning the fragment type. No `#[serde(flatten)]` —
//! explicit delegation to avoid known serde/schemars issues.

use serde::{Deserialize, Serialize};

use meerkat_core::{HookRunOverrides, OutputSchema, Provider};

/// Core session creation parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CoreCreateParams {
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

/// Structured output parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredOutputParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<OutputSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output_retries: Option<u32>,
}

/// Comms parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CommsParams {
    #[serde(default)]
    pub host_mode: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comms_name: Option<String>,
}

/// Hook parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HookParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hooks_override: Option<HookRunOverrides>,
}

/// Skills parameters (Phase 3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillsParams {
    #[serde(default)]
    pub skills_enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_references: Vec<String>,
}
