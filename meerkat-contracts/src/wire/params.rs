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

/// Skills parameters for session/turn requests.
///
/// `preload_skills`: Pre-load these skills at session creation.
/// `None` or empty = inventory-only mode (no pre-loading).
/// `Some([])` is normalized to `None` to prevent silent misconfiguration.
///
/// `skill_references`: Skill IDs to resolve and inject for this turn.
/// `None` or empty = no per-turn skill injection.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preload_skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_references: Option<Vec<String>>,
}

impl SkillsParams {
    /// Normalize: `Some([])` → `None` for both fields.
    pub fn normalize(&mut self) {
        if let Some(ref v) = self.preload_skills {
            if v.is_empty() {
                self.preload_skills = None;
            }
        }
        if let Some(ref v) = self.skill_references {
            if v.is_empty() {
                self.skill_references = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skills_params_none_serde() {
        let params = SkillsParams {
            preload_skills: None,
            skill_references: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        assert_eq!(json, "{}");

        let parsed: SkillsParams = serde_json::from_str("{}").unwrap();
        assert!(parsed.preload_skills.is_none());
        assert!(parsed.skill_references.is_none());
    }

    #[test]
    fn test_skills_params_empty_normalizes() {
        let mut params = SkillsParams {
            preload_skills: Some(vec![]),
            skill_references: Some(vec![]),
        };
        params.normalize();
        assert!(params.preload_skills.is_none());
        assert!(params.skill_references.is_none());
    }

    #[test]
    fn test_skills_params_with_ids() {
        let params = SkillsParams {
            preload_skills: Some(vec!["a/b".into()]),
            skill_references: Some(vec!["c/d".into()]),
        };
        let json = serde_json::to_string(&params).unwrap();
        let parsed: SkillsParams = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.preload_skills, Some(vec!["a/b".to_string()]));
        assert_eq!(parsed.skill_references, Some(vec!["c/d".to_string()]));
    }
}
