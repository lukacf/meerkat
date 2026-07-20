//! Level-triggered desired-state contracts for stable mob-member identities.
//!
//! There are deliberately only three durable reconciliation inputs:
//! [`IdentityIntentRecord`], [`IdentityLeaseRecord`], and narrowly scoped
//! [`IdentityOperationReceipt`] custody. [`IdentityDeclarationScopeHead`] is
//! transaction-ordering metadata for complete provider snapshots, never a
//! lifecycle input; [`IdentityConvergenceStatus`] is replaceable output only.
//! Session, runtime, roster, and wiring state are observed realization, and
//! reconciliation never persists a second copy of them here.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use meerkat_contracts::wire::{
    PortableDefinitionExtract, PortableProfile, PortableSystemPrompt, WireAuthBindingRef,
    WireMobRuntimeMode, WireOpaqueJson, WireResolvedToolAccessPolicy, WireTrustedPeerIdentity,
};
use meerkat_core::lifecycle::InputId;
use meerkat_core::ops::OperationId;
use meerkat_core::{
    BudgetLimits, ContentInput, Session, SessionCheckpointAncestryProof,
    SessionCheckpointAuthorityBase, SessionCheckpointProvenance, SessionCheckpointRelation,
    SessionCheckpointStamp, SessionCheckpointState, SessionGeneration, SessionId, SessionLineageId,
    ToolName, session_checkpoint_relation,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{AgentIdentity, MobId, ProfileName};

pub const IDENTITY_INTENT_SCHEMA_VERSION: u32 = 1;
pub const IDENTITY_DECLARATION_SCOPE_SCHEMA_VERSION: u32 = 1;
pub const IDENTITY_LEASE_SCHEMA_VERSION: u32 = 1;
pub const IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const IDENTITY_INTENT_MAX_ENCODED_BYTES: usize = 4 * 1024 * 1024;
pub const IDENTITY_DECLARATION_MANIFEST_MAX_ENCODED_BYTES: usize = 4 * 1024 * 1024;
pub const IDENTITY_LEASE_MAX_TTL_MS: u64 = 30_000;

/// Stable provider-owned declaration namespace within one mob.
///
/// A scope owns one complete roster/wiring snapshot.  Omission can therefore
/// mean deletion without allowing one provider to retire another provider's
/// identities.  It is write-ordering metadata only; the per-identity intent
/// rows remain the sole desired-state authority consumed by reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdentityDeclarationScopeId(String);

impl IdentityDeclarationScopeId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentityIntentError> {
        let value = value.into();
        validate_text("identity_declaration_scope", &value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Transactional ordering index for one complete declaration scope.
///
/// This row is deliberately not reconciliation input and owns no member
/// lifecycle semantics.  It exists so an initially empty scope has a durable
/// CAS head after restart.  Nonempty scope ownership remains sealed into each
/// [`IdentityIntentRecord`]; the store may audit/repair this index from those
/// rows and immutable receipts where possible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityDeclarationScopeHead {
    pub schema_version: u32,
    pub mob_id: MobId,
    pub scope_id: IdentityDeclarationScopeId,
    pub revision: u64,
    pub operation_id: OperationId,
    pub request_digest: String,
    pub compiled_manifest_digest: String,
    pub declared_member_count: u64,
    pub authority_digest: String,
}

impl IdentityDeclarationScopeHead {
    pub fn canonical_authority_digest(&self) -> Result<String, IdentityIntentError> {
        #[derive(Serialize)]
        struct DigestMaterial<'a> {
            domain: &'static str,
            schema_version: u32,
            mob_id: &'a MobId,
            scope_id: &'a IdentityDeclarationScopeId,
            revision: u64,
            operation_id: &'a OperationId,
            request_digest: &'a str,
            compiled_manifest_digest: &'a str,
            declared_member_count: u64,
        }
        let bytes = serde_json::to_vec(&DigestMaterial {
            domain: "meerkat.identity.declaration_scope_head.v1",
            schema_version: self.schema_version,
            mob_id: &self.mob_id,
            scope_id: &self.scope_id,
            revision: self.revision,
            operation_id: &self.operation_id,
            request_digest: &self.request_digest,
            compiled_manifest_digest: &self.compiled_manifest_digest,
            declared_member_count: self.declared_member_count,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        if self.schema_version != IDENTITY_DECLARATION_SCOPE_SCHEMA_VERSION {
            return Err(IdentityIntentError::UnsupportedSchemaVersion {
                record: "identity_declaration_scope_head",
                version: self.schema_version,
            });
        }
        validate_text("mob_id", self.mob_id.as_str())?;
        validate_text("identity_declaration_scope", self.scope_id.as_str())?;
        if self.revision == 0 || self.operation_id.0.is_nil() {
            return Err(IdentityIntentError::InvalidDeclarationScopeHead);
        }
        validate_sha256_digest(&self.request_digest)?;
        validate_sha256_digest(&self.compiled_manifest_digest)?;
        if self.canonical_authority_digest()? != self.authority_digest {
            return Err(IdentityIntentError::DigestMismatch);
        }
        Ok(())
    }
}

/// Exact desired session lineage for a stable member identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredSessionTarget {
    pub session_id: SessionId,
    /// Stable, content-authority lineage selected by the caller.  Ownership
    /// lease/fence tokens are deliberately not part of this identity.
    pub lineage_id: SessionLineageId,
    pub lineage_generation: SessionGeneration,
    pub authority_policy: DesiredSessionAuthorityPolicy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesiredSessionAuthorityPolicy {
    /// An absent target may be created with an empty transcript.
    #[default]
    CreateIfAbsent,
    /// Absence is evidence loss and must be surfaced as repair-blocked.
    RequireExisting,
}

/// Closed desired execution target.  Invalid combinations such as an external
/// binding plus a placed-session host are structurally unrepresentable.
/// Runtime ids, bootstrap tokens, pairing secrets, peer-id assertions,
/// host-binding epochs, and host/operator authority are absent by design.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "execution", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesiredExecution {
    ControllingSession,
    AnyBoundHostSession,
    PlacedSession {
        host_id: String,
    },
    External {
        address: DesiredExternalAddress,
        identity: WireTrustedPeerIdentity,
    },
}

/// Canonical, credential-free TCP address for a stable external member.
/// One-time pairing/bootstrap authority is deliberately not representable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DesiredExternalAddress(String);

impl DesiredExternalAddress {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, IdentityIntentError> {
        let value = value.as_ref();
        let parsed = url::Url::parse(value)
            .map_err(|error| IdentityIntentError::InvalidExternalAddress(error.to_string()))?;
        if parsed.scheme() != "tcp"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !(parsed.path().is_empty() || parsed.path() == "/")
        {
            return Err(IdentityIntentError::InvalidExternalAddress(
                "external address must be tcp://host:port with no credentials, path, query, or fragment"
                    .to_string(),
            ));
        }
        let host = parsed.host_str().ok_or_else(|| {
            IdentityIntentError::InvalidExternalAddress("external address has no host".to_string())
        })?;
        let port = parsed.port().ok_or_else(|| {
            IdentityIntentError::InvalidExternalAddress("external address has no port".to_string())
        })?;
        let canonical_host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        Ok(Self(format!("tcp://{canonical_host}:{port}")))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DesiredExternalAddress {
    type Error = IdentityIntentError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<DesiredExternalAddress> for String {
    fn from(value: DesiredExternalAddress) -> Self {
        value.0
    }
}

/// Authority-free per-spawn overlay.  This mirrors the portable build
/// vocabulary while deliberately omitting `mob_tool_authority_context` and
/// continuity: the actor mints current operator authority during
/// materialization and the outer intent owns the exact session target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredMemberOverlay {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<WireOpaqueJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    pub system_prompt: PortableSystemPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_access_policy: Option<WireResolvedToolAccessPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<WireAuthBindingRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_limits: Option<BudgetLimits>,
    pub runtime_mode: WireMobRuntimeMode,
}

/// One model-visible callback tool whose executable handler is process-local.
/// The exact name, description, and JSON input schema are durable desired
/// material; callback scopes, handlers, and dispatch authority are not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredLocalCallbackTool {
    pub name: ToolName,
    pub description: String,
    pub input_schema: serde_json::Value,
}

impl DesiredLocalCallbackTool {
    pub fn new(
        name: impl Into<ToolName>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Result<Self, IdentityIntentError> {
        let value = Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_text("local_callback_tool_name", self.name.as_str())?;
        validate_text("local_callback_tool_description", &self.description)?;
        if !self.input_schema.is_object() {
            return Err(IdentityIntentError::InvalidMemberMaterial(format!(
                "local callback tool '{}' input schema must be a JSON object",
                self.name
            )));
        }
        jsonschema::validator_for(&self.input_schema).map_err(|error| {
            IdentityIntentError::InvalidMemberMaterial(format!(
                "local callback tool '{}' input schema is invalid: {error}",
                self.name
            ))
        })?;
        Ok(())
    }
}

/// Fully resolved, authority-free desired construction material.
///
/// Unlike `PortableMemberSpec`, this value has no mob id, member identity, or
/// minted operator authority. Those are supplied by the actor immediately
/// before materialization. Secret values and in-process callbacks are also
/// absent; declarative tool configuration and required secret names remain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredMemberMaterial {
    pub profile_name: ProfileName,
    pub profile: PortableProfile,
    pub definition_extract: PortableDefinitionExtract,
    pub overlay: DesiredMemberOverlay,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_env_keys: Vec<String>,
    /// Exact model-visible definitions whose handlers must be supplied by
    /// process-local materialization services. Handlers and callback scopes
    /// are deliberately absent from durable desired state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_local_callback_tools: Vec<DesiredLocalCallbackTool>,
    pub execution: DesiredExecution,
}

impl DesiredMemberMaterial {
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_text("profile_name", self.profile_name.as_str())?;
        validate_execution(&self.execution)?;
        validate_string_set("required_env_key", &self.required_env_keys)?;
        validate_required_local_callback_tools(&self.required_local_callback_tools, true)?;
        validate_string_set(
            "definition_profile_name",
            &self.definition_extract.profile_names,
        )?;
        if !self
            .definition_extract
            .profile_names
            .iter()
            .any(|name| name == self.profile_name.as_str())
        {
            return Err(IdentityIntentError::InvalidMemberMaterial(
                "definition extract does not contain the selected profile name".to_string(),
            ));
        }
        if self.profile.runtime_mode != self.overlay.runtime_mode {
            return Err(IdentityIntentError::InvalidMemberMaterial(
                "profile and overlay runtime modes differ".to_string(),
            ));
        }
        if matches!(self.execution, DesiredExecution::External { .. })
            && !matches!(self.overlay.runtime_mode, WireMobRuntimeMode::TurnDriven)
        {
            return Err(IdentityIntentError::InvalidMemberMaterial(
                "external execution requires turn-driven runtime mode".to_string(),
            ));
        }

        let mut declared_skills = BTreeSet::new();
        for skill in &self.profile.skills {
            validate_text("profile_skill", skill)?;
            if !declared_skills.insert(skill.as_str()) {
                return Err(IdentityIntentError::InvalidMemberMaterial(format!(
                    "profile repeats skill '{skill}'"
                )));
            }
        }
        let extracted_skills = self
            .definition_extract
            .skills
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if declared_skills != extracted_skills {
            return Err(IdentityIntentError::InvalidMemberMaterial(
                "profile skills do not exactly match the definition extract".to_string(),
            ));
        }
        if let Some(policy) = &self.overlay.tool_access_policy {
            match policy {
                WireResolvedToolAccessPolicy::AllowList(names)
                | WireResolvedToolAccessPolicy::DenyList(names) => {
                    validate_string_set("tool_access_policy_name", names)?;
                }
            }
        }

        let rehydrated = crate::portable_profile::rehydrate_portable_profile(&self.profile)
            .map_err(IdentityIntentError::InvalidMemberMaterial)?;
        let projected = crate::portable_profile::project_portable_profile(
            &rehydrated,
            rehydrated.runtime_mode,
            &self.definition_extract.models,
            self.profile_name.as_str(),
            self.profile_name.as_str(),
            Vec::new(),
        )
        .map_err(IdentityIntentError::InvalidMemberMaterial)?;
        if projected != self.profile {
            return Err(IdentityIntentError::InvalidMemberMaterial(
                "portable profile does not round-trip exactly".to_string(),
            ));
        }
        Ok(())
    }
}

/// Actor-minted stable identity for one requested initial delivery.
///
/// The caller declares only message content.  The actor preserves this value
/// across unrelated desired-spec edits, drops it when the declaration removes
/// the message, and mints a strict successor when a later declaration re-arms
/// delivery.  Lost acknowledgements therefore retry one `InputId`, never the
/// raw content as a fresh effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredInitialDelivery {
    pub delivery_generation: u64,
    pub delivery_id: InputId,
    pub message_digest: String,
    pub message: ContentInput,
}

impl DesiredInitialDelivery {
    pub fn new(
        delivery_generation: u64,
        delivery_id: InputId,
        message: ContentInput,
    ) -> Result<Self, IdentityIntentError> {
        let message_digest = canonical_initial_message_digest(&message)?;
        let value = Self {
            delivery_generation,
            delivery_id,
            message_digest,
            message,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        if self.delivery_generation == 0
            || self.delivery_id.0.is_nil()
            || self.message_digest != canonical_initial_message_digest(&self.message)?
        {
            return Err(IdentityIntentError::InvalidInitialDelivery);
        }
        Ok(())
    }
}

/// Store-sealed member specification carried by the sole identity intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredMemberSpec {
    pub material: DesiredMemberMaterial,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_delivery: Option<DesiredInitialDelivery>,
}

impl DesiredMemberSpec {
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        self.material.validate()?;
        if let Some(delivery) = &self.initial_delivery {
            delivery.validate()?;
        }
        Ok(())
    }

    #[must_use]
    pub fn execution(&self) -> &DesiredExecution {
        &self.material.execution
    }
}

/// Safe compatibility input compiled by the actor against its canonical
/// `MobDefinition`.  `profile_override` is resolved portable material, not a
/// callback; operator authority, environment values, and bootstrap secrets
/// are deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityProfileMemberDeclaration {
    pub profile_name: ProfileName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_override: Option<PortableProfile>,
    /// Safe, authority-free model override applied by the actor before the
    /// canonical portable-profile projection is sealed into the intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    /// Safe, authority-free addressability override applied by the actor
    /// before sealing the portable profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_addressable_override: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<WireOpaqueJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_override: Option<PortableSystemPrompt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_access_policy: Option<WireResolvedToolAccessPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<WireAuthBindingRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_limits: Option<BudgetLimits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_env_keys: Vec<String>,
    /// Model-visible callback definitions compiled into durable desired
    /// material. The matching executable dispatcher remains process-local.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_local_callback_tools: Vec<DesiredLocalCallbackTool>,
    pub execution: DesiredExecution,
}

impl IdentityProfileMemberDeclaration {
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_text("profile_name", self.profile_name.as_str())?;
        if let Some(model) = &self.model_override {
            validate_text("identity_model_override", model)?;
        }
        validate_string_set("required_env_key", &self.required_env_keys)?;
        validate_required_local_callback_tools(&self.required_local_callback_tools, false)?;
        validate_execution(&self.execution)
    }
}

/// Public material input.  Compatibility callers name a profile and safe
/// overlays; advanced callers may supply already-resolved portable material.
/// Both routes end in the same exact `DesiredMemberMaterial` in the intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityMemberMaterialDeclaration {
    Profile(IdentityProfileMemberDeclaration),
    Resolved { material: DesiredMemberMaterial },
}

impl IdentityMemberMaterialDeclaration {
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        match self {
            Self::Profile(declaration) => declaration.validate(),
            Self::Resolved { material } => material.validate(),
        }
    }
}

/// One member in a provider-scoped complete desired snapshot. Session target,
/// intent revision, and one-shot delivery identity are intentionally absent:
/// the actor/store transaction reuses or allocates and seals those values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityMemberDeclaration {
    pub material: IdentityMemberMaterialDeclaration,
    #[serde(default)]
    pub session_authority_policy: DesiredSessionAuthorityPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_message: Option<ContentInput>,
    /// Optional one-time adoption of an exact, already-verified legacy
    /// checkpoint. The request remains part of the manifest digest and its
    /// immutable apply receipt, but the ownership fences are never folded
    /// into checkpoint content authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_import: Option<IdentityLegacyImport>,
}

impl IdentityMemberDeclaration {
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        self.material.validate()?;
        if let Some(legacy_import) = &self.legacy_import {
            legacy_import.validate()?;
            if self.session_authority_policy != DesiredSessionAuthorityPolicy::RequireExisting
                || self.initial_message.is_some()
            {
                return Err(IdentityIntentError::InvalidLegacyImport);
            }
        }
        Ok(())
    }
}

/// One-time adoption evidence for a verified legacy session document.
///
/// `continuity_epoch_highwater` seeds only lease ownership. The snapshot-side
/// fence is retained solely in the request digest as audit evidence and is
/// deliberately never compared with either the continuity high-water or the
/// checkpoint stamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "migration", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityLegacyImport {
    AdoptVerifiedLegacy {
        session: DesiredSessionTarget,
        checkpoint: SessionCheckpointStamp,
        continuity_epoch_highwater: u64,
        snapshot_fence_audit: u64,
    },
}

impl IdentityLegacyImport {
    #[must_use]
    pub const fn session(&self) -> &DesiredSessionTarget {
        match self {
            Self::AdoptVerifiedLegacy { session, .. } => session,
        }
    }

    #[must_use]
    pub const fn checkpoint(&self) -> &SessionCheckpointStamp {
        match self {
            Self::AdoptVerifiedLegacy { checkpoint, .. } => checkpoint,
        }
    }

    #[must_use]
    pub const fn continuity_epoch_highwater(&self) -> u64 {
        match self {
            Self::AdoptVerifiedLegacy {
                continuity_epoch_highwater,
                ..
            } => *continuity_epoch_highwater,
        }
    }

    #[must_use]
    pub const fn snapshot_fence_audit(&self) -> u64 {
        match self {
            Self::AdoptVerifiedLegacy {
                snapshot_fence_audit,
                ..
            } => *snapshot_fence_audit,
        }
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        let session = self.session();
        let checkpoint = self.checkpoint();
        validate_session_target(session)?;
        if session.authority_policy != DesiredSessionAuthorityPolicy::RequireExisting
            || checkpoint.provenance() != SessionCheckpointProvenance::RecoveryMigration
            || checkpoint.session_id() != &session.session_id
            || checkpoint.lineage_id() != &session.lineage_id
            || checkpoint.generation() != session.lineage_generation
            || !matches!(
                checkpoint.authority_base(),
                SessionCheckpointAuthorityBase::Legacy {
                    observed_generation,
                    observed_checkpoint_revision,
                    ..
                } if *observed_generation == session.lineage_generation
                    && *observed_checkpoint_revision == checkpoint.checkpoint_revision()
            )
        {
            return Err(IdentityIntentError::InvalidLegacyImport);
        }
        checkpoint
            .validate_for_session(&session.session_id)
            .map_err(session_checkpoint_error)?;
        self.continuity_epoch_highwater().checked_add(1).ok_or(
            IdentityIntentError::CounterExhausted {
                counter: "legacy lease epoch highwater",
            },
        )?;
        // snapshot_fence_audit is intentionally unconstrained: it is
        // historical audit evidence, not checkpoint or lease authority.
        let _ = self.snapshot_fence_audit();
        Ok(())
    }
}

/// Actor-compiled, allocation-ready member input for one atomic manifest
/// apply. Candidate ids are ignored when the store can reuse a valid current
/// target; they become authority only if selected and committed in the sole
/// intent row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityDeclarationMemberPlan {
    pub material: DesiredMemberMaterial,
    pub session_authority_policy: DesiredSessionAuthorityPolicy,
    pub initial_message: Option<ContentInput>,
    pub candidate_session_id: SessionId,
    pub candidate_lineage_id: SessionLineageId,
    pub candidate_initial_delivery_id: Option<InputId>,
}

impl IdentityDeclarationMemberPlan {
    pub fn candidate_session_target(&self) -> DesiredSessionTarget {
        DesiredSessionTarget {
            session_id: self.candidate_session_id.clone(),
            lineage_id: self.candidate_lineage_id.clone(),
            lineage_generation: SessionGeneration::INITIAL,
            authority_policy: self.session_authority_policy,
        }
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        self.material.validate()?;
        validate_session_target(&self.candidate_session_target())?;
        if self.candidate_initial_delivery_id.is_some() != self.initial_message.is_some()
            || self
                .candidate_initial_delivery_id
                .as_ref()
                .is_some_and(|delivery_id| delivery_id.0.is_nil())
        {
            return Err(IdentityIntentError::InvalidInitialDelivery);
        }
        Ok(())
    }
}

/// Exact actor-to-store transaction plan. It contains no observed realization
/// and no actuator authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityDeclarationApplyPlan {
    pub(crate) scope_id: IdentityDeclarationScopeId,
    pub(crate) operation_id: OperationId,
    pub(crate) expected_scope: IdentityDeclarationScopePrecondition,
    pub(crate) request_digest: String,
    pub(crate) members: BTreeMap<AgentIdentity, IdentityDeclarationMemberPlan>,
    pub(crate) wiring: BTreeSet<DesiredIdentityEdge>,
    pub(crate) legacy_imports: BTreeMap<AgentIdentity, IdentityLegacyImport>,
}

impl IdentityDeclarationApplyPlan {
    pub(crate) fn from_compiled_manifest(
        manifest: &IdentityDeclarationManifest,
        members: BTreeMap<AgentIdentity, IdentityDeclarationMemberPlan>,
    ) -> Result<Self, IdentityIntentError> {
        manifest.validate()?;
        if members.keys().ne(manifest.members.keys()) {
            return Err(IdentityIntentError::InvalidDeclarationOutcome);
        }
        let legacy_imports = manifest
            .members
            .iter()
            .filter_map(|(identity, member)| {
                member
                    .legacy_import
                    .clone()
                    .map(|legacy_import| (identity.clone(), legacy_import))
            })
            .collect();
        let value = Self {
            scope_id: manifest.scope_id.clone(),
            operation_id: manifest.operation_id.clone(),
            expected_scope: manifest.expected_scope,
            request_digest: manifest.request_digest()?,
            members,
            wiring: manifest.wiring.clone(),
            legacy_imports,
        };
        value.validate()?;
        Ok(value)
    }

    #[must_use]
    pub fn scope_id(&self) -> &IdentityDeclarationScopeId {
        &self.scope_id
    }

    #[must_use]
    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    #[must_use]
    pub const fn expected_scope(&self) -> IdentityDeclarationScopePrecondition {
        self.expected_scope
    }

    #[must_use]
    pub fn request_digest(&self) -> &str {
        &self.request_digest
    }

    #[must_use]
    pub fn members(&self) -> &BTreeMap<AgentIdentity, IdentityDeclarationMemberPlan> {
        &self.members
    }

    #[must_use]
    pub fn wiring(&self) -> &BTreeSet<DesiredIdentityEdge> {
        &self.wiring
    }

    #[must_use]
    pub fn legacy_imports(&self) -> &BTreeMap<AgentIdentity, IdentityLegacyImport> {
        &self.legacy_imports
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_text("identity_declaration_scope", self.scope_id.as_str())?;
        if self.operation_id.0.is_nil() {
            return Err(IdentityIntentError::NilOperationId);
        }
        if matches!(
            self.expected_scope,
            IdentityDeclarationScopePrecondition::Revision { revision: 0 }
        ) {
            return Err(IdentityIntentError::InvalidDeclarationRevision);
        }
        validate_sha256_digest(&self.request_digest)?;
        for (identity, member) in &self.members {
            validate_identity(identity)?;
            member.validate()?;
            if let Some(legacy_import) = self.legacy_imports.get(identity) {
                legacy_import.validate()?;
                if member.session_authority_policy != DesiredSessionAuthorityPolicy::RequireExisting
                    || member.initial_message.is_some()
                {
                    return Err(IdentityIntentError::InvalidLegacyImport);
                }
            }
        }
        if self
            .legacy_imports
            .keys()
            .any(|identity| !self.members.contains_key(identity))
        {
            return Err(IdentityIntentError::InvalidLegacyImport);
        }
        for edge in &self.wiring {
            if edge.a >= edge.b
                || !self.members.contains_key(&edge.a)
                || !self.members.contains_key(&edge.b)
            {
                return Err(IdentityIntentError::CrossScopeEdge(edge.clone()));
            }
        }
        Ok(())
    }
}

/// Canonical undirected desired edge.  The lexicographically smaller endpoint
/// is its sole intent owner, avoiding two independently mutable desired facts
/// for one physical edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredIdentityEdge {
    pub a: AgentIdentity,
    pub b: AgentIdentity,
}

impl DesiredIdentityEdge {
    pub fn new(left: AgentIdentity, right: AgentIdentity) -> Result<Self, IdentityIntentError> {
        if left == right {
            return Err(IdentityIntentError::SelfEdge(left));
        }
        let (a, b) = if left < right {
            (left, right)
        } else {
            (right, left)
        };
        Ok(Self { a, b })
    }

    #[must_use]
    pub fn owner(&self) -> &AgentIdentity {
        &self.a
    }
}

/// Optional compare-and-set boundary for a complete provider snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "precondition", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityDeclarationScopePrecondition {
    Any,
    Missing,
    Revision { revision: u64 },
}

/// One complete provider-scoped roster and edge declaration.
///
/// This is the high-level actor input.  Callers do not allocate sessions,
/// lineages, delivery ids, intent revisions, tombstones, or scope revisions.
/// The actor compiles member material, and one store transaction reuses the
/// current sealed target or allocates a new target only for a genuinely new
/// identity.  Every edge must remain inside the scope in schema v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityDeclarationManifest {
    pub scope_id: IdentityDeclarationScopeId,
    pub operation_id: OperationId,
    pub expected_scope: IdentityDeclarationScopePrecondition,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub members: BTreeMap<AgentIdentity, IdentityMemberDeclaration>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub wiring: BTreeSet<DesiredIdentityEdge>,
}

impl IdentityDeclarationManifest {
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_text("identity_declaration_scope", self.scope_id.as_str())?;
        if self.operation_id.0.is_nil() {
            return Err(IdentityIntentError::NilOperationId);
        }
        if matches!(
            self.expected_scope,
            IdentityDeclarationScopePrecondition::Revision { revision: 0 }
        ) {
            return Err(IdentityIntentError::InvalidDeclarationRevision);
        }
        for (identity, declaration) in &self.members {
            validate_identity(identity)?;
            declaration.validate()?;
        }
        for edge in &self.wiring {
            validate_identity(&edge.a)?;
            validate_identity(&edge.b)?;
            if edge.a >= edge.b {
                return Err(IdentityIntentError::NonCanonicalEdge(edge.clone()));
            }
            if !self.members.contains_key(&edge.a) || !self.members.contains_key(&edge.b) {
                return Err(IdentityIntentError::CrossScopeEdge(edge.clone()));
            }
        }
        let encoded = serde_json::to_vec(self)
            .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        if encoded.len() > IDENTITY_DECLARATION_MANIFEST_MAX_ENCODED_BYTES {
            return Err(IdentityIntentError::TooLarge {
                actual: encoded.len(),
                maximum: IDENTITY_DECLARATION_MANIFEST_MAX_ENCODED_BYTES,
            });
        }
        Ok(())
    }

    pub fn request_digest(&self) -> Result<String, IdentityIntentError> {
        self.validate()?;
        #[derive(Serialize)]
        struct DigestMaterial<'a> {
            domain: &'static str,
            manifest: &'a IdentityDeclarationManifest,
        }
        let bytes = serde_json::to_vec(&DigestMaterial {
            domain: "meerkat.identity.declaration_request.v1",
            manifest: self,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }
}

/// The only authoritative desired state for one stable identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "desired_presence",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum IdentityIntent {
    Present {
        identity: AgentIdentity,
        session: DesiredSessionTarget,
        member: Box<DesiredMemberSpec>,
        #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
        owned_wiring: BTreeSet<DesiredIdentityEdge>,
    },
    Absent {
        identity: AgentIdentity,
    },
}

impl IdentityIntent {
    #[must_use]
    pub fn identity(&self) -> &AgentIdentity {
        match self {
            Self::Present { identity, .. } | Self::Absent { identity } => identity,
        }
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_identity(self.identity())?;
        if let Self::Present {
            identity,
            session,
            member,
            owned_wiring,
        } = self
        {
            if session.session_id.0.is_nil() {
                return Err(IdentityIntentError::NilSessionId);
            }
            if matches!(
                session.authority_policy,
                DesiredSessionAuthorityPolicy::CreateIfAbsent
            ) && session.lineage_generation != SessionGeneration::INITIAL
            {
                return Err(IdentityIntentError::CreateRequiresInitialGeneration);
            }
            SessionLineageId::new(session.lineage_id.as_str().to_string()).map_err(|_| {
                IdentityIntentError::InvalidText {
                    field: "session_lineage_id",
                }
            })?;
            member.validate()?;
            for edge in owned_wiring {
                validate_identity(&edge.a)?;
                validate_identity(&edge.b)?;
                if edge.a >= edge.b {
                    return Err(IdentityIntentError::NonCanonicalEdge(edge.clone()));
                }
                if edge.owner() != identity {
                    return Err(IdentityIntentError::EdgeOwnedByDifferentIdentity {
                        identity: identity.clone(),
                        edge: edge.clone(),
                    });
                }
            }
        }
        let encoded = serde_json::to_vec(self)
            .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        if encoded.len() > IDENTITY_INTENT_MAX_ENCODED_BYTES {
            return Err(IdentityIntentError::TooLarge {
                actual: encoded.len(),
                maximum: IDENTITY_INTENT_MAX_ENCODED_BYTES,
            });
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, IdentityIntentError> {
        self.validate()?;
        let encoded = serde_json::to_vec(self)
            .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&encoded))
    }
}

/// Store-sealed cleanup targets retained when desired presence becomes
/// Absent. They are not a second desired owner: they are immutable retirement
/// evidence copied from the last Present intent so torn secondary indexes can
/// still be found and removed after a crash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cleanup", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityRetirementPlan {
    NoKnownRealization,
    Targets {
        session: DesiredSessionTarget,
        execution: DesiredExecution,
        /// Store-computed union of all desired edges incident to the identity,
        /// including edges owned lexicographically by another Present intent.
        incident_wiring: BTreeSet<DesiredIdentityEdge>,
    },
}

/// Store-sealed current desired row.  Absence is represented by an
/// `IdentityIntent::Absent` row, never by deleting this record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityIntentRecord {
    pub schema_version: u32,
    /// Physical mob authority scope. This is store-sealed alongside the
    /// desired row so moving otherwise-valid bytes under another mob key can
    /// never authorize that mob's cleanup or materialization.
    pub mob_id: MobId,
    pub intent_revision: u64,
    /// Provider scope/revision that last authored this row.  Both are absent
    /// only for explicitly migrated legacy authority and present together for
    /// a complete declaration manifest. They order writes; reconciliation
    /// never reads them as desired state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration_scope: Option<IdentityDeclarationScopeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration_revision: Option<u64>,
    /// Monotonic deletion high-water.  It is retained by later Present rows,
    /// so an old remote realization cannot resurrect across delete/recreate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tombstone_generation: Option<u64>,
    /// Monotonic one-shot delivery high-water. Removing the initial message
    /// retains this counter so a later re-arm gets a strict successor while
    /// unrelated material edits preserve the existing delivery identity.
    #[serde(default)]
    pub initial_delivery_generation_highwater: u64,
    pub retirement_plan: IdentityRetirementPlan,
    /// Digest of caller-authored desired content only, used for idempotent
    /// public apply comparisons.
    pub intent_digest: String,
    /// Digest of the complete store-sealed authority row (excluding itself).
    /// Cleanup never trusts `retirement_plan` without this exact seal.
    pub authority_digest: String,
    pub intent: IdentityIntent,
}

impl IdentityIntentRecord {
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        if self.schema_version != IDENTITY_INTENT_SCHEMA_VERSION {
            return Err(IdentityIntentError::UnsupportedSchemaVersion {
                record: "identity_intent",
                version: self.schema_version,
            });
        }
        validate_text("mob_id", self.mob_id.as_str())?;
        if self.intent_revision == 0 {
            return Err(IdentityIntentError::ZeroIntentRevision);
        }
        match (&self.declaration_scope, self.declaration_revision) {
            (Some(scope), Some(revision)) if revision > 0 => {
                validate_text("identity_declaration_scope", scope.as_str())?;
            }
            (None, None) => {}
            _ => return Err(IdentityIntentError::InvalidDeclarationRevision),
        }
        if self.tombstone_generation == Some(0)
            || (matches!(&self.intent, IdentityIntent::Absent { .. })
                && self.tombstone_generation.is_none())
        {
            return Err(IdentityIntentError::InvalidTombstoneGeneration);
        }
        match &self.intent {
            IdentityIntent::Present {
                member,
                session: _,
                identity: _,
                owned_wiring: _,
            } => {
                if let Some(delivery) = &member.initial_delivery {
                    if delivery.delivery_generation != self.initial_delivery_generation_highwater {
                        return Err(IdentityIntentError::InvalidInitialDelivery);
                    }
                }
                validate_retirement_plan(
                    &self.retirement_plan,
                    self.intent.identity(),
                    Some(&self.intent),
                )?;
            }
            IdentityIntent::Absent { .. } => {
                validate_retirement_plan(&self.retirement_plan, self.intent.identity(), None)?;
            }
        }
        let digest = self.intent.digest()?;
        if digest != self.intent_digest {
            return Err(IdentityIntentError::DigestMismatch);
        }
        if self.canonical_authority_digest()? != self.authority_digest {
            return Err(IdentityIntentError::DigestMismatch);
        }
        Ok(())
    }

    pub fn canonical_authority_digest(&self) -> Result<String, IdentityIntentError> {
        #[derive(Serialize)]
        struct AuthorityMaterial<'a> {
            domain: &'static str,
            schema_version: u32,
            mob_id: &'a MobId,
            intent_revision: u64,
            declaration_scope: &'a Option<IdentityDeclarationScopeId>,
            declaration_revision: Option<u64>,
            tombstone_generation: Option<u64>,
            initial_delivery_generation_highwater: u64,
            retirement_plan: &'a IdentityRetirementPlan,
            intent_digest: &'a str,
            intent: &'a IdentityIntent,
        }
        let bytes = serde_json::to_vec(&AuthorityMaterial {
            domain: "meerkat.identity.intent_authority.v1",
            schema_version: self.schema_version,
            mob_id: &self.mob_id,
            intent_revision: self.intent_revision,
            declaration_scope: &self.declaration_scope,
            declaration_revision: self.declaration_revision,
            tombstone_generation: self.tombstone_generation,
            initial_delivery_generation_highwater: self.initial_delivery_generation_highwater,
            retirement_plan: &self.retirement_plan,
            intent_digest: &self.intent_digest,
            intent: &self.intent,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityIntentApplyDisposition {
    Applied,
    Unchanged,
}

/// Exact original result retained for an operation id even after later
/// mutations advance the current desired row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityIntentApplyOutcome {
    pub disposition: IdentityIntentApplyDisposition,
    pub identity: AgentIdentity,
    pub intent_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration_scope: Option<IdentityDeclarationScopeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration_revision: Option<u64>,
    pub tombstone_generation: Option<u64>,
    pub initial_delivery_generation_highwater: u64,
    pub intent_digest: String,
    pub authority_digest: String,
    /// Exact store-sealed desired state, including the actor-owned session
    /// target and stable initial-delivery identity. Lost-ACK replay therefore
    /// never asks the caller to reconstruct allocation state.
    pub intent: IdentityIntent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityDeclarationManifestApplyDisposition {
    Applied,
    Unchanged,
}

/// Exact immutable result for one complete provider-scope apply operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityDeclarationManifestApplyOutcome {
    pub disposition: IdentityDeclarationManifestApplyDisposition,
    pub scope_id: IdentityDeclarationScopeId,
    pub scope_revision: u64,
    /// Digest of the caller declaration before actor compilation/allocation.
    pub request_digest: String,
    /// Digest of the exact sealed per-identity intent outcome map.
    pub compiled_manifest_digest: String,
    pub identities: BTreeMap<AgentIdentity, IdentityIntentApplyOutcome>,
}

impl IdentityDeclarationManifestApplyOutcome {
    pub fn canonical_compiled_manifest_digest(&self) -> Result<String, IdentityIntentError> {
        #[derive(Serialize)]
        struct DigestMaterial<'a> {
            domain: &'static str,
            scope_id: &'a IdentityDeclarationScopeId,
            scope_revision: u64,
            identities: &'a BTreeMap<AgentIdentity, IdentityIntentApplyOutcome>,
        }
        let bytes = serde_json::to_vec(&DigestMaterial {
            domain: "meerkat.identity.compiled_declaration_manifest.v1",
            scope_id: &self.scope_id,
            scope_revision: self.scope_revision,
            identities: &self.identities,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_text("identity_declaration_scope", self.scope_id.as_str())?;
        if self.scope_revision == 0 {
            return Err(IdentityIntentError::InvalidDeclarationRevision);
        }
        validate_sha256_digest(&self.request_digest)?;
        validate_sha256_digest(&self.compiled_manifest_digest)?;
        for (identity, outcome) in &self.identities {
            if identity != &outcome.identity
                || outcome.declaration_scope.as_ref() != Some(&self.scope_id)
                || outcome
                    .declaration_revision
                    .is_none_or(|revision| revision == 0 || revision > self.scope_revision)
                || outcome.intent.identity() != identity
                || outcome.intent_revision == 0
                || outcome.intent.digest()? != outcome.intent_digest
            {
                return Err(IdentityIntentError::InvalidDeclarationOutcome);
            }
        }
        if self.canonical_compiled_manifest_digest()? != self.compiled_manifest_digest {
            return Err(IdentityIntentError::DigestMismatch);
        }
        Ok(())
    }
}

/// Current exclusive reconcile claim.  `holder_id` names the logical
/// controller while `incarnation_id` distinguishes concurrent/restarted
/// processes.  A new incarnation may take over only after this bounded lease
/// expires, always at a strictly greater epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityLeaseClaim {
    pub holder_id: String,
    pub incarnation_id: String,
    pub epoch: u64,
    /// Store-sealed instant at which this bounded claim was acquired or last
    /// renewed. Callers never author lease timing evidence.
    pub renewed_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityLeaseRecord {
    pub schema_version: u32,
    pub epoch_highwater: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<IdentityLeaseClaim>,
}

impl IdentityLeaseRecord {
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        if self.schema_version != IDENTITY_LEASE_SCHEMA_VERSION {
            return Err(IdentityIntentError::UnsupportedSchemaVersion {
                record: "identity_lease",
                version: self.schema_version,
            });
        }
        if let Some(active) = &self.active {
            validate_text("holder_id", &active.holder_id)?;
            validate_text("incarnation_id", &active.incarnation_id)?;
            if active.epoch == 0 || active.epoch != self.epoch_highwater {
                return Err(IdentityIntentError::InvalidLeaseEpoch);
            }
            active
                .expires_at_ms
                .checked_sub(active.renewed_at_ms)
                .filter(|ttl_ms| *ttl_ms > 0 && *ttl_ms <= IDENTITY_LEASE_MAX_TTL_MS)
                .ok_or(IdentityIntentError::InvalidLeaseLifetime)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityLeaseClaimOutcome {
    Acquired(IdentityLeaseClaim),
    Renewed(IdentityLeaseClaim),
    HeldByOther(IdentityLeaseClaim),
}

/// Domain of an immutable identity operation receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityOperationKind {
    ApplyDeclarationManifest,
    SessionCreationConsumed,
    RetirementProven,
    ExternalBinding,
    InitialDelivery,
}

/// Scope of one immutable receipt. Manifest idempotency is provider-scoped;
/// actuator custody remains identity-scoped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "subject", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityOperationSubject {
    DeclarationScope {
        scope_id: IdentityDeclarationScopeId,
    },
    Identity {
        identity: AgentIdentity,
    },
}

/// Stable lookup slot for an immutable receipt. One-shot slots deliberately
/// exclude content, so a different payload for the same semantic slot is a
/// typed conflict rather than a second effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "slot", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityOperationSlot {
    ApplyDeclarationManifest {
        scope_id: IdentityDeclarationScopeId,
        mutation_id: OperationId,
    },
    SessionCreationConsumed {
        tombstone_generation: u64,
        session_id: SessionId,
        lineage_id: SessionLineageId,
        lineage_generation: SessionGeneration,
    },
    RetirementProven {
        tombstone_generation: u64,
    },
    ExternalBinding {
        tombstone_generation: u64,
        remote_signing_identity: WireTrustedPeerIdentity,
        controller_signing_identity: WireTrustedPeerIdentity,
    },
    InitialDelivery {
        tombstone_generation: u64,
        session_id: SessionId,
        lineage_id: SessionLineageId,
        lineage_generation: SessionGeneration,
        delivery_generation: u64,
    },
}

impl IdentityOperationSlot {
    #[must_use]
    pub const fn kind(&self) -> IdentityOperationKind {
        match self {
            Self::ApplyDeclarationManifest { .. } => {
                IdentityOperationKind::ApplyDeclarationManifest
            }
            Self::SessionCreationConsumed { .. } => IdentityOperationKind::SessionCreationConsumed,
            Self::RetirementProven { .. } => IdentityOperationKind::RetirementProven,
            Self::ExternalBinding { .. } => IdentityOperationKind::ExternalBinding,
            Self::InitialDelivery { .. } => IdentityOperationKind::InitialDelivery,
        }
    }
}

/// Immutable custody evidence. Lease epoch is intentionally absent from the
/// semantic payload: takeover must not invalidate an idempotency receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityOperationReceiptPayload {
    ApplyDeclarationManifest {
        outcome: IdentityDeclarationManifestApplyOutcome,
    },
    /// Sealed only after the first exact target session is observed. Once
    /// present, later absence is evidence loss and can never recreate history.
    SessionCreationConsumed {
        checkpoint: SessionCheckpointStamp,
    },
    RetirementProven {
        absent_authority_digest: String,
    },
    /// Lost-ACK custody for external trust materialization. No pairing secret
    /// or bootstrap token is retained.
    ExternalBinding {
        expected_address: DesiredExternalAddress,
        expected_identity: WireTrustedPeerIdentity,
        expected_controller_identity: WireTrustedPeerIdentity,
        ceremony_id: OperationId,
    },
    /// Stable one-shot input identity. Fresh transcript/input-state evidence,
    /// not a mutable phase field, proves whether delivery completed.
    InitialDelivery {
        delivery_generation: u64,
        delivery_id: InputId,
        message_digest: String,
    },
}

impl IdentityOperationReceiptPayload {
    #[must_use]
    pub const fn kind(&self) -> IdentityOperationKind {
        match self {
            Self::ApplyDeclarationManifest { .. } => {
                IdentityOperationKind::ApplyDeclarationManifest
            }
            Self::SessionCreationConsumed { .. } => IdentityOperationKind::SessionCreationConsumed,
            Self::RetirementProven { .. } => IdentityOperationKind::RetirementProven,
            Self::ExternalBinding { .. } => IdentityOperationKind::ExternalBinding,
            Self::InitialDelivery { .. } => IdentityOperationKind::InitialDelivery,
        }
    }
}

/// Insert-if-absent receipt, scoped to one mob and exact operation subject.
/// `audit_lease_epoch` records provenance only and is never consulted for
/// replay validity after lease takeover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityOperationReceipt {
    pub schema_version: u32,
    pub mob_id: MobId,
    pub subject: IdentityOperationSubject,
    pub effect_kind: IdentityOperationKind,
    pub slot: IdentityOperationSlot,
    /// Audit identity for this immutable insert. Retrieval and lost-ACK replay
    /// use `slot`, so a random internal id can never orphan custody.
    pub receipt_id: OperationId,
    /// Source intent authority is diagnostic provenance. One-shot receipt
    /// applicability is defined by the stable slot and payload, so unrelated
    /// intent changes do not invalidate custody.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_authority_digest: Option<String>,
    pub tombstone_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_lease_epoch: Option<u64>,
    pub request_digest: String,
    pub payload: IdentityOperationReceiptPayload,
}

impl IdentityOperationReceipt {
    pub fn canonical_request_digest(&self) -> Result<String, IdentityIntentError> {
        #[derive(Serialize)]
        struct DigestMaterial<'a> {
            domain: &'static str,
            mob_id: &'a MobId,
            subject: &'a IdentityOperationSubject,
            effect_kind: IdentityOperationKind,
            payload: ReceiptRequestMaterial<'a>,
        }

        #[derive(Serialize)]
        #[serde(tag = "operation", rename_all = "snake_case")]
        enum ReceiptRequestMaterial<'a> {
            ApplyDeclarationManifest {
                request_digest: &'a str,
            },
            SessionCreationConsumed {
                tombstone_generation: u64,
                checkpoint: &'a SessionCheckpointStamp,
            },
            RetirementProven {
                tombstone_generation: u64,
                absent_authority_digest: &'a str,
            },
            ExternalBinding {
                tombstone_generation: u64,
                expected_address: &'a DesiredExternalAddress,
                expected_identity: &'a WireTrustedPeerIdentity,
                expected_controller_identity: &'a WireTrustedPeerIdentity,
                ceremony_id: &'a OperationId,
            },
            InitialDelivery {
                tombstone_generation: u64,
                session_id: &'a SessionId,
                lineage_id: &'a SessionLineageId,
                lineage_generation: SessionGeneration,
                delivery_generation: u64,
                delivery_id: &'a InputId,
                message_digest: &'a str,
            },
        }

        let payload = match &self.payload {
            IdentityOperationReceiptPayload::ApplyDeclarationManifest { outcome } => {
                ReceiptRequestMaterial::ApplyDeclarationManifest {
                    request_digest: &outcome.request_digest,
                }
            }
            IdentityOperationReceiptPayload::SessionCreationConsumed { checkpoint } => {
                ReceiptRequestMaterial::SessionCreationConsumed {
                    tombstone_generation: self.tombstone_generation.unwrap_or(0),
                    checkpoint,
                }
            }
            IdentityOperationReceiptPayload::RetirementProven {
                absent_authority_digest,
            } => ReceiptRequestMaterial::RetirementProven {
                tombstone_generation: self.tombstone_generation.unwrap_or(0),
                absent_authority_digest,
            },
            IdentityOperationReceiptPayload::ExternalBinding {
                expected_address,
                expected_identity,
                expected_controller_identity,
                ceremony_id,
            } => ReceiptRequestMaterial::ExternalBinding {
                tombstone_generation: self.tombstone_generation.unwrap_or(0),
                expected_address,
                expected_identity,
                expected_controller_identity,
                ceremony_id,
            },
            IdentityOperationReceiptPayload::InitialDelivery {
                delivery_generation,
                delivery_id,
                message_digest,
            } => {
                let IdentityOperationSlot::InitialDelivery {
                    session_id,
                    lineage_id,
                    lineage_generation,
                    ..
                } = &self.slot
                else {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                };
                ReceiptRequestMaterial::InitialDelivery {
                    tombstone_generation: self.tombstone_generation.unwrap_or(0),
                    session_id,
                    lineage_id,
                    lineage_generation: *lineage_generation,
                    delivery_generation: *delivery_generation,
                    delivery_id,
                    message_digest,
                }
            }
        };
        let bytes = serde_json::to_vec(&DigestMaterial {
            domain: "meerkat.identity.operation_receipt.request.v1",
            mob_id: &self.mob_id,
            subject: &self.subject,
            effect_kind: self.effect_kind,
            payload,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        if self.schema_version != IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION {
            return Err(IdentityIntentError::UnsupportedSchemaVersion {
                record: "identity_operation_receipt",
                version: self.schema_version,
            });
        }
        validate_text("mob_id", self.mob_id.as_str())?;
        match &self.subject {
            IdentityOperationSubject::DeclarationScope { scope_id } => {
                validate_text("identity_declaration_scope", scope_id.as_str())?;
            }
            IdentityOperationSubject::Identity { identity } => validate_identity(identity)?,
        }
        if self.receipt_id.0.is_nil() {
            return Err(IdentityIntentError::InvalidOperationReceipt);
        }
        if self.tombstone_generation == Some(0) || self.audit_lease_epoch == Some(0) {
            return Err(IdentityIntentError::InvalidOperationReceipt);
        }
        if self.effect_kind != self.slot.kind() || self.effect_kind != self.payload.kind() {
            return Err(IdentityIntentError::InvalidOperationReceipt);
        }
        let normalized_tombstone = self.tombstone_generation.unwrap_or(0);
        match &self.payload {
            IdentityOperationReceiptPayload::ApplyDeclarationManifest { outcome } => {
                let (
                    IdentityOperationSubject::DeclarationScope {
                        scope_id: subject_scope,
                    },
                    IdentityOperationSlot::ApplyDeclarationManifest {
                        scope_id: slot_scope,
                        mutation_id,
                    },
                ) = (&self.subject, &self.slot)
                else {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                };
                if mutation_id != &self.receipt_id
                    || subject_scope != slot_scope
                    || subject_scope != &outcome.scope_id
                    || self.intent_revision.is_some()
                    || self.intent_digest.is_some()
                    || self.intent_authority_digest.is_some()
                    || self.tombstone_generation.is_some()
                    || self.audit_lease_epoch.is_some()
                {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                }
                outcome.validate()?;
            }
            IdentityOperationReceiptPayload::SessionCreationConsumed { checkpoint } => {
                validate_identity_receipt_authority(self)?;
                let IdentityOperationSlot::SessionCreationConsumed {
                    tombstone_generation,
                    session_id,
                    lineage_id,
                    lineage_generation,
                } = &self.slot
                else {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                };
                checkpoint
                    .validate_for_session(session_id)
                    .map_err(session_checkpoint_error)?;
                if *tombstone_generation != normalized_tombstone
                    || checkpoint.lineage_id() != lineage_id
                    || checkpoint.generation() != *lineage_generation
                {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                }
            }
            IdentityOperationReceiptPayload::RetirementProven {
                absent_authority_digest,
            } => {
                validate_identity_receipt_authority(self)?;
                let IdentityOperationSlot::RetirementProven {
                    tombstone_generation,
                } = &self.slot
                else {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                };
                if *tombstone_generation == 0 || *tombstone_generation != normalized_tombstone {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                }
                validate_sha256_digest(absent_authority_digest)?;
                if Some(absent_authority_digest) != self.intent_authority_digest.as_ref() {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                }
            }
            IdentityOperationReceiptPayload::ExternalBinding {
                expected_address: _,
                expected_identity,
                expected_controller_identity,
                ceremony_id,
            } => {
                validate_identity_receipt_authority(self)?;
                let IdentityOperationSlot::ExternalBinding {
                    tombstone_generation,
                    remote_signing_identity,
                    controller_signing_identity,
                } = &self.slot
                else {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                };
                if *tombstone_generation != normalized_tombstone
                    || remote_signing_identity != expected_identity
                    || controller_signing_identity != expected_controller_identity
                    || ceremony_id.0.is_nil()
                {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                }
                expected_identity.resolve().map_err(|error| {
                    IdentityIntentError::InvalidExternalIdentity(error.to_string())
                })?;
                expected_controller_identity.resolve().map_err(|error| {
                    IdentityIntentError::InvalidExternalIdentity(error.to_string())
                })?;
            }
            IdentityOperationReceiptPayload::InitialDelivery {
                delivery_generation,
                delivery_id,
                message_digest,
            } => {
                validate_identity_receipt_authority(self)?;
                let IdentityOperationSlot::InitialDelivery {
                    tombstone_generation,
                    delivery_generation: slot_generation,
                    ..
                } = &self.slot
                else {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                };
                if *tombstone_generation != normalized_tombstone
                    || *delivery_generation == 0
                    || delivery_generation != slot_generation
                    || delivery_id.0.is_nil()
                {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                }
                validate_sha256_digest(message_digest)?;
            }
        }
        if self.canonical_request_digest()? != self.request_digest {
            return Err(IdentityIntentError::DigestMismatch);
        }
        Ok(())
    }
}

fn validate_identity_receipt_authority(
    receipt: &IdentityOperationReceipt,
) -> Result<(), IdentityIntentError> {
    if !matches!(&receipt.subject, IdentityOperationSubject::Identity { .. })
        || receipt.intent_revision.is_none_or(|revision| revision == 0)
    {
        return Err(IdentityIntentError::InvalidOperationReceipt);
    }
    let Some(intent_digest) = &receipt.intent_digest else {
        return Err(IdentityIntentError::InvalidOperationReceipt);
    };
    let Some(authority_digest) = &receipt.intent_authority_digest else {
        return Err(IdentityIntentError::InvalidOperationReceipt);
    };
    validate_sha256_digest(intent_digest)?;
    validate_sha256_digest(authority_digest)
}

/// Total classification of one durable identity-store row. Transport/I/O
/// faults remain the store method's `Err` (temporarily unavailable); every
/// decodable physical row is represented here instead of being collapsed to
/// absence or a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityStoredObservation<T> {
    Missing,
    Valid(T),
    Unsupported {
        evidence_digest: String,
        detail: String,
    },
    Malformed {
        evidence_digest: String,
        detail: String,
    },
}

impl<T> IdentityStoredObservation<T> {
    pub fn validate_evidence(&self) -> Result<(), IdentityIntentError> {
        match self {
            Self::Unsupported {
                evidence_digest, ..
            }
            | Self::Malformed {
                evidence_digest, ..
            } => validate_sha256_digest(evidence_digest),
            Self::Missing | Self::Valid(_) => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityOperationReceiptInsertOutcome {
    Inserted(IdentityOperationReceipt),
    ExistingExact(IdentityOperationReceipt),
    Conflict(IdentityOperationReceipt),
}

/// Lossless classification of a store read.  Missing, malformed, and
/// unavailable are distinct inputs to the total reconciler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityAuthorityCondition {
    Unavailable,
    Missing,
    Malformed,
    PresentCreateIfAbsent,
    PresentRequireExisting,
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityLeaseCondition {
    Unavailable,
    Missing,
    Malformed,
    HeldByCurrentIncarnation,
    HeldByOtherLiveIncarnation,
    HeldByExpiredIncarnation,
}

/// Resource observation condition. `Divergent` means decodable, attributable
/// to this identity, and proved safe to replace under the target-local CAS.
/// Undecodable or ownership-ambiguous evidence is `Malformed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityResourceCondition {
    Unavailable,
    Missing,
    Matching,
    Divergent,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentitySessionCondition {
    Unavailable,
    Missing,
    Matching,
    RecoverableDivergence,
    AmbiguousDivergence,
    Malformed,
    IrrecoverablyCorrupt,
}

/// Lossless read of one immutable operation receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityReceiptCondition {
    NotRequired,
    Unavailable,
    Missing,
    Matching,
    Conflicting,
    Malformed,
}

/// External trust materialization evidence. A missing binding does not itself
/// authorize replay of a one-time bootstrap token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityExternalTrustCondition {
    NotRequired,
    Unavailable,
    Matching,
    Absent,
    Contradictory,
    Indeterminate,
    Malformed,
}

/// Ephemeral ceremony authority is observed independently from durable trust
/// and receipt state. It is never persisted in intent or receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityExternalCeremonyCondition {
    NotRequired,
    FreshAvailable,
    TemporarilyUnavailable,
    AwaitFresh,
    SpentOrUnknown,
}

/// One-shot initial-delivery state, joined from the immutable custody receipt
/// and fresh input/transcript evidence for its stable `InputId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityInitialDeliveryCondition {
    NotRequired,
    Unavailable,
    ProvenAbsent,
    AcceptedPendingExact,
    CommittedExact,
    ContentOnlyMatch,
    OperationCollision,
    Contradictory,
    Indeterminate,
    Malformed,
}

/// Minimal facts consumed by the pure generated classifier.  Observation
/// versions remain beside the actor's resource-local actuator permit and do
/// not become a universal witness here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityReconcileFacts {
    pub intent: IdentityAuthorityCondition,
    pub lease: IdentityLeaseCondition,
    pub external_binding_required: bool,
    pub initial_delivery_required: bool,
    pub session_creation_receipt: IdentityReceiptCondition,
    pub retirement_receipt: IdentityReceiptCondition,
    pub session: IdentitySessionCondition,
    pub runtime: IdentityResourceCondition,
    pub member: IdentityResourceCondition,
    pub external_binding_receipt: IdentityReceiptCondition,
    pub external_trust: IdentityExternalTrustCondition,
    pub external_ceremony: IdentityExternalCeremonyCondition,
    pub initial_delivery_receipt: IdentityReceiptCondition,
    pub initial_delivery: IdentityInitialDeliveryCondition,
    pub wiring: IdentityResourceCondition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityReconcileDecision {
    Backoff,
    RepairBlocked,
    AcquireLease,
    AwaitLease,
    SealRetirementProven,
    SealSessionCreationConsumed,
    EnsureSessionAuthority,
    EnsureRuntimeRegistration,
    AwaitExternalBindingCeremony,
    EnsureExternalBindingReceipt,
    EnsureExternalBinding,
    EnsureMemberMaterialization,
    EnsureInitialDeliveryReceipt,
    EnsureInitialDelivery,
    AwaitInitialDelivery,
    ReconcileWiring,
    RetireMemberMaterialization,
    RetireRuntimeRegistration,
    ReleaseSessionAuthority,
    Converged,
    Tombstoned,
    Quarantined,
}

/// Total, level-triggered classifier generated from the canonical MobMachine
/// DSL helper. This function is only a typed carrier adapter; there is no
/// handwritten sibling decision tree to drift from the executable kernel.
#[must_use]
pub fn classify_identity_reconciliation(
    facts: IdentityReconcileFacts,
) -> IdentityReconcileDecision {
    crate::machines::mob_machine::generated_identity_reconcile_decision(facts)
}

/// Opaque target-local CAS precondition. Missing state still has a real
/// absence witness; an unversioned absence can never authorize creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityTargetObservationVersion {
    /// The target is an immutable operation slot whose primary-key
    /// insert-if-absent transaction is the target-local CAS. No separately
    /// observed absence token is authoritative for this shape.
    InsertIfAbsent,
    Absent {
        absence_version: String,
    },
    Version {
        version: String,
    },
}

impl IdentityTargetObservationVersion {
    fn validate(&self) -> Result<(), IdentityIntentError> {
        let value = match self {
            Self::InsertIfAbsent => return Ok(()),
            Self::Absent { absence_version } => absence_version,
            Self::Version { version } => version,
        };
        if value.is_empty() {
            return Err(IdentityIntentError::InvalidObservationVersion);
        }
        Ok(())
    }
}

/// Resource observation whose shape carries the correct target-local CAS
/// witness for every actuatable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityResourceObservation {
    Unavailable {
        detail: String,
    },
    Missing {
        absence_version: String,
    },
    Matching {
        version: String,
    },
    Divergent {
        version: String,
        detail: String,
    },
    Malformed {
        observed_version: Option<String>,
        detail: String,
    },
}

impl IdentityResourceObservation {
    #[must_use]
    pub const fn condition(&self) -> IdentityResourceCondition {
        match self {
            Self::Unavailable { .. } => IdentityResourceCondition::Unavailable,
            Self::Missing { .. } => IdentityResourceCondition::Missing,
            Self::Matching { .. } => IdentityResourceCondition::Matching,
            Self::Divergent { .. } => IdentityResourceCondition::Divergent,
            Self::Malformed { .. } => IdentityResourceCondition::Malformed,
        }
    }

    pub fn target_precondition(
        &self,
    ) -> Result<Option<IdentityTargetObservationVersion>, IdentityIntentError> {
        let precondition = match self {
            Self::Missing { absence_version } => Some(IdentityTargetObservationVersion::Absent {
                absence_version: absence_version.clone(),
            }),
            Self::Matching { version } | Self::Divergent { version, .. } => {
                Some(IdentityTargetObservationVersion::Version {
                    version: version.clone(),
                })
            }
            Self::Unavailable { .. } | Self::Malformed { .. } => None,
        };
        if let Some(precondition) = &precondition {
            precondition.validate()?;
        }
        Ok(precondition)
    }
}

/// Full-document-verified checkpoint identity. Private fields ensure callers
/// cannot label a partial or digest-unverified stamp as Matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedIdentitySessionCheckpoint {
    stamp: SessionCheckpointStamp,
}

impl VerifiedIdentitySessionCheckpoint {
    #[must_use]
    pub fn stamp(&self) -> &SessionCheckpointStamp {
        &self.stamp
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IdentitySessionObservationState {
    Unavailable {
        detail: String,
    },
    Missing {
        absence_version: String,
    },
    Matching {
        checkpoint: VerifiedIdentitySessionCheckpoint,
        version: String,
    },
    RecoverableDivergence {
        checkpoint: VerifiedIdentitySessionCheckpoint,
        target: IdentityTargetObservationVersion,
        detail: String,
    },
    AmbiguousDivergence {
        evidence_digest: String,
        target: IdentityTargetObservationVersion,
        detail: String,
    },
    Malformed {
        evidence_digest: String,
        version: String,
        detail: String,
    },
    /// Persisted session evidence failed typed decoding before the observer
    /// could obtain a trustworthy target-local CAS version. This may classify
    /// the identity as malformed, but it can never authorize a session write.
    MalformedUnversioned {
        detail: String,
    },
    IrrecoverablyCorrupt {
        evidence_digest: String,
        version: String,
        detail: String,
    },
}

/// Proof-bearing session/transcript observation. The generated classifier
/// receives only [`IdentitySessionCondition`]; the actor retains this exact
/// target-local witness to mint a resource-scoped permit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySessionObservation {
    state: IdentitySessionObservationState,
}

impl IdentitySessionObservation {
    pub fn matching(
        desired: &DesiredSessionTarget,
        canonical: &Session,
        projection: &Session,
        version: String,
    ) -> Result<Self, IdentityIntentError> {
        let canonical_stamp = verified_checkpoint_for_desired(desired, canonical)?;
        let projection_stamp = verified_checkpoint_for_desired(desired, projection)?;
        if session_checkpoint_relation(canonical, projection).map_err(session_checkpoint_error)?
            != SessionCheckpointRelation::Exact
            || canonical_stamp != projection_stamp
        {
            return Err(IdentityIntentError::AmbiguousSessionCheckpoint);
        }
        IdentityTargetObservationVersion::Version {
            version: version.clone(),
        }
        .validate()?;
        Ok(Self {
            state: IdentitySessionObservationState::Matching {
                checkpoint: VerifiedIdentitySessionCheckpoint {
                    stamp: canonical_stamp,
                },
                version,
            },
        })
    }

    pub fn recoverable_missing_target(
        desired: &DesiredSessionTarget,
        canonical: &Session,
        absence_version: String,
    ) -> Result<Self, IdentityIntentError> {
        let stamp = verified_checkpoint_for_desired(desired, canonical)?;
        let target = IdentityTargetObservationVersion::Absent { absence_version };
        target.validate()?;
        Ok(Self {
            state: IdentitySessionObservationState::RecoverableDivergence {
                checkpoint: VerifiedIdentitySessionCheckpoint { stamp },
                target,
                detail: "verified canonical checkpoint can be projected into proved absence"
                    .to_string(),
            },
        })
    }

    /// Construct a repairable typed mismatch only when exact checkpoint
    /// ancestry proves forward projection, intra-turn rollback, or legacy
    /// migration. Every other decodable mismatch stays ambiguous.
    pub fn recoverable_divergence(
        desired: &DesiredSessionTarget,
        canonical: &Session,
        projection: &Session,
        projection_source_blob: Option<&[u8]>,
        ancestry: Option<&SessionCheckpointAncestryProof>,
        version: String,
    ) -> Result<Self, IdentityIntentError> {
        let canonical_stamp = verified_checkpoint_for_desired(desired, canonical)?;
        let relation =
            session_checkpoint_relation(canonical, projection).map_err(session_checkpoint_error)?;
        let recoverable = match projection
            .try_checkpoint_state()
            .map_err(session_checkpoint_error)?
        {
            SessionCheckpointState::Verified(projection_stamp) => match relation {
                SessionCheckpointRelation::LeftRevisionNewer => {
                    checkpoint_base_names(&canonical_stamp, &projection_stamp)
                        || ancestry
                            .is_some_and(|proof| proof.proves(&projection_stamp, &canonical_stamp))
                }
                SessionCheckpointRelation::LeftRevisionOlder => {
                    projection_stamp.provenance()
                        == SessionCheckpointProvenance::IntraTurnCheckpoint
                        && checkpoint_base_names(&projection_stamp, &canonical_stamp)
                }
                _ => false,
            },
            SessionCheckpointState::LegacyUnverified { .. } => {
                matches!(relation, SessionCheckpointRelation::RightLegacyUnverified)
                    && canonical_stamp.provenance()
                        == SessionCheckpointProvenance::RecoveryMigration
                    && projection_source_blob
                        .and_then(|source_blob| {
                            SessionCheckpointStamp::recovery_migration(
                                projection,
                                source_blob,
                                canonical_stamp.generation(),
                                canonical_stamp.checkpoint_revision(),
                            )
                            .ok()
                        })
                        .is_some_and(|expected| expected == canonical_stamp)
            }
        };
        if !recoverable {
            return Err(IdentityIntentError::AmbiguousSessionCheckpoint);
        }
        let target = IdentityTargetObservationVersion::Version { version };
        target.validate()?;
        Ok(Self {
            state: IdentitySessionObservationState::RecoverableDivergence {
                checkpoint: VerifiedIdentitySessionCheckpoint {
                    stamp: canonical_stamp,
                },
                target,
                detail: format!("proved checkpoint repair relation: {relation:?}"),
            },
        })
    }

    pub fn missing(absence_version: String) -> Result<Self, IdentityIntentError> {
        IdentityTargetObservationVersion::Absent {
            absence_version: absence_version.clone(),
        }
        .validate()?;
        Ok(Self {
            state: IdentitySessionObservationState::Missing { absence_version },
        })
    }

    #[must_use]
    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            state: IdentitySessionObservationState::Unavailable {
                detail: detail.into(),
            },
        }
    }

    pub fn ambiguous_divergence(
        evidence_digest: String,
        target: IdentityTargetObservationVersion,
        detail: impl Into<String>,
    ) -> Result<Self, IdentityIntentError> {
        validate_sha256_digest(&evidence_digest)?;
        target.validate()?;
        Ok(Self {
            state: IdentitySessionObservationState::AmbiguousDivergence {
                evidence_digest,
                target,
                detail: detail.into(),
            },
        })
    }

    pub fn malformed(
        evidence_digest: String,
        version: String,
        detail: impl Into<String>,
    ) -> Result<Self, IdentityIntentError> {
        validate_sha256_digest(&evidence_digest)?;
        IdentityTargetObservationVersion::Version {
            version: version.clone(),
        }
        .validate()?;
        Ok(Self {
            state: IdentitySessionObservationState::Malformed {
                evidence_digest,
                version,
                detail: detail.into(),
            },
        })
    }

    /// Record typed persisted corruption when no trustworthy target-local CAS
    /// version can be recovered from the failed read.
    ///
    /// This shape deliberately carries neither a fabricated evidence digest
    /// nor a target precondition. It can drive only the generated
    /// `Malformed` classification and diagnostic output.
    pub fn malformed_unversioned(detail: impl Into<String>) -> Result<Self, IdentityIntentError> {
        let detail = detail.into();
        validate_text("identity_session_malformed_detail", &detail)?;
        Ok(Self {
            state: IdentitySessionObservationState::MalformedUnversioned { detail },
        })
    }

    /// Match the typed unversioned persisted-corruption observation without
    /// exposing the private observation-state representation.
    #[must_use]
    pub fn malformed_unversioned_detail(&self) -> Option<&str> {
        match &self.state {
            IdentitySessionObservationState::MalformedUnversioned { detail } => Some(detail),
            _ => None,
        }
    }

    pub fn irrecoverably_corrupt(
        evidence_digest: String,
        version: String,
        detail: impl Into<String>,
    ) -> Result<Self, IdentityIntentError> {
        validate_sha256_digest(&evidence_digest)?;
        IdentityTargetObservationVersion::Version {
            version: version.clone(),
        }
        .validate()?;
        Ok(Self {
            state: IdentitySessionObservationState::IrrecoverablyCorrupt {
                evidence_digest,
                version,
                detail: detail.into(),
            },
        })
    }

    #[must_use]
    pub const fn condition(&self) -> IdentitySessionCondition {
        match &self.state {
            IdentitySessionObservationState::Unavailable { .. } => {
                IdentitySessionCondition::Unavailable
            }
            IdentitySessionObservationState::Missing { .. } => IdentitySessionCondition::Missing,
            IdentitySessionObservationState::Matching { .. } => IdentitySessionCondition::Matching,
            IdentitySessionObservationState::RecoverableDivergence { .. } => {
                IdentitySessionCondition::RecoverableDivergence
            }
            IdentitySessionObservationState::AmbiguousDivergence { .. } => {
                IdentitySessionCondition::AmbiguousDivergence
            }
            IdentitySessionObservationState::Malformed { .. } => {
                IdentitySessionCondition::Malformed
            }
            IdentitySessionObservationState::MalformedUnversioned { .. } => {
                IdentitySessionCondition::Malformed
            }
            IdentitySessionObservationState::IrrecoverablyCorrupt { .. } => {
                IdentitySessionCondition::IrrecoverablyCorrupt
            }
        }
    }

    pub fn target_precondition(
        &self,
    ) -> Result<Option<IdentityTargetObservationVersion>, IdentityIntentError> {
        let target = match &self.state {
            IdentitySessionObservationState::Missing { absence_version } => {
                Some(IdentityTargetObservationVersion::Absent {
                    absence_version: absence_version.clone(),
                })
            }
            IdentitySessionObservationState::Matching { version, .. }
            | IdentitySessionObservationState::Malformed { version, .. }
            | IdentitySessionObservationState::IrrecoverablyCorrupt { version, .. } => {
                Some(IdentityTargetObservationVersion::Version {
                    version: version.clone(),
                })
            }
            IdentitySessionObservationState::RecoverableDivergence { target, .. }
            | IdentitySessionObservationState::AmbiguousDivergence { target, .. } => {
                Some(target.clone())
            }
            IdentitySessionObservationState::Unavailable { .. }
            | IdentitySessionObservationState::MalformedUnversioned { .. } => None,
        };
        if let Some(target) = &target {
            target.validate()?;
        }
        Ok(target)
    }

    /// Return the full-document-verified checkpoint carried by a matching or
    /// proved-recoverable observation. Receipt creation uses this exact stamp;
    /// ownership lease data never enters the checkpoint witness.
    #[must_use]
    pub fn verified_checkpoint(&self) -> Option<&SessionCheckpointStamp> {
        match &self.state {
            IdentitySessionObservationState::Matching { checkpoint, .. }
            | IdentitySessionObservationState::RecoverableDivergence { checkpoint, .. } => {
                Some(checkpoint.stamp())
            }
            IdentitySessionObservationState::Unavailable { .. }
            | IdentitySessionObservationState::Missing { .. }
            | IdentitySessionObservationState::AmbiguousDivergence { .. }
            | IdentitySessionObservationState::Malformed { .. }
            | IdentitySessionObservationState::MalformedUnversioned { .. }
            | IdentitySessionObservationState::IrrecoverablyCorrupt { .. } => None,
        }
    }
}

fn verified_checkpoint_for_desired(
    desired: &DesiredSessionTarget,
    session: &Session,
) -> Result<SessionCheckpointStamp, IdentityIntentError> {
    let stamp = match session
        .try_checkpoint_state()
        .map_err(session_checkpoint_error)?
    {
        SessionCheckpointState::Verified(stamp) => stamp,
        SessionCheckpointState::LegacyUnverified { .. } => {
            return Err(IdentityIntentError::AmbiguousSessionCheckpoint);
        }
    };
    if stamp.session_id() != &desired.session_id
        || stamp.lineage_id() != &desired.lineage_id
        || stamp.generation() != desired.lineage_generation
    {
        return Err(IdentityIntentError::AmbiguousSessionCheckpoint);
    }
    Ok(stamp)
}

fn checkpoint_base_names(child: &SessionCheckpointStamp, parent: &SessionCheckpointStamp) -> bool {
    matches!(
        child.authority_base(),
        SessionCheckpointAuthorityBase::Typed { anchor }
            if anchor.session_id == *parent.session_id()
                && anchor.lineage_id == *parent.lineage_id()
                && anchor.generation == parent.generation()
                && anchor.checkpoint_revision == parent.checkpoint_revision()
                && anchor.digest == *parent.digest()
                && anchor.provenance == parent.provenance()
    )
}

fn session_checkpoint_error(error: impl fmt::Display) -> IdentityIntentError {
    IdentityIntentError::SessionCheckpoint(error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityActuatorTarget {
    Session,
    Runtime,
    SessionCreationReceipt,
    RetirementReceipt,
    ExternalBindingReceipt,
    ExternalBinding,
    Member,
    InitialDeliveryReceipt,
    InitialDelivery,
    Wiring,
}

/// Ephemeral authority for exactly one target resource write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityActuationPermit {
    pub mob_id: MobId,
    pub identity: AgentIdentity,
    pub target: IdentityActuatorTarget,
    pub intent_revision: u64,
    pub intent_digest: String,
    pub intent_authority_digest: String,
    pub lease_epoch: u64,
    pub lease_holder_id: String,
    pub lease_incarnation_id: String,
    pub lease_expires_at_ms: u64,
    pub target_observation: IdentityTargetObservationVersion,
}

impl IdentityActuationPermit {
    /// Validate the self-contained half of the permit. The target writer must
    /// perform one atomic CAS that checks this scope, the exact current intent
    /// revision, the exact current lease epoch/incarnation and unexpired
    /// deadline, plus the target-local observation.
    pub fn validate_for_write(&self, observed_at_ms: u64) -> Result<(), IdentityIntentError> {
        validate_text("mob_id", self.mob_id.as_str())?;
        validate_identity(&self.identity)?;
        validate_text("lease_holder_id", &self.lease_holder_id)?;
        validate_text("lease_incarnation_id", &self.lease_incarnation_id)?;
        if self.intent_revision == 0 || self.lease_epoch == 0 {
            return Err(IdentityIntentError::InvalidActuationPermit);
        }
        validate_sha256_digest(&self.intent_digest)?;
        validate_sha256_digest(&self.intent_authority_digest)?;
        if observed_at_ms >= self.lease_expires_at_ms {
            return Err(IdentityIntentError::ExpiredActuationPermit);
        }
        self.target_observation.validate()?;
        let receipt_target = matches!(
            self.target,
            IdentityActuatorTarget::SessionCreationReceipt
                | IdentityActuatorTarget::RetirementReceipt
                | IdentityActuatorTarget::ExternalBindingReceipt
                | IdentityActuatorTarget::InitialDeliveryReceipt
        );
        if receipt_target
            != matches!(
                self.target_observation,
                IdentityTargetObservationVersion::InsertIfAbsent
            )
        {
            return Err(IdentityIntentError::InvalidActuationPermit);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityConvergenceCondition {
    Pending,
    Reconciling,
    Converged,
    Backoff,
    RepairBlocked,
    Quarantined,
    Tombstoned,
    Suspended,
}

/// Replaceable, output-only diagnostic.  It is never read by the classifier
/// and never grants an actuator permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConvergenceStatus {
    pub identity: AgentIdentity,
    pub intent_revision: Option<u64>,
    pub lease_epoch: Option<u64>,
    pub decision: Option<IdentityReconcileDecision>,
    pub observed_at_ms: u64,
    pub detail: Option<String>,
}

impl IdentityConvergenceStatus {
    /// Output condition is mechanically derived from the latest decision, so
    /// replaceable diagnostics cannot contradict the pure classifier.
    #[must_use]
    pub const fn condition(&self) -> IdentityConvergenceCondition {
        match self.decision {
            None => IdentityConvergenceCondition::Pending,
            Some(IdentityReconcileDecision::Backoff) => IdentityConvergenceCondition::Backoff,
            Some(IdentityReconcileDecision::RepairBlocked) => {
                IdentityConvergenceCondition::RepairBlocked
            }
            Some(
                IdentityReconcileDecision::AwaitLease
                | IdentityReconcileDecision::AwaitExternalBindingCeremony
                | IdentityReconcileDecision::AwaitInitialDelivery,
            ) => IdentityConvergenceCondition::Suspended,
            Some(IdentityReconcileDecision::Converged) => IdentityConvergenceCondition::Converged,
            Some(IdentityReconcileDecision::Tombstoned) => IdentityConvergenceCondition::Tombstoned,
            Some(IdentityReconcileDecision::Quarantined) => {
                IdentityConvergenceCondition::Quarantined
            }
            Some(
                IdentityReconcileDecision::AcquireLease
                | IdentityReconcileDecision::SealRetirementProven
                | IdentityReconcileDecision::SealSessionCreationConsumed
                | IdentityReconcileDecision::EnsureSessionAuthority
                | IdentityReconcileDecision::EnsureRuntimeRegistration
                | IdentityReconcileDecision::EnsureExternalBindingReceipt
                | IdentityReconcileDecision::EnsureExternalBinding
                | IdentityReconcileDecision::EnsureMemberMaterialization
                | IdentityReconcileDecision::EnsureInitialDeliveryReceipt
                | IdentityReconcileDecision::EnsureInitialDelivery
                | IdentityReconcileDecision::ReconcileWiring
                | IdentityReconcileDecision::RetireMemberMaterialization
                | IdentityReconcileDecision::RetireRuntimeRegistration
                | IdentityReconcileDecision::ReleaseSessionAuthority,
            ) => IdentityConvergenceCondition::Reconciling,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityIntentError {
    InvalidText {
        field: &'static str,
    },
    NilOperationId,
    NilSessionId,
    CreateRequiresInitialGeneration,
    InvalidExternalAddress(String),
    InvalidExternalIdentity(String),
    SelfEdge(AgentIdentity),
    NonCanonicalEdge(DesiredIdentityEdge),
    CrossScopeEdge(DesiredIdentityEdge),
    EdgeOwnedByDifferentIdentity {
        identity: AgentIdentity,
        edge: DesiredIdentityEdge,
    },
    TooLarge {
        actual: usize,
        maximum: usize,
    },
    Serialization(String),
    UnsupportedSchemaVersion {
        record: &'static str,
        version: u32,
    },
    ZeroIntentRevision,
    InvalidDeclarationRevision,
    InvalidDeclarationScopeHead,
    InvalidDeclarationOutcome,
    InvalidLegacyImport,
    InvalidMemberMaterial(String),
    DigestMismatch,
    InvalidLeaseEpoch,
    InvalidLeaseLifetime,
    InvalidTombstoneGeneration,
    InvalidInitialDelivery,
    InvalidRetirementPlan,
    InvalidOperationReceipt,
    InvalidObservationVersion,
    InvalidActuationPermit,
    ExpiredActuationPermit,
    AmbiguousSessionCheckpoint,
    SessionCheckpoint(String),
    CounterExhausted {
        counter: &'static str,
    },
}

impl fmt::Display for IdentityIntentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidText { field } => {
                write!(formatter, "{field} must be nonempty canonical text")
            }
            Self::NilOperationId => {
                formatter.write_str("identity intent operation id must be non-nil")
            }
            Self::NilSessionId => formatter.write_str("identity intent session id must be non-nil"),
            Self::CreateRequiresInitialGeneration => formatter
                .write_str("CreateIfAbsent requires the initial session lineage generation"),
            Self::InvalidExternalAddress(detail) => {
                write!(formatter, "invalid desired external address: {detail}")
            }
            Self::InvalidExternalIdentity(detail) => {
                write!(
                    formatter,
                    "invalid desired external signing identity: {detail}"
                )
            }
            Self::SelfEdge(identity) => {
                write!(formatter, "identity '{identity}' cannot wire to itself")
            }
            Self::NonCanonicalEdge(edge) => {
                write!(formatter, "desired wiring edge is not canonical: {edge:?}")
            }
            Self::CrossScopeEdge(edge) => write!(
                formatter,
                "identity declaration wiring edge {edge:?} crosses the provider scope"
            ),
            Self::EdgeOwnedByDifferentIdentity { identity, edge } => write!(
                formatter,
                "identity '{identity}' cannot own desired wiring edge {edge:?}; owner is '{}'",
                edge.owner()
            ),
            Self::TooLarge { actual, maximum } => write!(
                formatter,
                "identity intent is {actual} bytes; maximum is {maximum}"
            ),
            Self::Serialization(detail) => {
                write!(formatter, "identity intent serialization failed: {detail}")
            }
            Self::UnsupportedSchemaVersion { record, version } => {
                write!(formatter, "unsupported {record} schema version {version}")
            }
            Self::ZeroIntentRevision => {
                formatter.write_str("identity intent revision must be nonzero")
            }
            Self::InvalidDeclarationRevision => formatter.write_str(
                "identity declaration scope and nonzero revision must be present together",
            ),
            Self::InvalidDeclarationScopeHead => formatter.write_str(
                "identity declaration scope head is internally incoherent",
            ),
            Self::InvalidDeclarationOutcome => formatter.write_str(
                "identity declaration apply outcome is not bound to the sealed scope revision",
            ),
            Self::InvalidLegacyImport => formatter.write_str(
                "legacy identity adoption requires an exact RecoveryMigration checkpoint, RequireExisting target, no initial delivery, and a reclaimable lease high-water",
            ),
            Self::InvalidMemberMaterial(detail) => {
                write!(formatter, "invalid desired member material: {detail}")
            }
            Self::DigestMismatch => {
                formatter.write_str("identity intent digest does not match content")
            }
            Self::InvalidLeaseEpoch => formatter
                .write_str("identity lease active epoch must equal a nonzero epoch highwater"),
            Self::InvalidLeaseLifetime => write!(
                formatter,
                "identity lease lifetime must be between 1 and {IDENTITY_LEASE_MAX_TTL_MS}ms"
            ),
            Self::InvalidTombstoneGeneration => formatter.write_str(
                "absent identity intent requires a nonzero monotonic tombstone generation",
            ),
            Self::InvalidInitialDelivery => formatter.write_str(
                "initial delivery must have a nonzero generation, stable input id, and matching message digest",
            ),
            Self::InvalidRetirementPlan => formatter.write_str(
                "identity retirement plan must retain the store-sealed cleanup targets",
            ),
            Self::InvalidOperationReceipt => {
                formatter.write_str("identity operation receipt is internally incoherent")
            }
            Self::InvalidObservationVersion => {
                formatter.write_str("identity target observation version must be nonempty")
            }
            Self::InvalidActuationPermit => {
                formatter.write_str("identity actuation permit is internally incoherent")
            }
            Self::ExpiredActuationPermit => {
                formatter.write_str("identity actuation permit lease has expired")
            }
            Self::AmbiguousSessionCheckpoint => formatter.write_str(
                "session checkpoint evidence does not prove exact coherence or a safe repair relation",
            ),
            Self::SessionCheckpoint(detail) => {
                write!(formatter, "session checkpoint validation failed: {detail}")
            }
            Self::CounterExhausted { counter } => {
                write!(formatter, "identity {counter} counter exhausted")
            }
        }
    }
}

impl std::error::Error for IdentityIntentError {}

fn validate_identity(identity: &AgentIdentity) -> Result<(), IdentityIntentError> {
    validate_text("identity", identity.as_str())
}

fn validate_text(field: &'static str, value: &str) -> Result<(), IdentityIntentError> {
    if value.is_empty() || value.trim() != value {
        Err(IdentityIntentError::InvalidText { field })
    } else {
        Ok(())
    }
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn canonical_initial_message_digest(message: &ContentInput) -> Result<String, IdentityIntentError> {
    #[derive(Serialize)]
    struct DigestMaterial<'a> {
        domain: &'static str,
        message: &'a ContentInput,
    }
    let bytes = serde_json::to_vec(&DigestMaterial {
        domain: "meerkat.identity.initial_delivery.message.v1",
        message,
    })
    .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
    Ok(sha256_digest(&bytes))
}

fn validate_sha256_digest(value: &str) -> Result<(), IdentityIntentError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(IdentityIntentError::DigestMismatch);
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(IdentityIntentError::DigestMismatch);
    }
    Ok(())
}

fn validate_session_target(session: &DesiredSessionTarget) -> Result<(), IdentityIntentError> {
    if session.session_id.0.is_nil() {
        return Err(IdentityIntentError::NilSessionId);
    }
    SessionLineageId::new(session.lineage_id.as_str().to_string()).map_err(|_| {
        IdentityIntentError::InvalidText {
            field: "session_lineage_id",
        }
    })?;
    if matches!(
        session.authority_policy,
        DesiredSessionAuthorityPolicy::CreateIfAbsent
    ) && session.lineage_generation != SessionGeneration::INITIAL
    {
        return Err(IdentityIntentError::CreateRequiresInitialGeneration);
    }
    Ok(())
}

fn validate_retirement_plan(
    plan: &IdentityRetirementPlan,
    identity: &AgentIdentity,
    current_intent: Option<&IdentityIntent>,
) -> Result<(), IdentityIntentError> {
    let IdentityRetirementPlan::Targets {
        session,
        execution,
        incident_wiring,
    } = plan
    else {
        return if current_intent.is_none() {
            Ok(())
        } else {
            Err(IdentityIntentError::InvalidRetirementPlan)
        };
    };
    validate_session_target(session)?;
    validate_execution(execution)?;
    if incident_wiring
        .iter()
        .any(|edge| &edge.a != identity && &edge.b != identity)
    {
        return Err(IdentityIntentError::InvalidRetirementPlan);
    }
    if let Some(IdentityIntent::Present {
        session: desired_session,
        member,
        owned_wiring,
        ..
    }) = current_intent
    {
        if session != desired_session
            || execution != member.execution()
            || !owned_wiring.is_subset(incident_wiring)
        {
            return Err(IdentityIntentError::InvalidRetirementPlan);
        }
    }
    Ok(())
}

fn validate_string_set(field: &'static str, values: &[String]) -> Result<(), IdentityIntentError> {
    let mut prior = None;
    for value in values {
        validate_text(field, value)?;
        if prior.is_some_and(|prior: &String| prior >= value) {
            return Err(IdentityIntentError::InvalidText { field });
        }
        prior = Some(value);
    }
    Ok(())
}

fn validate_required_local_callback_tools(
    tools: &[DesiredLocalCallbackTool],
    require_canonical_order: bool,
) -> Result<(), IdentityIntentError> {
    let mut names = BTreeSet::new();
    let mut prior_name = None;
    for tool in tools {
        tool.validate()?;
        if !names.insert(tool.name.as_str()) {
            return Err(IdentityIntentError::InvalidMemberMaterial(format!(
                "local callback tool '{}' is declared more than once",
                tool.name
            )));
        }
        if require_canonical_order
            && prior_name.is_some_and(|prior: &str| prior >= tool.name.as_str())
        {
            return Err(IdentityIntentError::InvalidMemberMaterial(
                "local callback tools are not in canonical name order".to_string(),
            ));
        }
        prior_name = Some(tool.name.as_str());
    }
    Ok(())
}

fn validate_execution(execution: &DesiredExecution) -> Result<(), IdentityIntentError> {
    match execution {
        DesiredExecution::External {
            address: _,
            identity,
        } => {
            identity
                .resolve()
                .map_err(|error| IdentityIntentError::InvalidExternalIdentity(error.to_string()))?;
        }
        DesiredExecution::PlacedSession { host_id } => validate_text("host_id", host_id)?,
        DesiredExecution::ControllingSession | DesiredExecution::AnyBoundHostSession => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matching_facts() -> IdentityReconcileFacts {
        IdentityReconcileFacts {
            intent: IdentityAuthorityCondition::PresentRequireExisting,
            lease: IdentityLeaseCondition::HeldByCurrentIncarnation,
            external_binding_required: false,
            initial_delivery_required: false,
            session_creation_receipt: IdentityReceiptCondition::NotRequired,
            retirement_receipt: IdentityReceiptCondition::NotRequired,
            session: IdentitySessionCondition::Matching,
            runtime: IdentityResourceCondition::Matching,
            member: IdentityResourceCondition::Matching,
            external_binding_receipt: IdentityReceiptCondition::NotRequired,
            external_trust: IdentityExternalTrustCondition::NotRequired,
            external_ceremony: IdentityExternalCeremonyCondition::NotRequired,
            initial_delivery_receipt: IdentityReceiptCondition::NotRequired,
            initial_delivery: IdentityInitialDeliveryCondition::NotRequired,
            wiring: IdentityResourceCondition::Matching,
        }
    }

    fn generated_transition_decision(
        authority: &mut crate::machines::mob_machine::MobMachineAuthority,
        facts: IdentityReconcileFacts,
    ) -> IdentityReconcileDecision {
        let transition = crate::machines::mob_machine::MobMachineMutator::apply(
            authority,
            crate::machines::mob_machine::identity_reconciliation_input(facts),
        )
        .expect("generated identity reconciliation transition must be total");
        let effects = transition.into_effects();
        match effects.as_slice() {
            [
                crate::machines::mob_machine::MobMachineEffect::IdentityReconciliationClassified {
                    decision,
                },
            ] => *decision,
            other => panic!(
                "generated identity reconciliation transition emitted unexpected effects: {other:?}"
            ),
        }
    }

    #[test]
    fn unversioned_session_corruption_is_malformed_without_write_authority() {
        let observation = IdentitySessionObservation::malformed_unversioned(
            "persisted Session failed typed decoding",
        )
        .expect("canonical corruption detail should construct");

        assert_eq!(observation.condition(), IdentitySessionCondition::Malformed);
        assert_eq!(observation.target_precondition().unwrap(), None);
        assert_eq!(observation.verified_checkpoint(), None);
        assert_eq!(
            observation.malformed_unversioned_detail(),
            Some("persisted Session failed typed decoding")
        );
        assert_eq!(
            IdentitySessionObservation::unavailable("transport failure")
                .malformed_unversioned_detail(),
            None
        );
    }

    #[test]
    fn unversioned_session_corruption_rejects_noncanonical_detail() {
        for detail in ["", " ", " leading", "trailing "] {
            assert!(matches!(
                IdentitySessionObservation::malformed_unversioned(detail),
                Err(IdentityIntentError::InvalidText {
                    field: "identity_session_malformed_detail"
                })
            ));
        }
    }

    #[test]
    fn unversioned_session_corruption_reuses_strict_malformed_condition_serde() {
        let observation =
            IdentitySessionObservation::malformed_unversioned("unsupported checkpoint schema")
                .unwrap();
        let encoded = serde_json::to_string(&observation.condition()).unwrap();
        assert_eq!(encoded, r#""malformed""#);
        assert_eq!(
            serde_json::from_str::<IdentitySessionCondition>(&encoded).unwrap(),
            IdentitySessionCondition::Malformed
        );
        assert!(
            serde_json::from_str::<IdentitySessionCondition>(r#""malformed_unversioned""#).is_err(),
            "the observation shape must not extend generated classifier vocabulary"
        );
    }

    #[test]
    fn classifier_is_total_over_the_core_observation_product() {
        let mut authority = crate::machines::mob_machine::MobMachineAuthority::new();
        let initial_state = authority.state().clone();
        let intents = [
            IdentityAuthorityCondition::Unavailable,
            IdentityAuthorityCondition::Missing,
            IdentityAuthorityCondition::Malformed,
            IdentityAuthorityCondition::PresentCreateIfAbsent,
            IdentityAuthorityCondition::PresentRequireExisting,
            IdentityAuthorityCondition::Absent,
        ];
        let leases = [
            IdentityLeaseCondition::Unavailable,
            IdentityLeaseCondition::Missing,
            IdentityLeaseCondition::Malformed,
            IdentityLeaseCondition::HeldByCurrentIncarnation,
            IdentityLeaseCondition::HeldByOtherLiveIncarnation,
            IdentityLeaseCondition::HeldByExpiredIncarnation,
        ];
        let resources = [
            IdentityResourceCondition::Unavailable,
            IdentityResourceCondition::Missing,
            IdentityResourceCondition::Matching,
            IdentityResourceCondition::Divergent,
            IdentityResourceCondition::Malformed,
        ];
        let sessions = [
            IdentitySessionCondition::Unavailable,
            IdentitySessionCondition::Missing,
            IdentitySessionCondition::Matching,
            IdentitySessionCondition::RecoverableDivergence,
            IdentitySessionCondition::AmbiguousDivergence,
            IdentitySessionCondition::Malformed,
            IdentitySessionCondition::IrrecoverablyCorrupt,
        ];

        let mut classified = 0usize;
        for intent in intents {
            for lease in leases {
                for session in sessions {
                    for runtime in resources {
                        for member in resources {
                            for wiring in resources {
                                let mut facts = matching_facts();
                                facts.intent = intent;
                                facts.lease = lease;
                                facts.session = session;
                                facts.runtime = runtime;
                                facts.member = member;
                                facts.wiring = wiring;
                                let expected = classify_identity_reconciliation(facts);
                                assert_eq!(
                                    generated_transition_decision(&mut authority, facts),
                                    expected,
                                );
                                classified += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(classified, 31_500);
        assert_eq!(authority.state(), &initial_state);
    }

    #[test]
    fn classifier_is_total_over_operation_garbage_cross_shapes() {
        let mut authority = crate::machines::mob_machine::MobMachineAuthority::new();
        let initial_state = authority.state().clone();
        let receipts = [
            IdentityReceiptCondition::NotRequired,
            IdentityReceiptCondition::Unavailable,
            IdentityReceiptCondition::Missing,
            IdentityReceiptCondition::Matching,
            IdentityReceiptCondition::Conflicting,
            IdentityReceiptCondition::Malformed,
        ];
        let trusts = [
            IdentityExternalTrustCondition::NotRequired,
            IdentityExternalTrustCondition::Unavailable,
            IdentityExternalTrustCondition::Matching,
            IdentityExternalTrustCondition::Absent,
            IdentityExternalTrustCondition::Contradictory,
            IdentityExternalTrustCondition::Indeterminate,
            IdentityExternalTrustCondition::Malformed,
        ];
        let ceremonies = [
            IdentityExternalCeremonyCondition::NotRequired,
            IdentityExternalCeremonyCondition::FreshAvailable,
            IdentityExternalCeremonyCondition::TemporarilyUnavailable,
            IdentityExternalCeremonyCondition::AwaitFresh,
            IdentityExternalCeremonyCondition::SpentOrUnknown,
        ];
        let deliveries = [
            IdentityInitialDeliveryCondition::NotRequired,
            IdentityInitialDeliveryCondition::Unavailable,
            IdentityInitialDeliveryCondition::ProvenAbsent,
            IdentityInitialDeliveryCondition::AcceptedPendingExact,
            IdentityInitialDeliveryCondition::CommittedExact,
            IdentityInitialDeliveryCondition::ContentOnlyMatch,
            IdentityInitialDeliveryCondition::OperationCollision,
            IdentityInitialDeliveryCondition::Contradictory,
            IdentityInitialDeliveryCondition::Indeterminate,
            IdentityInitialDeliveryCondition::Malformed,
        ];
        let mut classified = 0usize;
        for external_required in [false, true] {
            for delivery_required in [false, true] {
                for external_receipt in receipts {
                    for external_trust in trusts {
                        for ceremony in ceremonies {
                            for delivery_receipt in receipts {
                                for initial_delivery in deliveries {
                                    let mut facts = matching_facts();
                                    facts.external_binding_required = external_required;
                                    facts.initial_delivery_required = delivery_required;
                                    facts.external_binding_receipt = external_receipt;
                                    facts.external_trust = external_trust;
                                    facts.external_ceremony = ceremony;
                                    facts.initial_delivery_receipt = delivery_receipt;
                                    facts.initial_delivery = initial_delivery;
                                    let expected = classify_identity_reconciliation(facts);
                                    assert_eq!(
                                        generated_transition_decision(&mut authority, facts),
                                        expected,
                                    );
                                    classified += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(classified, 50_400);
        assert_eq!(authority.state(), &initial_state);
    }

    #[test]
    fn classifier_orders_one_obligation_per_pass() {
        let mut facts = matching_facts();
        facts.intent = IdentityAuthorityCondition::PresentCreateIfAbsent;
        facts.session_creation_receipt = IdentityReceiptCondition::Missing;
        facts.wiring = IdentityResourceCondition::Divergent;
        facts.member = IdentityResourceCondition::Missing;
        facts.runtime = IdentityResourceCondition::Divergent;
        facts.session = IdentitySessionCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::EnsureSessionAuthority
        );
        facts.session = IdentitySessionCondition::Matching;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::SealSessionCreationConsumed
        );
        facts.session_creation_receipt = IdentityReceiptCondition::Matching;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::EnsureRuntimeRegistration
        );
        facts.runtime = IdentityResourceCondition::Matching;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::EnsureMemberMaterialization
        );
        facts.member = IdentityResourceCondition::Matching;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::ReconcileWiring
        );
    }

    #[test]
    fn wiring_drift_and_cleanup_share_one_reconciliation_obligation() {
        let mut facts = matching_facts();
        facts.wiring = IdentityResourceCondition::Divergent;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::ReconcileWiring
        );

        facts.wiring = IdentityResourceCondition::Matching;
        facts.session = IdentitySessionCondition::Malformed;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::ReconcileWiring
        );

        facts.intent = IdentityAuthorityCondition::Absent;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::ReconcileWiring
        );
    }

    #[test]
    fn divergent_member_is_repair_blocked_before_create_only_actuation() {
        let mut facts = matching_facts();
        facts.member = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::EnsureMemberMaterialization
        );

        facts.member = IdentityResourceCondition::Divergent;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RepairBlocked
        );
    }

    #[test]
    fn malformed_evidence_beats_unrelated_unavailability() {
        let mut facts = matching_facts();
        facts.runtime = IdentityResourceCondition::Malformed;
        facts.wiring = IdentityResourceCondition::Unavailable;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RepairBlocked
        );
    }

    #[test]
    fn unrelated_later_observations_do_not_block_session_or_runtime_obligations() {
        let mut facts = matching_facts();
        facts.intent = IdentityAuthorityCondition::PresentCreateIfAbsent;
        facts.session_creation_receipt = IdentityReceiptCondition::Missing;
        facts.session = IdentitySessionCondition::Missing;
        facts.runtime = IdentityResourceCondition::Malformed;
        facts.member = IdentityResourceCondition::Unavailable;
        facts.wiring = IdentityResourceCondition::Unavailable;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::EnsureSessionAuthority
        );

        facts.session = IdentitySessionCondition::Matching;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::SealSessionCreationConsumed
        );
        facts.session_creation_receipt = IdentityReceiptCondition::Matching;
        facts.runtime = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::EnsureRuntimeRegistration
        );
    }

    #[test]
    fn absent_cleanup_drains_known_residue_before_ambiguous_session_blocks() {
        let mut facts = matching_facts();
        facts.intent = IdentityAuthorityCondition::Absent;
        facts.retirement_receipt = IdentityReceiptCondition::Missing;
        facts.session = IdentitySessionCondition::Malformed;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::ReconcileWiring
        );
        facts.wiring = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RetireMemberMaterialization
        );
        facts.member = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RetireRuntimeRegistration
        );
        facts.runtime = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RepairBlocked
        );
    }

    #[test]
    fn present_malformed_session_drains_derived_residue_before_blocking() {
        let mut facts = matching_facts();
        facts.session = IdentitySessionCondition::Malformed;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::ReconcileWiring
        );
        facts.wiring = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RetireMemberMaterialization
        );
        facts.member = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RetireRuntimeRegistration
        );
        facts.runtime = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RepairBlocked
        );
    }

    #[test]
    fn require_existing_never_fabricates_missing_or_ambiguous_history() {
        for session in [
            IdentitySessionCondition::Missing,
            IdentitySessionCondition::AmbiguousDivergence,
        ] {
            let mut facts = matching_facts();
            facts.session = session;
            facts.wiring = IdentityResourceCondition::Missing;
            facts.member = IdentityResourceCondition::Missing;
            facts.runtime = IdentityResourceCondition::Missing;
            assert_eq!(
                classify_identity_reconciliation(facts),
                IdentityReconcileDecision::RepairBlocked
            );
        }
    }

    #[test]
    fn create_if_absent_is_consumed_after_the_first_matching_checkpoint() {
        let mut facts = matching_facts();
        facts.intent = IdentityAuthorityCondition::PresentCreateIfAbsent;
        facts.session_creation_receipt = IdentityReceiptCondition::Missing;
        facts.session = IdentitySessionCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::EnsureSessionAuthority
        );
        facts.session = IdentitySessionCondition::Matching;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::SealSessionCreationConsumed
        );
        facts.session_creation_receipt = IdentityReceiptCondition::Matching;
        facts.session = IdentitySessionCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RepairBlocked
        );
    }

    #[test]
    fn absent_retires_every_resource_then_seals_proof() {
        let mut facts = matching_facts();
        facts.intent = IdentityAuthorityCondition::Absent;
        facts.retirement_receipt = IdentityReceiptCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::ReconcileWiring
        );
        facts.wiring = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RetireMemberMaterialization
        );
        facts.member = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RetireRuntimeRegistration
        );
        facts.runtime = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::ReleaseSessionAuthority
        );
        facts.session = IdentitySessionCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::SealRetirementProven
        );
        facts.retirement_receipt = IdentityReceiptCondition::Matching;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::Tombstoned
        );
    }

    #[test]
    fn external_binding_never_replays_without_fresh_ceremony() {
        let mut facts = matching_facts();
        facts.external_binding_required = true;
        facts.external_binding_receipt = IdentityReceiptCondition::Matching;
        facts.external_trust = IdentityExternalTrustCondition::Absent;
        facts.external_ceremony = IdentityExternalCeremonyCondition::AwaitFresh;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::AwaitExternalBindingCeremony
        );
        facts.external_ceremony = IdentityExternalCeremonyCondition::FreshAvailable;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::EnsureExternalBinding
        );
        facts.external_ceremony = IdentityExternalCeremonyCondition::SpentOrUnknown;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RepairBlocked
        );
    }

    #[test]
    fn initial_delivery_uses_exact_runtime_input_identity() {
        let mut facts = matching_facts();
        facts.initial_delivery_required = true;
        facts.initial_delivery_receipt = IdentityReceiptCondition::Missing;
        facts.initial_delivery = IdentityInitialDeliveryCondition::ProvenAbsent;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::EnsureInitialDeliveryReceipt
        );
        facts.initial_delivery_receipt = IdentityReceiptCondition::Matching;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::EnsureInitialDelivery
        );
        facts.initial_delivery = IdentityInitialDeliveryCondition::AcceptedPendingExact;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::AwaitInitialDelivery
        );
        facts.initial_delivery = IdentityInitialDeliveryCondition::CommittedExact;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::Converged
        );
    }

    #[test]
    fn expired_or_missing_lease_is_reclaimable_by_construction() {
        let mut facts = matching_facts();
        facts.lease = IdentityLeaseCondition::HeldByExpiredIncarnation;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::AcquireLease
        );
        facts.lease = IdentityLeaseCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::AcquireLease
        );
    }

    #[test]
    fn corrupt_transcript_is_preserved_after_derived_residue_is_retired() {
        let mut facts = matching_facts();
        facts.session = IdentitySessionCondition::IrrecoverablyCorrupt;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::ReconcileWiring
        );
        facts.wiring = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RetireMemberMaterialization
        );
        facts.member = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::RetireRuntimeRegistration
        );
        facts.runtime = IdentityResourceCondition::Missing;
        assert_eq!(
            classify_identity_reconciliation(facts),
            IdentityReconcileDecision::Quarantined
        );
    }

    #[test]
    fn external_desired_binding_cannot_carry_bootstrap_authority() {
        let identity = serde_json::json!({
            "kind": "ed25519_public_key",
            "public_key": "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="
        });
        let with_token = serde_json::json!({
            "execution": "external",
            "address": "tcp://127.0.0.1:4242",
            "identity": identity.clone(),
            "bootstrap_token": "must-not-decode"
        });
        assert!(serde_json::from_value::<DesiredExecution>(with_token).is_err());
        let query_secret = serde_json::json!({
            "execution": "external",
            "address": "tcp://127.0.0.1:4242?mob_supervisor_bootstrap_token=secret",
            "identity": identity.clone()
        });
        assert!(serde_json::from_value::<DesiredExecution>(query_secret).is_err());
        let valid = serde_json::json!({
            "execution": "external",
            "address": "tcp://127.0.0.1:4242",
            "identity": identity
        });
        assert!(serde_json::from_value::<DesiredExecution>(valid).is_ok());
    }

    #[test]
    fn local_callback_tool_contract_is_strict_and_canonical() {
        let alpha = DesiredLocalCallbackTool::new(
            "alpha",
            "Alpha callback",
            serde_json::json!({"type": "object"}),
        )
        .unwrap();
        let beta = DesiredLocalCallbackTool::new(
            "beta",
            "Beta callback",
            serde_json::json!({"type": "object"}),
        )
        .unwrap();
        validate_required_local_callback_tools(&[alpha.clone(), beta.clone()], true).unwrap();
        assert!(validate_required_local_callback_tools(&[beta, alpha.clone()], true).is_err());
        assert!(validate_required_local_callback_tools(&[alpha.clone(), alpha], true).is_err());
        assert!(
            DesiredLocalCallbackTool::new(
                "invalid",
                "Invalid callback",
                serde_json::json!({"type": 7}),
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<DesiredLocalCallbackTool>(serde_json::json!({
                "name": "extra",
                "description": "Extra callback",
                "input_schema": {},
                "handler": "must-not-enter-durable-material"
            }))
            .is_err()
        );
    }

    #[test]
    fn authority_digest_seals_tombstone_and_cleanup_targets() {
        let identity = AgentIdentity::from("parent-1");
        let intent = IdentityIntent::Absent { identity };
        let mut record = IdentityIntentRecord {
            schema_version: IDENTITY_INTENT_SCHEMA_VERSION,
            mob_id: MobId::from("homecore"),
            intent_revision: 7,
            declaration_scope: None,
            declaration_revision: None,
            tombstone_generation: Some(3),
            initial_delivery_generation_highwater: 0,
            retirement_plan: IdentityRetirementPlan::NoKnownRealization,
            intent_digest: intent.digest().unwrap(),
            authority_digest: String::new(),
            intent,
        };
        record.authority_digest = record.canonical_authority_digest().unwrap();
        record.validate().unwrap();
        let mut legacy_shape = serde_json::to_value(&record).unwrap();
        legacy_shape.as_object_mut().unwrap().remove("mob_id");
        assert!(
            serde_json::from_value::<IdentityIntentRecord>(legacy_shape).is_err(),
            "mob authority scope is required and must never default during decode"
        );
        let mut transplanted = record.clone();
        transplanted.mob_id = MobId::from("other-mob");
        assert!(matches!(
            transplanted.validate(),
            Err(IdentityIntentError::DigestMismatch)
        ));
        record.tombstone_generation = Some(4);
        assert!(matches!(
            record.validate(),
            Err(IdentityIntentError::DigestMismatch)
        ));
    }

    #[test]
    fn empty_declaration_scope_has_a_sealed_restart_cas_head() {
        let scope_id = IdentityDeclarationScopeId::new("homecore.roster").unwrap();
        let operation_id = OperationId::new();
        let manifest = IdentityDeclarationManifest {
            scope_id: scope_id.clone(),
            operation_id: operation_id.clone(),
            expected_scope: IdentityDeclarationScopePrecondition::Missing,
            members: BTreeMap::new(),
            wiring: BTreeSet::new(),
        };
        let request_digest = manifest.request_digest().unwrap();
        let compiled_manifest_digest = sha256_digest(b"empty-compiled-manifest");
        let mut head = IdentityDeclarationScopeHead {
            schema_version: IDENTITY_DECLARATION_SCOPE_SCHEMA_VERSION,
            mob_id: MobId::from("homecore"),
            scope_id,
            revision: 1,
            operation_id,
            request_digest,
            compiled_manifest_digest,
            declared_member_count: 0,
            authority_digest: String::new(),
        };
        head.authority_digest = head.canonical_authority_digest().unwrap();
        head.validate().unwrap();

        let mut stale_after_restart = head.clone();
        stale_after_restart.revision = 2;
        assert!(matches!(
            stale_after_restart.validate(),
            Err(IdentityIntentError::DigestMismatch)
        ));
    }

    #[test]
    fn declaration_manifest_rejects_cross_scope_wiring() {
        let mut members = BTreeMap::new();
        members.insert(
            AgentIdentity::from("parent-1"),
            IdentityMemberDeclaration {
                material: IdentityMemberMaterialDeclaration::Profile(
                    IdentityProfileMemberDeclaration {
                        profile_name: ProfileName::from("parent"),
                        profile_override: None,
                        model_override: None,
                        external_addressable_override: None,
                        context: None,
                        labels: None,
                        additional_instructions: None,
                        system_prompt_override: None,
                        tool_access_policy: None,
                        auth_binding: None,
                        budget_limits: None,
                        runtime_mode: None,
                        required_env_keys: Vec::new(),
                        required_local_callback_tools: Vec::new(),
                        execution: DesiredExecution::ControllingSession,
                    },
                ),
                session_authority_policy: DesiredSessionAuthorityPolicy::CreateIfAbsent,
                initial_message: None,
                legacy_import: None,
            },
        );
        let manifest = IdentityDeclarationManifest {
            scope_id: IdentityDeclarationScopeId::new("homecore.roster").unwrap(),
            operation_id: OperationId::new(),
            expected_scope: IdentityDeclarationScopePrecondition::Any,
            members,
            wiring: BTreeSet::from([DesiredIdentityEdge::new(
                AgentIdentity::from("parent-1"),
                AgentIdentity::from("outside-scope"),
            )
            .unwrap()]),
        };
        assert!(matches!(
            manifest.validate(),
            Err(IdentityIntentError::CrossScopeEdge(_))
        ));
    }

    #[test]
    fn initial_delivery_identity_is_content_sealed() {
        let mut delivery =
            DesiredInitialDelivery::new(1, InputId::new(), ContentInput::from("hello once"))
                .unwrap();
        delivery.validate().unwrap();
        delivery.message = ContentInput::from("different message");
        assert!(matches!(
            delivery.validate(),
            Err(IdentityIntentError::InvalidInitialDelivery)
        ));
    }

    #[test]
    fn permit_requires_scope_incarnation_and_unexpired_claim() {
        let permit = IdentityActuationPermit {
            mob_id: MobId::from("homecore"),
            identity: AgentIdentity::from("parent-1"),
            target: IdentityActuatorTarget::Runtime,
            intent_revision: 4,
            intent_digest: format!("sha256:{}", "1".repeat(64)),
            intent_authority_digest: format!("sha256:{}", "2".repeat(64)),
            lease_epoch: 9,
            lease_holder_id: "controller-a".to_string(),
            lease_incarnation_id: "incarnation-a".to_string(),
            lease_expires_at_ms: 200,
            target_observation: IdentityTargetObservationVersion::Absent {
                absence_version: "absence:17".to_string(),
            },
        };
        permit.validate_for_write(199).unwrap();
        assert!(matches!(
            permit.validate_for_write(200),
            Err(IdentityIntentError::ExpiredActuationPermit)
        ));

        let mut receipt_permit = permit.clone();
        receipt_permit.target = IdentityActuatorTarget::InitialDeliveryReceipt;
        assert!(matches!(
            receipt_permit.validate_for_write(199),
            Err(IdentityIntentError::InvalidActuationPermit)
        ));
        receipt_permit.target_observation = IdentityTargetObservationVersion::InsertIfAbsent;
        receipt_permit.validate_for_write(199).unwrap();

        let mut resource_with_receipt_cas = permit;
        resource_with_receipt_cas.target_observation =
            IdentityTargetObservationVersion::InsertIfAbsent;
        assert!(matches!(
            resource_with_receipt_cas.validate_for_write(199),
            Err(IdentityIntentError::InvalidActuationPermit)
        ));
    }

    #[test]
    fn bounded_lease_lifetime_is_enforced() {
        let valid = IdentityLeaseRecord {
            schema_version: IDENTITY_LEASE_SCHEMA_VERSION,
            epoch_highwater: 1,
            active: Some(IdentityLeaseClaim {
                holder_id: "controller".to_string(),
                incarnation_id: "process-a".to_string(),
                epoch: 1,
                renewed_at_ms: 10,
                expires_at_ms: 10 + IDENTITY_LEASE_MAX_TTL_MS,
            }),
        };
        valid.validate().unwrap();
        let mut invalid = valid;
        invalid.active.as_mut().unwrap().expires_at_ms += 1;
        assert!(matches!(
            invalid.validate(),
            Err(IdentityIntentError::InvalidLeaseLifetime)
        ));
    }
}
