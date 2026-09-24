//! Wire types for skill introspection.

use meerkat_core::skills::{SkillKey, SkillScope, SourceIdentityRecord};
use serde::{Deserialize, Serialize};

/// Typed source provenance for a skill entry. `display_name` is presentation
/// metadata only; `source_uuid` is the stable identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillSourceProvenance {
    #[serde(flatten)]
    pub identity: SourceIdentityRecord,
}

/// Wire representation of a skill entry (for list responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillEntry {
    /// Canonical skill identity.
    pub key: SkillKey,
    /// Human-readable name.
    pub name: String,
    /// Short description.
    pub description: String,
    /// Typed discovery scope (`builtin`, `project`, or `user`).
    pub scope: SkillScope,
    /// Typed source provenance.
    pub source: SkillSourceProvenance,
    /// Whether this skill is active (not shadowed by a higher-precedence source).
    pub is_active: bool,
    /// If shadowed, the higher-precedence source identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadowed_by: Option<SkillSourceProvenance>,
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
    /// Canonical skill identity.
    pub key: SkillKey,
    /// Human-readable name.
    pub name: String,
    /// Short description.
    pub description: String,
    /// Typed discovery scope (`builtin`, `project`, or `user`).
    pub scope: SkillScope,
    /// Typed source provenance.
    pub source: SkillSourceProvenance,
    /// Full skill body (markdown content).
    pub body: String,
}
