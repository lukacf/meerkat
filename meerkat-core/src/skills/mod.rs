//! Skill system contracts for Meerkat.
//!
//! Defines the core types, traits, and errors for the skill system.
//! The `meerkat-skills` crate provides the implementations.

use std::borrow::Cow;
use std::collections::BTreeMap;

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Skill ID
// ---------------------------------------------------------------------------

/// Skill identifier — newtype for type safety.
///
/// The canonical format is a slash-delimited path: `{collection-path}/{name}`.
/// Examples: `"extraction/email-extractor"`, `"a/b/c"`, `"pdf-processing"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillId(pub String);

impl SkillId {
    /// Extract the collection path (everything before the last `/`).
    ///
    /// Returns `None` for root-level skills (no `/` in the ID).
    pub fn collection(&self) -> Option<&str> {
        self.0.rfind('/').map(|pos| &self.0[..pos])
    }

    /// Extract the spec-compliant flat name (last path segment).
    pub fn skill_name(&self) -> &str {
        match self.0.rfind('/') {
            Some(pos) => &self.0[pos + 1..],
            None => &self.0,
        }
    }
}

impl std::fmt::Display for SkillId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SkillId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SkillId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

/// Where a skill was discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(strum::EnumString, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SkillScope {
    /// Embedded in a component crate.
    #[default]
    Builtin,
    /// Project-level: `.rkat/skills/`.
    Project,
    /// User-level: `~/.rkat/skills/`.
    User,
}

// ---------------------------------------------------------------------------
// Descriptor
// ---------------------------------------------------------------------------

/// Metadata describing a skill.
///
/// This is the **single authoritative definition**. The `source_name` field is
/// set by `CompositeSkillSource` when merging named sources. Individual
/// `SkillSource` implementations leave it empty.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillDescriptor {
    /// Canonical namespaced ID: `"extraction/email-extractor"`.
    /// This is the ONLY identifier used across all layers.
    pub id: SkillId,
    /// Human-readable name from SKILL.md frontmatter (e.g. `"email-extractor"`).
    /// Typically matches the last segment of the ID but is independently set.
    pub name: String,
    pub description: String,
    pub scope: SkillScope,
    /// Capability IDs required for this skill (as string forms of CapabilityId).
    pub requires_capabilities: Vec<String>,
    /// Extensible metadata (from SKILL.md frontmatter `metadata:` field).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub metadata: IndexMap<String, String>,
    /// Repository name this skill came from (e.g. "company", "project").
    /// Populated by `CompositeSkillSource` from the `NamedSource` wrapper.
    /// Empty string for sources used outside `CompositeSkillSource`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_name: String,
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// A loaded skill with its full content.
#[derive(Debug, Clone)]
pub struct SkillDocument {
    pub descriptor: SkillDescriptor,
    pub body: String,
    pub extensions: IndexMap<String, String>,
}

// ---------------------------------------------------------------------------
// Filter & Collection
// ---------------------------------------------------------------------------

/// Filter for listing skills.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillFilter {
    /// Segment-aware recursive prefix filter: return all skills whose
    /// collection path starts with this value at a `/` boundary.
    ///
    /// `"extraction"` matches `extraction/email`, `extraction/medical/x`
    /// but NOT `extract/something` or `extractions/foo`.
    ///
    /// Implementation: match when skill's collection path == filter
    /// OR skill's collection path starts with `"{filter}/"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    /// Free-text search across name + description (all collections).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

/// A skill collection (derived from namespaced IDs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCollection {
    /// Collection path prefix (e.g. `"extraction"` or `"extraction/medical"`).
    pub path: String,
    /// Human-readable description.
    pub description: String,
    /// Number of skills in this collection (recursive — includes subcollections).
    pub count: usize,
}

/// A resolved skill ready for injection.
#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    pub id: SkillId,
    pub name: String,
    /// The rendered `<skill>` XML block, sanitized and size-limited.
    pub rendered_body: String,
    pub byte_size: usize,
}

// ---------------------------------------------------------------------------
// Collection derivation
// ---------------------------------------------------------------------------

/// Check whether a skill's collection path matches a prefix filter at
/// segment boundaries.
///
/// `"extraction"` matches skills with collection `"extraction"`,
/// `"extraction/medical"`, etc. but NOT `"extract"` or `"extractions"`.
pub fn collection_matches_prefix(skill_collection: Option<&str>, prefix: &str) -> bool {
    match skill_collection {
        None => false,
        Some(coll) => {
            coll == prefix
                || (coll.starts_with(prefix)
                    && coll.as_bytes().get(prefix.len()) == Some(&b'/'))
        }
    }
}

/// Derive top-level collections from a list of skill descriptors.
///
/// Returns unique top-level collection paths with their recursive skill counts.
/// Skills without a collection (root-level) are not included.
pub fn derive_collections(skills: &[SkillDescriptor]) -> Vec<SkillCollection> {
    // Count skills per top-level collection prefix.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for skill in skills {
        if let Some(coll) = skill.id.collection() {
            // Extract top-level: first segment only
            let top = match coll.find('/') {
                Some(pos) => &coll[..pos],
                None => coll,
            };
            *counts.entry(top.to_string()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .map(|(path, count)| SkillCollection {
            description: format!("{count} skills"),
            path,
            count,
        })
        .collect()
}

/// Apply a `SkillFilter` to a slice of descriptors.
pub fn apply_filter(skills: &[SkillDescriptor], filter: &SkillFilter) -> Vec<SkillDescriptor> {
    let mut result: Vec<SkillDescriptor> = skills.to_vec();

    if let Some(ref prefix) = filter.collection {
        result.retain(|s| collection_matches_prefix(s.id.collection(), prefix));
    }

    if let Some(ref query) = filter.query {
        let q = query.to_lowercase();
        result.retain(|s| {
            s.name.to_lowercase().contains(&q) || s.description.to_lowercase().contains(&q)
        });
    }

    result
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Source of skill definitions.
#[async_trait]
pub trait SkillSource: Send + Sync {
    /// List skill descriptors, optionally filtered by collection prefix or query.
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError>;

    /// Load a skill document by its canonical ID.
    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError>;

    /// List top-level collections with counts.
    /// Default implementation derives collections from skill ID prefixes.
    async fn collections(&self) -> Result<Vec<SkillCollection>, SkillError> {
        let all = self.list(&SkillFilter::default()).await?;
        Ok(derive_collections(&all))
    }
}

/// Engine that manages skill resolution and rendering.
#[async_trait]
pub trait SkillEngine: Send + Sync {
    /// Generate the system prompt inventory (XML, compact).
    async fn inventory_section(&self) -> Result<String, SkillError>;

    /// Resolve skill IDs and render injection content.
    async fn resolve_and_render(
        &self,
        ids: &[SkillId],
    ) -> Result<Vec<ResolvedSkill>, SkillError>;

    /// List collections (delegates to source).
    async fn collections(&self) -> Result<Vec<SkillCollection>, SkillError>;

    /// List skills with optional filter (for browse_skills tool).
    async fn list_skills(
        &self,
        filter: &SkillFilter,
    ) -> Result<Vec<SkillDescriptor>, SkillError>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- SkillId ---

    #[test]
    fn test_skill_id_collection_extraction() {
        let id = SkillId("extraction/email".into());
        assert_eq!(id.collection(), Some("extraction"));
    }

    #[test]
    fn test_skill_id_nested_collection() {
        let id = SkillId("a/b/c".into());
        assert_eq!(id.collection(), Some("a/b"));
    }

    #[test]
    fn test_skill_id_root_level() {
        let id = SkillId("pdf".into());
        assert_eq!(id.collection(), None);
    }

    #[test]
    fn test_skill_id_name_extraction() {
        let id = SkillId("extraction/email".into());
        assert_eq!(id.skill_name(), "email");

        let root = SkillId("pdf-processing".into());
        assert_eq!(root.skill_name(), "pdf-processing");

        let nested = SkillId("a/b/c".into());
        assert_eq!(nested.skill_name(), "c");
    }

    // --- SkillFilter ---

    #[test]
    fn test_skill_filter_default_is_empty() {
        let filter = SkillFilter::default();
        assert!(filter.collection.is_none());
        assert!(filter.query.is_none());
    }

    // --- derive_collections ---

    #[test]
    fn test_derive_collections_basic() {
        let skills = vec![
            SkillDescriptor {
                id: SkillId("extraction/email".into()),
                ..Default::default()
            },
            SkillDescriptor {
                id: SkillId("extraction/fiction".into()),
                ..Default::default()
            },
            SkillDescriptor {
                id: SkillId("formatting/markdown".into()),
                ..Default::default()
            },
        ];

        let collections = derive_collections(&skills);
        assert_eq!(collections.len(), 2);

        let extraction = collections.iter().find(|c| c.path == "extraction");
        assert!(extraction.is_some());
        assert_eq!(extraction.map(|c| c.count), Some(2));

        let formatting = collections.iter().find(|c| c.path == "formatting");
        assert!(formatting.is_some());
        assert_eq!(formatting.map(|c| c.count), Some(1));
    }

    #[test]
    fn test_derive_collections_nested() {
        let skills = vec![
            SkillDescriptor {
                id: SkillId("extraction/email".into()),
                ..Default::default()
            },
            SkillDescriptor {
                id: SkillId("extraction/medical/diagnosis".into()),
                ..Default::default()
            },
            SkillDescriptor {
                id: SkillId("extraction/medical/imaging/ct".into()),
                ..Default::default()
            },
        ];

        let collections = derive_collections(&skills);
        // All nest under top-level "extraction"
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].path, "extraction");
        assert_eq!(collections[0].count, 3);
    }

    #[test]
    fn test_derive_collections_empty() {
        let collections = derive_collections(&[]);
        assert!(collections.is_empty());

        // Root-level skills don't create collections
        let skills = vec![SkillDescriptor {
            id: SkillId("pdf-processing".into()),
            ..Default::default()
        }];
        let collections = derive_collections(&skills);
        assert!(collections.is_empty());
    }

    // --- collection_matches_prefix (segment-aware) ---

    #[test]
    fn test_collection_prefix_match_segment() {
        // "extraction" matches "extraction" and "extraction/medical" but not "extract"
        assert!(collection_matches_prefix(Some("extraction"), "extraction"));
        assert!(collection_matches_prefix(
            Some("extraction/medical"),
            "extraction"
        ));
        assert!(!collection_matches_prefix(Some("extract"), "extraction"));
        assert!(!collection_matches_prefix(
            Some("extractions"),
            "extraction"
        ));
        assert!(!collection_matches_prefix(None, "extraction"));
    }

    // --- apply_filter ---

    #[test]
    fn test_apply_filter_collection() {
        let skills = vec![
            SkillDescriptor {
                id: SkillId("extraction/email".into()),
                name: "email".into(),
                ..Default::default()
            },
            SkillDescriptor {
                id: SkillId("formatting/md".into()),
                name: "md".into(),
                ..Default::default()
            },
        ];

        let filtered = apply_filter(
            &skills,
            &SkillFilter {
                collection: Some("extraction".into()),
                ..Default::default()
            },
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id.0, "extraction/email");
    }

    #[test]
    fn test_apply_filter_query() {
        let skills = vec![
            SkillDescriptor {
                id: SkillId("a/email".into()),
                name: "email".into(),
                description: "Extract from emails".into(),
                ..Default::default()
            },
            SkillDescriptor {
                id: SkillId("b/fiction".into()),
                name: "fiction".into(),
                description: "Extract from fiction".into(),
                ..Default::default()
            },
        ];

        let filtered = apply_filter(
            &skills,
            &SkillFilter {
                query: Some("email".into()),
                ..Default::default()
            },
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "email");
    }
}
