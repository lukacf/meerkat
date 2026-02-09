//! Skills configuration.

use serde::{Deserialize, Serialize};

/// Skills configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    /// Whether skills are enabled.
    pub enabled: bool,
    /// Maximum injection content size in bytes.
    pub max_injection_bytes: usize,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_injection_bytes: 32 * 1024,
        }
    }
}

/// How skill references are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(strum::EnumString, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SkillResolutionMode {
    /// Only explicit `/skill-id` or exact name matches.
    Explicit,
}
