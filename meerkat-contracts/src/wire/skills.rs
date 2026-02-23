//! Wire types for skill introspection.

use serde::{Deserialize, Serialize};

/// Wire representation of a skill entry (for list responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillEntry {
    /// Canonical skill ID (e.g. "extraction/email").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Short description.
    pub description: String,
    /// Scope: "builtin", "project", or "user".
    pub scope: String,
    /// Source repository name (e.g. "embedded", "project").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Whether this skill is active (not shadowed by a higher-precedence source).
    pub is_active: bool,
    /// If shadowed, the name of the higher-precedence source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadowed_by: Option<String>,
}

/// Wire response for listing skills with introspection data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillListResponse {
    pub skills: Vec<SkillEntry>,
}

/// Wire response for inspecting a single skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillInspectResponse {
    /// Canonical skill ID.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Short description.
    pub description: String,
    /// Scope: "builtin", "project", or "user".
    pub scope: String,
    /// Source repository name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Full skill body (markdown content).
    pub body: String,
}
