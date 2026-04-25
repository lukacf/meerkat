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
pub use identity::{ResolveError, SkillAlias, SourceIdentityRegistry};

// ---------------------------------------------------------------------------
// Canonical identity (Skills V4)
// ---------------------------------------------------------------------------

/// Canonical source UUID shared by embedded (component-crate `inventory`)
/// registrations. Embedded skills all live inside this single logical source.
pub const BUILTIN_SOURCE_UUID: Uuid = Uuid::from_u128(0x0000_0000_0000_4b11_8111_0000_0000_0001);

/// Canonical source UUID for conventional project-local `.rkat/skills`.
pub const PROJECT_LOCAL_SOURCE_UUID: Uuid =
    Uuid::from_u128(0x0000_0000_0000_4b11_8111_0000_0000_0002);

/// Canonical source identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "String", into = "String")]
pub struct SourceUuid(Uuid);

impl SourceUuid {
    pub fn parse(value: &str) -> Result<Self, SkillError> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|e| SkillError::Parse(format!("invalid source_uuid '{value}': {e}").into()))
    }

    pub fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// The canonical UUID for builtin (inventory-registered) skills.
    pub fn builtin() -> Self {
        Self(BUILTIN_SOURCE_UUID)
    }

    /// The canonical UUID for conventional project-local `.rkat/skills`.
    pub fn project_local() -> Self {
        Self(PROJECT_LOCAL_SOURCE_UUID)
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
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
///
/// This is the single identity carried across every surface — the wire parses
/// directly into this struct, tools receive this struct, the registry stores
/// this struct. There is no slash-delimited string path form.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillKey {
    pub source_uuid: SourceUuid,
    pub skill_name: SkillName,
}

impl SkillKey {
    pub fn new(source_uuid: SourceUuid, skill_name: SkillName) -> Self {
        Self {
            source_uuid,
            skill_name,
        }
    }

    /// Build a `SkillKey` whose source is the builtin (inventory) source.
    pub fn builtin(skill_name: SkillName) -> Self {
        Self {
            source_uuid: SourceUuid::builtin(),
            skill_name,
        }
    }
}

impl std::fmt::Display for SkillKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.source_uuid, self.skill_name)
    }
}

/// Structured skill reference for wire/surface input.
///
/// Post-V4 this has exactly one variant — `Structured` — and is kept only as a
/// typed enum to leave room for future source-scoped variants (e.g. alias or
/// capability-scoped forms) without changing the trait surface. No legacy
/// string form, no slash-delimited path, no untagged fallback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillRef {
    Structured(SkillKey),
}

impl SkillRef {
    pub fn key(&self) -> &SkillKey {
        match self {
            Self::Structured(key) => key,
        }
    }

    pub fn into_key(self) -> SkillKey {
        match self {
            Self::Structured(key) => key,
        }
    }
}

impl From<SkillKey> for SkillRef {
    fn from(key: SkillKey) -> Self {
        Self::Structured(key)
    }
}

/// Slug-validated capability identifier for skill requirements.
///
/// Replaces the legacy `Vec<String>` capability lists with a typed
/// namespace. Parsed at construction, so callers cannot smuggle invalid
/// identifiers into descriptors or requirements.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "String", into = "String")]
pub struct CapabilityId(String);

impl CapabilityId {
    pub fn parse(value: &str) -> Result<Self, SkillError> {
        if value.is_empty() {
            return Err(SkillError::Parse("capability_id cannot be empty".into()));
        }

        let bytes = value.as_bytes();
        if bytes.first() == Some(&b'-') || bytes.last() == Some(&b'-') {
            return Err(SkillError::Parse(
                format!("invalid capability_id '{value}': cannot start/end with '-'").into(),
            ));
        }

        if value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            && !value.contains("--")
        {
            return Ok(Self(value.to_string()));
        }

        Err(SkillError::Parse(
            format!("invalid capability_id '{value}': expected lowercase slug [a-z0-9_-], no '--'")
                .into(),
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CapabilityId {
    type Error = SkillError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<CapabilityId> for String {
    fn from(value: CapabilityId) -> Self {
        value.0
    }
}

impl std::fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
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
    #[serde(default = "default_source_identity_status")]
    pub status: SourceIdentityStatus,
}

fn default_source_identity_status() -> SourceIdentityStatus {
    SourceIdentityStatus::Active
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
    pub key: SkillKey,
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
/// `key` is the single authoritative identifier. The `source_name` field is
/// set by `CompositeSkillSource` when merging named sources. Individual
/// `SkillSource` implementations leave it empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDescriptor {
    /// Canonical SkillKey (source_uuid + skill_name).
    pub key: SkillKey,
    /// Human-readable name from SKILL.md frontmatter (e.g. `"email-extractor"`).
    pub name: String,
    pub description: String,
    pub scope: SkillScope,
    /// Extensible metadata (from SKILL.md frontmatter `metadata:` field).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub metadata: IndexMap<String, String>,
    /// Capability requirements (typed slugs; replaces the legacy
    /// `Vec<String>` path).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capability_requirements: Vec<CapabilityId>,
    /// Repository name this skill came from (e.g. "company", "project").
    /// Populated by `CompositeSkillSource` from the `NamedSource` wrapper.
    /// Empty string for sources used outside `CompositeSkillSource`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_name: String,
}

impl SkillDescriptor {
    pub fn new(key: SkillKey, name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key,
            name: name.into(),
            description: description.into(),
            scope: SkillScope::default(),
            metadata: IndexMap::new(),
            capability_requirements: Vec::new(),
            source_name: String::new(),
        }
    }
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
    /// Free-text search across name + description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Filter to a single source UUID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_uuid: Option<SourceUuid>,
}

/// A skill collection (derived from source UUIDs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCollection {
    /// Source UUID that this collection groups by.
    pub source_uuid: SourceUuid,
    /// Human-readable description.
    pub description: String,
    /// Number of skills in this collection.
    pub count: usize,
}

/// A resolved skill ready for injection.
#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    pub key: SkillKey,
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

/// Derive collections from a list of skill descriptors by grouping on
/// `SourceUuid`.
pub fn derive_collections(skills: &[SkillDescriptor]) -> Vec<SkillCollection> {
    let mut counts: BTreeMap<SourceUuid, usize> = BTreeMap::new();
    for skill in skills {
        *counts.entry(skill.key.source_uuid.clone()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(source_uuid, count)| SkillCollection {
            description: if count == 1 {
                "1 skill".to_string()
            } else {
                format!("{count} skills")
            },
            source_uuid,
            count,
        })
        .collect()
}

/// Apply a `SkillFilter` to a slice of descriptors.
pub fn apply_filter(skills: &[SkillDescriptor], filter: &SkillFilter) -> Vec<SkillDescriptor> {
    let query_lower = filter.query.as_ref().map(|q| q.to_lowercase());

    skills
        .iter()
        .filter(|s| {
            if let Some(ref source) = filter.source_uuid
                && &s.key.source_uuid != source
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
// Introspection
// ---------------------------------------------------------------------------

/// Entry for skill introspection — includes provenance and shadow status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIntrospectionEntry {
    /// The skill descriptor (flattened for ergonomics).
    #[serde(flatten)]
    pub descriptor: SkillDescriptor,
    /// If this skill is shadowed, the name of the higher-precedence source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadowed_by: Option<String>,
    /// Canonical source UUID that shadows this skill, when shadowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadowed_by_source_uuid: Option<SourceUuid>,
    /// Whether this skill is active (not shadowed).
    pub is_active: bool,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors from skill operations.
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill not found: {key}")]
    NotFound { key: SkillKey },

    #[error("skill '{key}' requires unavailable capability: {capability}")]
    CapabilityUnavailable {
        key: SkillKey,
        capability: CapabilityId,
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
    /// List skill descriptors, optionally filtered by source UUID or query.
    fn list(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send;

    /// Load a skill document by its canonical key.
    fn load(
        &self,
        key: &SkillKey,
    ) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send;

    /// List collections with counts. Default derives from `list()`.
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
        key: &SkillKey,
    ) -> impl Future<Output = Result<Vec<SkillArtifact>, SkillError>> + Send {
        let missing = key.clone();
        async move { Err(SkillError::NotFound { key: missing }) }
    }

    /// Read a specific artifact from a skill.
    fn read_artifact(
        &self,
        key: &SkillKey,
        artifact_path: &str,
    ) -> impl Future<Output = Result<SkillArtifactContent, SkillError>> + Send {
        let missing = key.clone();
        let _ = artifact_path;
        async move { Err(SkillError::NotFound { key: missing }) }
    }

    /// Invoke a skill-defined function with structured arguments.
    fn invoke_function(
        &self,
        key: &SkillKey,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send {
        let missing = key.clone();
        let _ = function_name;
        let _ = arguments;
        async move { Err(SkillError::NotFound { key: missing }) }
    }

    /// List all skills with provenance information (active + shadowed).
    fn list_all_with_provenance(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillIntrospectionEntry>, SkillError>> + Send {
        async {
            let skills = self.list(filter).await?;
            Ok(skills
                .into_iter()
                .map(|descriptor| SkillIntrospectionEntry {
                    descriptor,
                    shadowed_by: None,
                    shadowed_by_source_uuid: None,
                    is_active: true,
                })
                .collect())
        }
    }

    /// Load a skill from a specific named source, bypassing first-wins resolution.
    fn load_from_source(
        &self,
        key: &SkillKey,
        _source_name: Option<&str>,
    ) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send {
        async move { self.load(key).await }
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

    fn load(
        &self,
        key: &SkillKey,
    ) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send {
        async move { (**self).load(key).await }
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
        key: &SkillKey,
    ) -> impl Future<Output = Result<Vec<SkillArtifact>, SkillError>> + Send {
        async move { (**self).list_artifacts(key).await }
    }

    fn read_artifact(
        &self,
        key: &SkillKey,
        artifact_path: &str,
    ) -> impl Future<Output = Result<SkillArtifactContent, SkillError>> + Send {
        async move { (**self).read_artifact(key, artifact_path).await }
    }

    fn invoke_function(
        &self,
        key: &SkillKey,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send {
        async move {
            (**self)
                .invoke_function(key, function_name, arguments)
                .await
        }
    }

    fn list_all_with_provenance(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillIntrospectionEntry>, SkillError>> + Send {
        async move { (**self).list_all_with_provenance(filter).await }
    }

    fn load_from_source(
        &self,
        key: &SkillKey,
        source_name: Option<&str>,
    ) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send {
        let source_name = source_name.map(ToString::to_string);
        async move { (**self).load_from_source(key, source_name.as_deref()).await }
    }
}

/// Engine that manages skill resolution and rendering.
pub trait SkillEngine: Send + Sync {
    /// Generate the system prompt inventory (XML, compact).
    fn inventory_section(&self) -> impl Future<Output = Result<String, SkillError>> + Send;

    /// Resolve skill keys and render injection content.
    fn resolve_and_render(
        &self,
        keys: &[SkillKey],
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
        key: &SkillKey,
    ) -> impl Future<Output = Result<Vec<SkillArtifact>, SkillError>> + Send;

    /// Read a specific artifact from a skill.
    fn read_artifact(
        &self,
        key: &SkillKey,
        artifact_path: &str,
    ) -> impl Future<Output = Result<SkillArtifactContent, SkillError>> + Send;

    /// Invoke a skill-defined function with structured arguments.
    fn invoke_function(
        &self,
        key: &SkillKey,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send;

    /// List all skills with provenance and shadow information.
    fn list_all_with_provenance(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillIntrospectionEntry>, SkillError>> + Send {
        async {
            let skills = self.list_skills(filter).await?;
            Ok(skills
                .into_iter()
                .map(|descriptor| SkillIntrospectionEntry {
                    descriptor,
                    shadowed_by: None,
                    shadowed_by_source_uuid: None,
                    is_active: true,
                })
                .collect())
        }
    }

    /// Load a skill from a specific named source, bypassing first-wins resolution.
    fn load_from_source(
        &self,
        key: &SkillKey,
        _source_name: Option<&str>,
    ) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send {
        let _ = _source_name;
        let missing = key.clone();
        async move { Err(SkillError::NotFound { key: missing }) }
    }

    /// Apply the engine's source-identity lineage remap chain to `key`,
    /// producing the canonical `SkillKey` to use for downstream resolve
    /// operations. Engines that do not own a `SourceIdentityRegistry`
    /// return the input unchanged — the default preserves pre-V4
    /// behavior for lightweight/test engines while giving registry-backed
    /// engines (e.g. `meerkat_skills::DefaultSkillEngine`) a typed seam
    /// that builtin skill tools can consume before calling resolve,
    /// closing the lineage-remap gap on `load_skill` / `read_artifact` /
    /// `invoke_function` / `list_artifacts`.
    fn canonical_skill_key(
        &self,
        key: &SkillKey,
    ) -> impl Future<Output = Result<SkillKey, SkillError>> + Send {
        let canonical = key.clone();
        async move { Ok(canonical) }
    }
}

type OwnedSkillFuture<T> = Pin<Box<dyn Future<Output = Result<T, SkillError>> + Send + 'static>>;
type InventoryFn = dyn Fn() -> OwnedSkillFuture<String> + Send + Sync;
type ResolveFn = dyn Fn(Vec<SkillKey>) -> OwnedSkillFuture<Vec<ResolvedSkill>> + Send + Sync;
type CollectionsFn = dyn Fn() -> OwnedSkillFuture<Vec<SkillCollection>> + Send + Sync;
type ListSkillsFn = dyn Fn(SkillFilter) -> OwnedSkillFuture<Vec<SkillDescriptor>> + Send + Sync;
type QuarantinedDiagnosticsFn =
    dyn Fn() -> OwnedSkillFuture<Vec<SkillQuarantineDiagnostic>> + Send + Sync;
type HealthSnapshotFn = dyn Fn() -> OwnedSkillFuture<SourceHealthSnapshot> + Send + Sync;
type ListArtifactsFn = dyn Fn(SkillKey) -> OwnedSkillFuture<Vec<SkillArtifact>> + Send + Sync;
type ReadArtifactFn =
    dyn Fn(SkillKey, String) -> OwnedSkillFuture<SkillArtifactContent> + Send + Sync;
type InvokeFunctionFn = dyn Fn(SkillKey, String, serde_json::Value) -> OwnedSkillFuture<serde_json::Value>
    + Send
    + Sync;
type ListAllWithProvenanceFn =
    dyn Fn(SkillFilter) -> OwnedSkillFuture<Vec<SkillIntrospectionEntry>> + Send + Sync;
type LoadFromSourceFn =
    dyn Fn(SkillKey, Option<String>) -> OwnedSkillFuture<SkillDocument> + Send + Sync;
type CanonicalSkillKeyFn = dyn Fn(SkillKey) -> OwnedSkillFuture<SkillKey> + Send + Sync;

#[derive(Clone)]
#[allow(clippy::struct_field_names)]
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
    list_all_with_provenance_fn: Arc<ListAllWithProvenanceFn>,
    load_from_source_fn: Arc<LoadFromSourceFn>,
    canonical_skill_key_fn: Arc<CanonicalSkillKeyFn>,
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
        let invoke_function_engine = Arc::clone(&engine);
        let provenance_engine = Arc::clone(&engine);
        let canonical_engine = Arc::clone(&engine);
        let load_from_source_engine = engine;

        Self {
            inventory_fn: Arc::new(move || {
                let engine = Arc::clone(&inventory_engine);
                Box::pin(async move { engine.inventory_section().await })
            }),
            resolve_fn: Arc::new(move |keys: Vec<SkillKey>| {
                let engine = Arc::clone(&resolve_engine);
                Box::pin(async move { engine.resolve_and_render(&keys).await })
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
            list_artifacts_fn: Arc::new(move |key: SkillKey| {
                let engine = Arc::clone(&list_artifacts_engine);
                Box::pin(async move { engine.list_artifacts(&key).await })
            }),
            read_artifact_fn: Arc::new(move |key: SkillKey, artifact_path: String| {
                let engine = Arc::clone(&read_artifact_engine);
                Box::pin(async move { engine.read_artifact(&key, &artifact_path).await })
            }),
            invoke_function_fn: Arc::new(
                move |key: SkillKey, function_name: String, arguments: serde_json::Value| {
                    let engine = Arc::clone(&invoke_function_engine);
                    Box::pin(async move {
                        engine
                            .invoke_function(&key, &function_name, arguments)
                            .await
                    })
                },
            ),
            list_all_with_provenance_fn: Arc::new(move |filter: SkillFilter| {
                let engine = Arc::clone(&provenance_engine);
                Box::pin(async move { engine.list_all_with_provenance(&filter).await })
            }),
            load_from_source_fn: Arc::new(move |key: SkillKey, source_name: Option<String>| {
                let engine = Arc::clone(&load_from_source_engine);
                Box::pin(async move { engine.load_from_source(&key, source_name.as_deref()).await })
            }),
            canonical_skill_key_fn: Arc::new(move |key: SkillKey| {
                let engine = Arc::clone(&canonical_engine);
                Box::pin(async move { engine.canonical_skill_key(&key).await })
            }),
        }
    }

    pub async fn inventory_section(&self) -> Result<String, SkillError> {
        (self.inventory_fn)().await
    }

    pub async fn resolve_and_render(
        &self,
        keys: &[SkillKey],
    ) -> Result<Vec<ResolvedSkill>, SkillError> {
        (self.resolve_fn)(keys.to_vec()).await
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

    pub async fn list_artifacts(&self, key: &SkillKey) -> Result<Vec<SkillArtifact>, SkillError> {
        (self.list_artifacts_fn)(key.clone()).await
    }

    pub async fn read_artifact(
        &self,
        key: &SkillKey,
        artifact_path: &str,
    ) -> Result<SkillArtifactContent, SkillError> {
        (self.read_artifact_fn)(key.clone(), artifact_path.to_string()).await
    }

    pub async fn invoke_function(
        &self,
        key: &SkillKey,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, SkillError> {
        (self.invoke_function_fn)(key.clone(), function_name.to_string(), arguments).await
    }

    pub async fn list_all_with_provenance(
        &self,
        filter: &SkillFilter,
    ) -> Result<Vec<SkillIntrospectionEntry>, SkillError> {
        (self.list_all_with_provenance_fn)(filter.clone()).await
    }

    pub async fn load_from_source(
        &self,
        key: &SkillKey,
        source_name: Option<&str>,
    ) -> Result<SkillDocument, SkillError> {
        (self.load_from_source_fn)(key.clone(), source_name.map(ToString::to_string)).await
    }

    /// Apply the engine's source-identity lineage remap chain to `key`.
    /// See [`SkillEngine::canonical_skill_key`] for semantics.
    pub async fn canonical_skill_key(&self, key: &SkillKey) -> Result<SkillKey, SkillError> {
        (self.canonical_skill_key_fn)(key.clone()).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn test_key(skill: &str) -> SkillKey {
        SkillKey {
            source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                .expect("valid uuid"),
            skill_name: SkillName::parse(skill).expect("valid slug"),
        }
    }

    #[test]
    fn test_skill_key_json_roundtrip() {
        let key = test_key("email-extractor");
        let json = serde_json::to_string(&key).expect("serialize");
        let decoded: SkillKey = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, key);
    }

    #[test]
    fn test_skill_name_slug_validation() {
        assert!(SkillName::parse("email-extractor").is_ok());
        assert!(SkillName::parse("EmailExtractor").is_err());
        assert!(SkillName::parse("email_extractor").is_err());
        assert!(SkillName::parse("-leading").is_err());
        assert!(SkillName::parse("trailing-").is_err());
        assert!(SkillName::parse("double--dash").is_err());
        assert!(SkillName::parse("").is_err());
    }

    #[test]
    fn test_capability_id_slug_validation() {
        assert!(CapabilityId::parse("builtins").is_ok());
        assert!(CapabilityId::parse("shell_patterns").is_ok());
        assert!(CapabilityId::parse("mcp-bridge").is_ok());
        assert!(CapabilityId::parse("").is_err());
        assert!(CapabilityId::parse("-leading").is_err());
        assert!(CapabilityId::parse("trailing-").is_err());
        assert!(CapabilityId::parse("double--dash").is_err());
        assert!(CapabilityId::parse("UPPER").is_err());
        assert!(CapabilityId::parse("space x").is_err());
    }

    #[test]
    fn test_capability_id_json_roundtrip() {
        let cap = CapabilityId::parse("builtins").expect("valid");
        let json = serde_json::to_string(&cap).expect("serialize");
        assert_eq!(json, "\"builtins\"");
        let decoded: CapabilityId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, cap);
    }

    #[test]
    fn test_skill_ref_structured_only_serde() {
        let key = test_key("email-extractor");
        let r = SkillRef::Structured(key);
        let json = serde_json::to_value(&r).expect("serialize");
        // The tag must be "structured" and there must be no legacy variant.
        assert_eq!(json["kind"], "structured");
        let decoded: SkillRef = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded, r);
    }

    #[test]
    fn test_skill_ref_rejects_string_form() {
        // Legacy slash-delimited string form is no longer a valid SkillRef.
        let res: Result<SkillRef, _> = serde_json::from_str("\"extraction/email\"");
        assert!(res.is_err());
    }

    #[test]
    fn test_derive_collections_groups_by_source() {
        let a = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").expect("uuid a");
        let b = SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7").expect("uuid b");
        let skills = vec![
            SkillDescriptor::new(
                SkillKey::new(a.clone(), SkillName::parse("email").expect("slug")),
                "email",
                "Extract emails",
            ),
            SkillDescriptor::new(
                SkillKey::new(a.clone(), SkillName::parse("pdf").expect("slug")),
                "pdf",
                "Process pdf",
            ),
            SkillDescriptor::new(
                SkillKey::new(b.clone(), SkillName::parse("markdown").expect("slug")),
                "markdown",
                "Render markdown",
            ),
        ];
        let collections = derive_collections(&skills);
        assert_eq!(collections.len(), 2);
        let a_coll = collections
            .iter()
            .find(|c| c.source_uuid == a)
            .expect("a present");
        assert_eq!(a_coll.count, 2);
        let b_coll = collections
            .iter()
            .find(|c| c.source_uuid == b)
            .expect("b present");
        assert_eq!(b_coll.count, 1);
    }

    #[test]
    fn test_apply_filter_by_source_uuid() {
        let a = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").expect("uuid a");
        let b = SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7").expect("uuid b");
        let skills = vec![
            SkillDescriptor::new(
                SkillKey::new(a.clone(), SkillName::parse("email").expect("slug")),
                "email",
                "",
            ),
            SkillDescriptor::new(
                SkillKey::new(b, SkillName::parse("pdf").expect("slug")),
                "pdf",
                "",
            ),
        ];
        let filtered = apply_filter(
            &skills,
            &SkillFilter {
                source_uuid: Some(a.clone()),
                ..Default::default()
            },
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].key.source_uuid, a);
    }
}
