//! Skill system contracts for Meerkat.
//!
//! Defines the core types, traits, and errors for the skill system.
//! The `meerkat-skills` crate provides the implementations.

use std::borrow::Cow;

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Skill identifier — newtype for type safety.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillId(pub String);

impl std::fmt::Display for SkillId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a skill was discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(strum::EnumString, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SkillScope {
    /// Embedded in a component crate.
    Builtin,
    /// Project-level: `.rkat/skills/`.
    Project,
    /// User-level: `~/.rkat/skills/`.
    User,
}

/// Metadata describing a skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDescriptor {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub scope: SkillScope,
    /// Capability IDs required for this skill (as string forms of CapabilityId).
    /// Using strings here avoids circular dependency between meerkat-core and
    /// meerkat-contracts. The `meerkat-skills` crate resolves these to typed
    /// `CapabilityId` values.
    pub requires_capabilities: Vec<String>,
}

/// A loaded skill with its full content.
#[derive(Debug, Clone)]
pub struct SkillDocument {
    pub descriptor: SkillDescriptor,
    pub body: String,
    pub extensions: IndexMap<String, String>,
}

/// Errors from skill operations.
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill not found: {id}")]
    NotFound { id: SkillId },

    #[error("skill requires unavailable capability: {capability}")]
    CapabilityUnavailable { id: SkillId, capability: String },

    #[error("ambiguous skill reference '{reference}' matches: {matches:?}")]
    Ambiguous {
        reference: String,
        matches: Vec<SkillId>,
    },

    #[error("skill loading failed: {0}")]
    Load(Cow<'static, str>),

    #[error("skill parse failed: {0}")]
    Parse(Cow<'static, str>),
}

/// Source of skill definitions.
#[async_trait]
pub trait SkillSource: Send + Sync {
    /// List available skill descriptors.
    async fn list(&self) -> Result<Vec<SkillDescriptor>, SkillError>;
    /// Load a skill document by ID.
    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError>;
}

/// Engine that manages skill resolution and rendering.
#[async_trait]
pub trait SkillEngine: Send + Sync {
    /// Generate the skill inventory section for the system prompt.
    async fn inventory_section(&self) -> Result<String, SkillError>;
    /// Resolve skill references and render injection content.
    async fn resolve_and_render(
        &self,
        references: &[String],
        available_capabilities: &[String],
    ) -> Result<String, SkillError>;
}
