//! Skill system contracts for Meerkat.
//!
//! Defines the core types, traits, and errors for the skill system.
//! The `meerkat-skills` crate provides the implementations.

use std::borrow::Cow;
use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

mod identity;
pub use identity::{SkillAlias, SourceIdentityRegistry};

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
// Canonical identity (Skills V2.1 Phase 0)
// ---------------------------------------------------------------------------

/// Canonical source identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "String", into = "String")]
pub struct SourceUuid(Uuid);

impl SourceUuid {
    pub fn parse(value: &str) -> Result<Self, SkillError> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|e| SkillError::Parse(format!("invalid source_uuid '{value}': {e}").into()))
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl TryFrom<String> for SourceUuid {
    type Error = SkillError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<SourceUuid> for String {
    fn from(value: SourceUuid) -> Self {
        value.0.hyphenated().to_string()
    }
}

impl std::fmt::Display for SourceUuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.hyphenated().to_string())
    }
}

/// Canonical skill slug (lowercase, dash-separated).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "String", into = "String")]
pub struct SkillName(String);

impl SkillName {
    pub fn parse(value: &str) -> Result<Self, SkillError> {
        if value.is_empty() {
            return Err(SkillError::Parse("skill_name cannot be empty".into()));
        }

        let bytes = value.as_bytes();
        let starts_or_ends_dash = bytes.first() == Some(&b'-') || bytes.last() == Some(&b'-');
        if starts_or_ends_dash {
            return Err(SkillError::Parse(
                format!("invalid skill_name '{value}': cannot start/end with '-'").into(),
            ));
        }

        if value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !value.contains("--")
        {
            return Ok(Self(value.to_string()));
        }

        Err(SkillError::Parse(
            format!("invalid skill_name '{value}': expected lowercase slug [a-z0-9-], no '--'")
                .into(),
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SkillName {
    type Error = SkillError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<SkillName> for String {
    fn from(value: SkillName) -> Self {
        value.0
    }
}

impl std::fmt::Display for SkillName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Canonical runtime identity for a skill.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillKey {
    pub source_uuid: SourceUuid,
    pub skill_name: SkillName,
}

/// Boundary compatibility reference: legacy string or structured key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum SkillRef {
    Legacy(String),
    Structured(SkillKey),
}

/// Source transport class used for identity governance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SourceTransportKind {
    Embedded,
    Filesystem,
    Git,
    Http,
    Stdio,
}

/// Source lifecycle status in the identity registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SourceIdentityStatus {
    Active,
    Disabled,
    Retired,
}

/// Identity registry record for a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SourceIdentityRecord {
    pub source_uuid: SourceUuid,
    pub display_name: String,
    pub transport_kind: SourceTransportKind,
    pub fingerprint: String,
    pub status: SourceIdentityStatus,
}

/// Lineage event for source identity governance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SourceIdentityLineageEvent {
    RenameOrRelocate {
        from: SourceUuid,
        to: SourceUuid,
    },
    Rotate {
        from: SourceUuid,
        to: SourceUuid,
    },
    Split {
        from: SourceUuid,
        into: Vec<SourceUuid>,
    },
    Merge {
        from: Vec<SourceUuid>,
        to: SourceUuid,
    },
}

/// Lineage record carrying provenance metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SourceIdentityLineage {
    pub event_id: String,
    pub recorded_at_unix_secs: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_from_skills: Vec<SkillName>,
    pub event: SourceIdentityLineageEvent,
}

/// Per-skill remap entry for lineage split/merge/rotate operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillKeyRemap {
    pub from: SkillKey,
    pub to: SkillKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Source health state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SourceHealthState {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Thresholds for source health transitions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SourceHealthThresholds {
    /// Degrade when invalid_ratio >= this value.
    pub degraded_invalid_ratio: f32,
    /// Unhealthy when invalid_ratio >= this value.
    pub unhealthy_invalid_ratio: f32,
    /// Degrade when failure streak >= this value.
    pub degraded_failure_streak: u32,
    /// Unhealthy when failure streak >= this value.
    pub unhealthy_failure_streak: u32,
}

impl Default for SourceHealthThresholds {
    fn default() -> Self {
        Self {
            degraded_invalid_ratio: 0.05,
            unhealthy_invalid_ratio: 0.40,
            degraded_failure_streak: 3,
            unhealthy_failure_streak: 10,
        }
    }
}

/// Source health snapshot reported by sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SourceHealthSnapshot {
    pub state: SourceHealthState,
    pub invalid_ratio: f32,
    pub invalid_count: u32,
    pub total_count: u32,
    pub failure_streak: u32,
    pub handshake_failed: bool,
}

impl Default for SourceHealthSnapshot {
    fn default() -> Self {
        Self {
            state: SourceHealthState::Healthy,
            invalid_ratio: 0.0,
            invalid_count: 0,
            total_count: 0,
            failure_streak: 0,
            handshake_failed: false,
        }
    }
}

/// Per-skill quarantine diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillQuarantineDiagnostic {
    pub source_uuid: SourceUuid,
    pub skill_id: SkillId,
    pub location: String,
    pub error_code: String,
    pub error_class: String,
    pub message: String,
    pub first_seen_unix_secs: u64,
    pub last_seen_unix_secs: u64,
}

/// Runtime-visible diagnostics for skill health and quarantined entries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillRuntimeDiagnostics {
    pub source_health: SourceHealthSnapshot,
    pub quarantined: Vec<SkillQuarantineDiagnostic>,
}

/// Determine health state from invalid ratio + failure streak + handshake state.
pub fn classify_source_health(
    invalid_ratio: f32,
    failure_streak: u32,
    handshake_failed: bool,
    thresholds: SourceHealthThresholds,
) -> SourceHealthState {
    if handshake_failed
        || invalid_ratio >= thresholds.unhealthy_invalid_ratio
        || failure_streak >= thresholds.unhealthy_failure_streak
    {
        SourceHealthState::Unhealthy
    } else if invalid_ratio >= thresholds.degraded_invalid_ratio
        || failure_streak >= thresholds.degraded_failure_streak
    {
        SourceHealthState::Degraded
    } else {
        SourceHealthState::Healthy
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

/// Artifact metadata for skill-hosted resources.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillArtifact {
    pub path: String,
    pub mime_type: String,
    pub byte_length: usize,
}

/// Artifact content payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillArtifactContent {
    pub path: String,
    pub mime_type: String,
    pub content: String,
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
                || (coll.starts_with(prefix) && coll.as_bytes().get(prefix.len()) == Some(&b'/'))
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
            description: if count == 1 {
                "1 skill".to_string()
            } else {
                format!("{count} skills")
            },
            path,
            count,
        })
        .collect()
}

/// Apply a `SkillFilter` to a slice of descriptors.
///
/// Filters by iterating once instead of cloning the entire slice upfront.
pub fn apply_filter(skills: &[SkillDescriptor], filter: &SkillFilter) -> Vec<SkillDescriptor> {
    let query_lower = filter.query.as_ref().map(|q| q.to_lowercase());

    skills
        .iter()
        .filter(|s| {
            if let Some(ref prefix) = filter.collection
                && !collection_matches_prefix(s.id.collection(), prefix)
            {
                return false;
            }
            if let Some(ref q) = query_lower
                && !s.name.to_lowercase().contains(q)
                && !s.description.to_lowercase().contains(q)
            {
                return false;
            }
            true
        })
        .cloned()
        .collect()
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

    #[error(
        "source UUID collision for {source_uuid}: existing fingerprint '{existing_fingerprint}' conflicts with '{new_fingerprint}'"
    )]
    SourceUuidCollision {
        source_uuid: String,
        existing_fingerprint: String,
        new_fingerprint: String,
    },

    #[error(
        "source UUID mutation rejected for fingerprint '{fingerprint}': {existing_source_uuid} -> {mutated_source_uuid} without lineage"
    )]
    SourceUuidMutationWithoutLineage {
        fingerprint: String,
        existing_source_uuid: String,
        mutated_source_uuid: String,
    },

    #[error("lineage event '{event_id}' ({event_kind}) requires explicit per-skill remap entries")]
    MissingSkillRemaps {
        event_id: String,
        event_kind: &'static str,
    },

    #[error(
        "skill remap from {from_source_uuid}/{from_skill_name} to {to_source_uuid}/{to_skill_name} is not allowed by lineage"
    )]
    RemapWithoutLineage {
        from_source_uuid: String,
        from_skill_name: String,
        to_source_uuid: String,
        to_skill_name: String,
    },

    #[error("invalid legacy skill reference '{reference}': expected '<source_uuid>/<skill_name>'")]
    InvalidLegacySkillRefFormat { reference: String },

    #[error("unknown skill alias '{alias}'")]
    UnknownSkillAlias { alias: String },

    #[error("skill remap cycle detected for {source_uuid}/{skill_name}")]
    RemapCycle {
        source_uuid: String,
        skill_name: String,
    },
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Source of skill definitions.
pub trait SkillSource: Send + Sync {
    /// List skill descriptors, optionally filtered by collection prefix or query.
    fn list(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send;

    /// Load a skill document by its canonical ID.
    fn load(&self, id: &SkillId) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send;

    /// List top-level collections with counts.
    /// Default implementation derives collections from skill ID prefixes.
    fn collections(&self) -> impl Future<Output = Result<Vec<SkillCollection>, SkillError>> + Send {
        async {
            let all = self.list(&SkillFilter::default()).await?;
            Ok(derive_collections(&all))
        }
    }

    /// Report per-skill quarantine diagnostics for invalid skills.
    fn quarantined_diagnostics(
        &self,
    ) -> impl Future<Output = Result<Vec<SkillQuarantineDiagnostic>, SkillError>> + Send {
        async { Ok(Vec::new()) }
    }

    /// Report source health snapshot for operator surfaces.
    fn health_snapshot(
        &self,
    ) -> impl Future<Output = Result<SourceHealthSnapshot, SkillError>> + Send {
        async { Ok(SourceHealthSnapshot::default()) }
    }

    /// List resources/artifacts exposed by a skill.
    fn list_artifacts(
        &self,
        id: &SkillId,
    ) -> impl Future<Output = Result<Vec<SkillArtifact>, SkillError>> + Send {
        let missing = id.clone();
        async move { Err(SkillError::NotFound { id: missing }) }
    }

    /// Read a specific artifact from a skill.
    fn read_artifact(
        &self,
        id: &SkillId,
        artifact_path: &str,
    ) -> impl Future<Output = Result<SkillArtifactContent, SkillError>> + Send {
        let missing = id.clone();
        let _ = artifact_path;
        async move { Err(SkillError::NotFound { id: missing }) }
    }

    /// Invoke a skill-defined function with structured arguments.
    fn invoke_function(
        &self,
        id: &SkillId,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send {
        let missing = id.clone();
        let _ = function_name;
        let _ = arguments;
        async move { Err(SkillError::NotFound { id: missing }) }
    }
}

#[allow(clippy::manual_async_fn)]
impl<T> SkillSource for Arc<T>
where
    T: SkillSource + ?Sized,
{
    fn list(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send {
        async move { (**self).list(filter).await }
    }

    fn load(&self, id: &SkillId) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send {
        async move { (**self).load(id).await }
    }

    fn collections(&self) -> impl Future<Output = Result<Vec<SkillCollection>, SkillError>> + Send {
        async move { (**self).collections().await }
    }

    fn quarantined_diagnostics(
        &self,
    ) -> impl Future<Output = Result<Vec<SkillQuarantineDiagnostic>, SkillError>> + Send {
        async move { (**self).quarantined_diagnostics().await }
    }

    fn health_snapshot(
        &self,
    ) -> impl Future<Output = Result<SourceHealthSnapshot, SkillError>> + Send {
        async move { (**self).health_snapshot().await }
    }

    fn list_artifacts(
        &self,
        id: &SkillId,
    ) -> impl Future<Output = Result<Vec<SkillArtifact>, SkillError>> + Send {
        async move { (**self).list_artifacts(id).await }
    }

    fn read_artifact(
        &self,
        id: &SkillId,
        artifact_path: &str,
    ) -> impl Future<Output = Result<SkillArtifactContent, SkillError>> + Send {
        async move { (**self).read_artifact(id, artifact_path).await }
    }

    fn invoke_function(
        &self,
        id: &SkillId,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send {
        async move { (**self).invoke_function(id, function_name, arguments).await }
    }
}

/// Engine that manages skill resolution and rendering.
pub trait SkillEngine: Send + Sync {
    /// Generate the system prompt inventory (XML, compact).
    fn inventory_section(&self) -> impl Future<Output = Result<String, SkillError>> + Send;

    /// Resolve skill IDs and render injection content.
    fn resolve_and_render(
        &self,
        ids: &[SkillId],
    ) -> impl Future<Output = Result<Vec<ResolvedSkill>, SkillError>> + Send;

    /// List collections (delegates to source).
    fn collections(&self) -> impl Future<Output = Result<Vec<SkillCollection>, SkillError>> + Send;

    /// List skills with optional filter (for browse_skills tool).
    fn list_skills(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send;

    /// Report per-skill quarantine diagnostics for invalid skills.
    fn quarantined_diagnostics(
        &self,
    ) -> impl Future<Output = Result<Vec<SkillQuarantineDiagnostic>, SkillError>> + Send;

    /// Report source health snapshot for operator surfaces.
    fn health_snapshot(
        &self,
    ) -> impl Future<Output = Result<SourceHealthSnapshot, SkillError>> + Send;

    /// List resources/artifacts exposed by a skill.
    fn list_artifacts(
        &self,
        id: &SkillId,
    ) -> impl Future<Output = Result<Vec<SkillArtifact>, SkillError>> + Send;

    /// Read a specific artifact from a skill.
    fn read_artifact(
        &self,
        id: &SkillId,
        artifact_path: &str,
    ) -> impl Future<Output = Result<SkillArtifactContent, SkillError>> + Send;

    /// Invoke a skill-defined function with structured arguments.
    fn invoke_function(
        &self,
        id: &SkillId,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send;
}

type OwnedSkillFuture<T> = Pin<Box<dyn Future<Output = Result<T, SkillError>> + Send + 'static>>;
type InventoryFn = dyn Fn() -> OwnedSkillFuture<String> + Send + Sync;
type ResolveFn = dyn Fn(Vec<SkillId>) -> OwnedSkillFuture<Vec<ResolvedSkill>> + Send + Sync;
type CollectionsFn = dyn Fn() -> OwnedSkillFuture<Vec<SkillCollection>> + Send + Sync;
type ListSkillsFn = dyn Fn(SkillFilter) -> OwnedSkillFuture<Vec<SkillDescriptor>> + Send + Sync;
type QuarantinedDiagnosticsFn =
    dyn Fn() -> OwnedSkillFuture<Vec<SkillQuarantineDiagnostic>> + Send + Sync;
type HealthSnapshotFn = dyn Fn() -> OwnedSkillFuture<SourceHealthSnapshot> + Send + Sync;
type ListArtifactsFn = dyn Fn(SkillId) -> OwnedSkillFuture<Vec<SkillArtifact>> + Send + Sync;
type ReadArtifactFn =
    dyn Fn(SkillId, String) -> OwnedSkillFuture<SkillArtifactContent> + Send + Sync;
type InvokeFunctionFn =
    dyn Fn(SkillId, String, serde_json::Value) -> OwnedSkillFuture<serde_json::Value> + Send + Sync;

#[derive(Clone)]
pub struct SkillRuntime {
    inventory_fn: Arc<InventoryFn>,
    resolve_fn: Arc<ResolveFn>,
    collections_fn: Arc<CollectionsFn>,
    list_skills_fn: Arc<ListSkillsFn>,
    quarantined_diagnostics_fn: Arc<QuarantinedDiagnosticsFn>,
    health_snapshot_fn: Arc<HealthSnapshotFn>,
    list_artifacts_fn: Arc<ListArtifactsFn>,
    read_artifact_fn: Arc<ReadArtifactFn>,
    invoke_function_fn: Arc<InvokeFunctionFn>,
}

impl SkillRuntime {
    pub fn new<E>(engine: Arc<E>) -> Self
    where
        E: SkillEngine + Send + Sync + 'static,
    {
        let inventory_engine = Arc::clone(&engine);
        let resolve_engine = Arc::clone(&engine);
        let collections_engine = Arc::clone(&engine);
        let list_engine = Arc::clone(&engine);
        let quarantined_engine = Arc::clone(&engine);
        let health_engine = Arc::clone(&engine);
        let list_artifacts_engine = Arc::clone(&engine);
        let read_artifact_engine = Arc::clone(&engine);
        let invoke_function_engine = engine;

        Self {
            inventory_fn: Arc::new(move || {
                let engine = Arc::clone(&inventory_engine);
                Box::pin(async move { engine.inventory_section().await })
            }),
            resolve_fn: Arc::new(move |ids: Vec<SkillId>| {
                let engine = Arc::clone(&resolve_engine);
                Box::pin(async move { engine.resolve_and_render(&ids).await })
            }),
            collections_fn: Arc::new(move || {
                let engine = Arc::clone(&collections_engine);
                Box::pin(async move { engine.collections().await })
            }),
            list_skills_fn: Arc::new(move |filter: SkillFilter| {
                let engine = Arc::clone(&list_engine);
                Box::pin(async move { engine.list_skills(&filter).await })
            }),
            quarantined_diagnostics_fn: Arc::new(move || {
                let engine = Arc::clone(&quarantined_engine);
                Box::pin(async move { engine.quarantined_diagnostics().await })
            }),
            health_snapshot_fn: Arc::new(move || {
                let engine = Arc::clone(&health_engine);
                Box::pin(async move { engine.health_snapshot().await })
            }),
            list_artifacts_fn: Arc::new(move |id: SkillId| {
                let engine = Arc::clone(&list_artifacts_engine);
                Box::pin(async move { engine.list_artifacts(&id).await })
            }),
            read_artifact_fn: Arc::new(move |id: SkillId, artifact_path: String| {
                let engine = Arc::clone(&read_artifact_engine);
                Box::pin(async move { engine.read_artifact(&id, &artifact_path).await })
            }),
            invoke_function_fn: Arc::new(
                move |id: SkillId, function_name: String, arguments: serde_json::Value| {
                    let engine = Arc::clone(&invoke_function_engine);
                    Box::pin(
                        async move { engine.invoke_function(&id, &function_name, arguments).await },
                    )
                },
            ),
        }
    }

    pub async fn inventory_section(&self) -> Result<String, SkillError> {
        (self.inventory_fn)().await
    }

    pub async fn resolve_and_render(
        &self,
        ids: &[SkillId],
    ) -> Result<Vec<ResolvedSkill>, SkillError> {
        (self.resolve_fn)(ids.to_vec()).await
    }

    pub async fn collections(&self) -> Result<Vec<SkillCollection>, SkillError> {
        (self.collections_fn)().await
    }

    pub async fn list_skills(
        &self,
        filter: &SkillFilter,
    ) -> Result<Vec<SkillDescriptor>, SkillError> {
        (self.list_skills_fn)(filter.clone()).await
    }

    pub async fn quarantined_diagnostics(
        &self,
    ) -> Result<Vec<SkillQuarantineDiagnostic>, SkillError> {
        (self.quarantined_diagnostics_fn)().await
    }

    pub async fn health_snapshot(&self) -> Result<SourceHealthSnapshot, SkillError> {
        (self.health_snapshot_fn)().await
    }

    pub async fn list_artifacts(&self, id: &SkillId) -> Result<Vec<SkillArtifact>, SkillError> {
        (self.list_artifacts_fn)(id.clone()).await
    }

    pub async fn read_artifact(
        &self,
        id: &SkillId,
        artifact_path: &str,
    ) -> Result<SkillArtifactContent, SkillError> {
        (self.read_artifact_fn)(id.clone(), artifact_path.to_string()).await
    }

    pub async fn invoke_function(
        &self,
        id: &SkillId,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, SkillError> {
        (self.invoke_function_fn)(id.clone(), function_name.to_string(), arguments).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used)]
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

    #[test]
    fn test_source_uuid_json_roundtrip_stable() {
        let raw = "dc256086-0d2f-4f61-a307-320d4148107f";
        let parsed = SourceUuid::parse(raw).expect("valid uuid should parse");
        let json = serde_json::to_string(&parsed).expect("source uuid should serialize");
        assert_eq!(json, format!("\"{raw}\""));

        let decoded: SourceUuid =
            serde_json::from_str(&json).expect("source uuid should deserialize");
        assert_eq!(decoded, parsed);
    }

    #[test]
    fn test_skill_name_slug_validation() {
        assert!(SkillName::parse("email-extractor").is_ok());
        assert!(SkillName::parse("EmailExtractor").is_err());
        assert!(SkillName::parse("email_extractor").is_err());
    }

    #[test]
    fn test_skill_key_json_roundtrip_stable() {
        let key = SkillKey {
            source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                .expect("valid source uuid"),
            skill_name: SkillName::parse("email-extractor").expect("valid skill slug"),
        };

        let json = serde_json::to_string(&key).expect("skill key should serialize");
        let decoded: SkillKey = serde_json::from_str(&json).expect("skill key should deserialize");
        assert_eq!(decoded, key);
    }

    #[test]
    fn test_skill_ref_boundary_legacy_and_structured_equivalence() {
        let legacy = SkillRef::Legacy("extraction/email".to_string());
        let structured = SkillRef::Structured(SkillKey {
            source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                .expect("valid source uuid"),
            skill_name: SkillName::parse("email-extractor").expect("valid skill slug"),
        });

        let legacy_json = serde_json::to_string(&legacy).expect("legacy ref should serialize");
        let structured_json =
            serde_json::to_string(&structured).expect("structured ref should serialize");

        let legacy_roundtrip: SkillRef =
            serde_json::from_str(&legacy_json).expect("legacy ref should deserialize");
        let structured_roundtrip: SkillRef =
            serde_json::from_str(&structured_json).expect("structured ref should deserialize");

        assert_eq!(legacy_roundtrip, legacy);
        assert_eq!(structured_roundtrip, structured);
    }

    #[test]
    fn test_identity_lineage_roundtrip() {
        let lineage = SourceIdentityLineage {
            event_id: "lineage-evt-1".to_string(),
            recorded_at_unix_secs: 1_739_974_400,
            required_from_skills: vec![
                SkillName::parse("email-extractor").expect("valid skill slug"),
                SkillName::parse("pdf-processing").expect("valid skill slug"),
            ],
            event: SourceIdentityLineageEvent::Split {
                from: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                    .expect("valid source uuid"),
                into: vec![
                    SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                        .expect("valid source uuid"),
                    SourceUuid::parse("e8df561d-d38f-4242-af55-3a6efb34c950")
                        .expect("valid source uuid"),
                ],
            },
        };

        let json = serde_json::to_string(&lineage).expect("lineage should serialize");
        let decoded: SourceIdentityLineage =
            serde_json::from_str(&json).expect("lineage should deserialize");
        assert_eq!(decoded, lineage);
    }

    #[test]
    fn test_skill_key_remap_roundtrip() {
        let remap = SkillKeyRemap {
            from: SkillKey {
                source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                    .expect("valid source uuid"),
                skill_name: SkillName::parse("email-extractor").expect("valid skill slug"),
            },
            to: SkillKey {
                source_uuid: SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                    .expect("valid source uuid"),
                skill_name: SkillName::parse("mail-extractor").expect("valid skill slug"),
            },
            reason: Some("split source".to_string()),
        };

        let json = serde_json::to_string(&remap).expect("remap should serialize");
        let decoded: SkillKeyRemap = serde_json::from_str(&json).expect("remap should deserialize");
        assert_eq!(decoded, remap);
    }

    #[test]
    fn test_source_health_state_default_thresholds() {
        let thresholds = SourceHealthThresholds::default();
        assert_eq!(
            classify_source_health(0.0, 0, false, thresholds),
            SourceHealthState::Healthy
        );
        assert_eq!(
            classify_source_health(0.05, 0, false, thresholds),
            SourceHealthState::Degraded
        );
        assert_eq!(
            classify_source_health(0.0, 3, false, thresholds),
            SourceHealthState::Degraded
        );
        assert_eq!(
            classify_source_health(0.40, 0, false, thresholds),
            SourceHealthState::Unhealthy
        );
        assert_eq!(
            classify_source_health(0.0, 0, true, thresholds),
            SourceHealthState::Unhealthy
        );
    }

    #[test]
    fn test_source_health_state_overridden_thresholds() {
        let thresholds = SourceHealthThresholds {
            degraded_invalid_ratio: 0.10,
            unhealthy_invalid_ratio: 0.60,
            degraded_failure_streak: 4,
            unhealthy_failure_streak: 8,
        };
        assert_eq!(
            classify_source_health(0.09, 3, false, thresholds),
            SourceHealthState::Healthy
        );
        assert_eq!(
            classify_source_health(0.10, 0, false, thresholds),
            SourceHealthState::Degraded
        );
        assert_eq!(
            classify_source_health(0.0, 4, false, thresholds),
            SourceHealthState::Degraded
        );
        assert_eq!(
            classify_source_health(0.0, 8, false, thresholds),
            SourceHealthState::Unhealthy
        );
    }
}
