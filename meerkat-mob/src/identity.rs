//! Level-triggered desired-state contracts for stable mob-member identities.
//!
//! There are deliberately only three durable reconciliation inputs:
//! [`IdentityIntentRecord`], [`IdentityLeaseRecord`], and narrowly scoped
//! [`IdentityOperationReceipt`] custody. [`IdentityConvergenceStatus`] is
//! replaceable output only. Session, runtime, roster, and wiring state are
//! observed realization, and reconciliation never persists a second copy of
//! them here.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use meerkat_contracts::wire::{
    PortableDefinitionExtract, PortableProfile, PortableSystemPrompt, WireAuthBindingRef,
    WireMobRuntimeMode, WireOpaqueJson, WireResolvedToolAccessPolicy, WireTrustedPeerIdentity,
};
use meerkat_core::lifecycle::InputId;
use meerkat_core::ops::OperationId;
use meerkat_core::{
    ApplicationToolPolicyBinding, BudgetLimits, ContentInput, Session, SessionGeneration,
    SessionId, SessionLineageId, ToolCategoryOverrides, ToolName,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{AgentIdentity, MobId, ProfileName};

pub const IDENTITY_INTENT_SCHEMA_VERSION: u32 = 1;
pub const IDENTITY_LEASE_SCHEMA_VERSION: u32 = 1;
pub const IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const IDENTITY_INTENT_MUTATION_RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const IDENTITY_CONVERGENCE_RESOLUTION_RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const IDENTITY_ADOPTION_RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const IDENTITY_INTENT_MAX_ENCODED_BYTES: usize = 4 * 1024 * 1024;
pub const IDENTITY_LEASE_MAX_TTL_MS: u64 = 30_000;
pub const IDENTITY_CONVERGENCE_MAX_DRAIN_MS: u64 = 5 * 60 * 1000;

/// Provider namespace retained as provenance in already-sealed intent rows.
///
/// Reconciliation never reads this value as desired state. It remains part of
/// the exact authority digest for intent records sealed with it.
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

/// Exact desired session lineage for a stable member identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredSessionTarget {
    pub session_id: SessionId,
    /// Stable logical lineage selected by the caller for intent identity and
    /// idempotency slots. This is not physical persistence authority: only
    /// [`IdentitySessionStoreAuthority`] establishes store currentness.
    pub lineage_id: SessionLineageId,
    pub lineage_generation: SessionGeneration,
    pub authority_policy: DesiredSessionAuthorityPolicy,
}

/// Store-issued physical identity of one exact committed session boundary.
///
/// The physical token variant is private, so callers cannot construct a
/// caller-authored string and label it durable authority. The only production
/// constructor consumes [`meerkat_runtime::RuntimeSessionAuthority`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentitySessionStoreAuthority {
    session_id: SessionId,
    store_revision: u64,
    token: IdentitySessionStoreAuthorityToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "profile", rename_all = "snake_case", deny_unknown_fields)]
enum IdentitySessionStoreAuthorityToken {
    WholeBlobV1 { blob_sha256: String },
    HeadCanonicalV1 { committed_head_token: String },
}

impl IdentitySessionStoreAuthority {
    pub(crate) fn from_runtime_authority(
        authority: meerkat_runtime::RuntimeSessionAuthority,
    ) -> Self {
        match authority {
            meerkat_runtime::RuntimeSessionAuthority::WholeBlob(authority) => Self {
                session_id: authority.session_id().clone(),
                store_revision: authority.store_revision(),
                token: IdentitySessionStoreAuthorityToken::WholeBlobV1 {
                    blob_sha256: authority.blob_sha256().to_string(),
                },
            },
            meerkat_runtime::RuntimeSessionAuthority::HeadCanonical(authority) => Self {
                session_id: authority.session_id().clone(),
                store_revision: authority.store_revision(),
                token: IdentitySessionStoreAuthorityToken::HeadCanonicalV1 {
                    committed_head_token: authority.committed_head_token().to_string(),
                },
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn whole_blob_for_test(
        session_id: SessionId,
        store_revision: u64,
        blob_sha256: impl Into<String>,
    ) -> Self {
        Self {
            session_id,
            store_revision,
            token: IdentitySessionStoreAuthorityToken::WholeBlobV1 {
                blob_sha256: blob_sha256.into(),
            },
        }
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn profile(&self) -> meerkat_runtime::RuntimeSessionPersistenceProfile {
        match &self.token {
            IdentitySessionStoreAuthorityToken::WholeBlobV1 { .. } => {
                meerkat_runtime::RuntimeSessionPersistenceProfile::WholeBlobV1
            }
            IdentitySessionStoreAuthorityToken::HeadCanonicalV1 { .. } => {
                meerkat_runtime::RuntimeSessionPersistenceProfile::HeadCanonicalV1
            }
        }
    }

    #[must_use]
    pub const fn store_revision(&self) -> u64 {
        self.store_revision
    }

    #[must_use]
    pub fn token(&self) -> &str {
        match &self.token {
            IdentitySessionStoreAuthorityToken::WholeBlobV1 { blob_sha256 } => blob_sha256,
            IdentitySessionStoreAuthorityToken::HeadCanonicalV1 {
                committed_head_token,
            } => committed_head_token,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), IdentityIntentError> {
        if self.session_id.0.is_nil() || self.store_revision == 0 {
            return Err(IdentityIntentError::InvalidSessionStoreAuthority);
        }
        let valid_token = match &self.token {
            IdentitySessionStoreAuthorityToken::WholeBlobV1 { blob_sha256 } => {
                has_prefixed_sha256(blob_sha256, &["row-sha256:"])
            }
            IdentitySessionStoreAuthorityToken::HeadCanonicalV1 {
                committed_head_token,
            } => has_prefixed_sha256(committed_head_token, &["head-v5-sha256:"]),
        };
        if !valid_token {
            return Err(IdentityIntentError::InvalidSessionStoreAuthority);
        }
        Ok(())
    }

    pub(crate) fn observation_version(&self) -> Result<String, IdentityIntentError> {
        #[derive(Serialize)]
        struct ObservationVersionMaterial<'a> {
            domain: &'static str,
            authority: &'a IdentitySessionStoreAuthority,
        }

        self.validate()?;
        let bytes = serde_json::to_vec(&ObservationVersionMaterial {
            domain: "meerkat.identity.session_store_authority.v1",
            authority: self,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }
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
    /// Build-time prompt replacement for materializing a fresh member.
    /// `None` is reserved for adoption of an existing exact session: its
    /// durable transcript remains the sole prompt authority on resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<PortableSystemPrompt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_access_policy: Option<WireResolvedToolAccessPolicy>,
    #[serde(default)]
    pub tool_category_overrides: ToolCategoryOverrides,
    #[serde(default)]
    pub application_tool_policy: ApplicationToolPolicyBinding,
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

/// Administrative declaration for the exact callback definition set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "kind",
    content = "tools",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CallbackToolSetDeclaration {
    Inherit,
    Set(Vec<DesiredLocalCallbackTool>),
}

/// One static execution constraint in the public member declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "kind",
    content = "names",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MemberToolAccessConstraint {
    AllowNames(Vec<String>),
    DenyNames(Vec<String>),
    ReadOnly,
}

/// Explicit execution-policy declaration. Empty constraints are invalid and
/// unrestricted access has exactly one spelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "kind",
    content = "constraints",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MemberToolAccessDeclaration {
    Inherit,
    Unrestricted,
    Constraints(Vec<MemberToolAccessConstraint>),
}

/// Durable administrative tool intent for one stable member identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MemberToolDeclaration {
    #[serde(default)]
    pub category_overrides: ToolCategoryOverrides,
    pub callback_tools: CallbackToolSetDeclaration,
    pub execution: MemberToolAccessDeclaration,
    pub application_policy: ApplicationToolPolicyBinding,
}

impl MemberToolDeclaration {
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        if let CallbackToolSetDeclaration::Set(callbacks) = &self.callback_tools {
            validate_required_local_callback_tools(callbacks, true)?;
        }
        if let MemberToolAccessDeclaration::Constraints(constraints) = &self.execution {
            if constraints.is_empty() {
                return Err(IdentityIntentError::InvalidMemberToolDeclaration(
                    "tool access constraints must not be empty".to_string(),
                ));
            }
            for constraint in constraints {
                match constraint {
                    MemberToolAccessConstraint::AllowNames(names) => {
                        validate_string_set("member_tool_allow_name", names)?;
                    }
                    MemberToolAccessConstraint::DenyNames(names) => {
                        validate_string_set("member_tool_deny_name", names)?;
                    }
                    MemberToolAccessConstraint::ReadOnly => {}
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn to_wire(&self) -> meerkat_contracts::wire::WireMemberToolDeclaration {
        use meerkat_contracts::wire::{
            WireCallbackToolSetDeclaration, WireDesiredLocalCallbackTool,
            WireMemberToolAccessConstraint, WireMemberToolAccessDeclaration,
            WireMemberToolDeclaration,
        };
        let callback_tools = match &self.callback_tools {
            CallbackToolSetDeclaration::Inherit => WireCallbackToolSetDeclaration::Inherit,
            CallbackToolSetDeclaration::Set(tools) => WireCallbackToolSetDeclaration::Set(
                tools
                    .iter()
                    .map(|tool| WireDesiredLocalCallbackTool {
                        name: tool.name.to_string(),
                        description: tool.description.clone(),
                        input_schema: tool.input_schema.clone(),
                    })
                    .collect(),
            ),
        };
        let execution = match &self.execution {
            MemberToolAccessDeclaration::Inherit => WireMemberToolAccessDeclaration::Inherit,
            MemberToolAccessDeclaration::Unrestricted => {
                WireMemberToolAccessDeclaration::Unrestricted
            }
            MemberToolAccessDeclaration::Constraints(constraints) => {
                WireMemberToolAccessDeclaration::Constraints(
                    constraints
                        .iter()
                        .map(|constraint| match constraint {
                            MemberToolAccessConstraint::AllowNames(names) => {
                                WireMemberToolAccessConstraint::AllowNames(names.clone())
                            }
                            MemberToolAccessConstraint::DenyNames(names) => {
                                WireMemberToolAccessConstraint::DenyNames(names.clone())
                            }
                            MemberToolAccessConstraint::ReadOnly => {
                                WireMemberToolAccessConstraint::ReadOnly
                            }
                        })
                        .collect(),
                )
            }
        };
        WireMemberToolDeclaration {
            category_overrides: self.category_overrides,
            callback_tools,
            execution,
            application_policy: self.application_policy.clone(),
        }
    }
}

impl TryFrom<meerkat_contracts::wire::WireMemberToolDeclaration> for MemberToolDeclaration {
    type Error = IdentityIntentError;

    fn try_from(
        value: meerkat_contracts::wire::WireMemberToolDeclaration,
    ) -> Result<Self, Self::Error> {
        use meerkat_contracts::wire::{
            WireCallbackToolSetDeclaration, WireMemberToolAccessConstraint,
            WireMemberToolAccessDeclaration,
        };
        let callback_tools = match value.callback_tools {
            WireCallbackToolSetDeclaration::Inherit => CallbackToolSetDeclaration::Inherit,
            WireCallbackToolSetDeclaration::Set(tools) => CallbackToolSetDeclaration::Set(
                tools
                    .into_iter()
                    .map(|tool| {
                        DesiredLocalCallbackTool::new(
                            tool.name,
                            tool.description,
                            tool.input_schema,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        };
        let execution = match value.execution {
            WireMemberToolAccessDeclaration::Inherit => MemberToolAccessDeclaration::Inherit,
            WireMemberToolAccessDeclaration::Unrestricted => {
                MemberToolAccessDeclaration::Unrestricted
            }
            WireMemberToolAccessDeclaration::Constraints(constraints) => {
                MemberToolAccessDeclaration::Constraints(
                    constraints
                        .into_iter()
                        .map(|constraint| match constraint {
                            WireMemberToolAccessConstraint::AllowNames(names) => {
                                MemberToolAccessConstraint::AllowNames(names)
                            }
                            WireMemberToolAccessConstraint::DenyNames(names) => {
                                MemberToolAccessConstraint::DenyNames(names)
                            }
                            WireMemberToolAccessConstraint::ReadOnly => {
                                MemberToolAccessConstraint::ReadOnly
                            }
                        })
                        .collect(),
                )
            }
        };
        let declaration = Self {
            category_overrides: value.category_overrides,
            callback_tools,
            execution,
            application_policy: value.application_policy,
        };
        declaration.validate()?;
        Ok(declaration)
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
                // Name-independent: there is no name set to validate.
                WireResolvedToolAccessPolicy::ReadOnly => {}
                WireResolvedToolAccessPolicy::Constraints(constraints) => {
                    if constraints.is_empty() {
                        return Err(IdentityIntentError::InvalidMemberMaterial(
                            "tool access constraints must not be empty".to_string(),
                        ));
                    }
                    for constraint in constraints {
                        match constraint {
                            meerkat_contracts::wire::WireToolAccessConstraint::AllowNames(
                                names,
                            ) => {
                                validate_string_set("tool_access_allow_name", names)?;
                            }
                            meerkat_contracts::wire::WireToolAccessConstraint::DenyNames(names) => {
                                validate_string_set("tool_access_deny_name", names)?;
                            }
                            meerkat_contracts::wire::WireToolAccessConstraint::ReadOnly => {}
                        }
                    }
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

    /// Project the current durable tool portion as one explicit declaration.
    /// Callback and execution inheritance are resolved before persistence, so
    /// reads never have to guess which earlier request supplied them.
    #[must_use]
    pub fn member_tool_declaration(&self) -> MemberToolDeclaration {
        let execution = match &self.overlay.tool_access_policy {
            None => MemberToolAccessDeclaration::Unrestricted,
            Some(WireResolvedToolAccessPolicy::AllowList(names)) => {
                MemberToolAccessDeclaration::Constraints(vec![
                    MemberToolAccessConstraint::AllowNames(names.clone()),
                ])
            }
            Some(WireResolvedToolAccessPolicy::DenyList(names)) => {
                MemberToolAccessDeclaration::Constraints(vec![
                    MemberToolAccessConstraint::DenyNames(names.clone()),
                ])
            }
            Some(WireResolvedToolAccessPolicy::ReadOnly) => {
                MemberToolAccessDeclaration::Constraints(vec![MemberToolAccessConstraint::ReadOnly])
            }
            Some(WireResolvedToolAccessPolicy::Constraints(constraints)) => {
                MemberToolAccessDeclaration::Constraints(
                    constraints
                        .iter()
                        .map(|constraint| match constraint {
                            meerkat_contracts::wire::WireToolAccessConstraint::AllowNames(
                                names,
                            ) => MemberToolAccessConstraint::AllowNames(names.clone()),
                            meerkat_contracts::wire::WireToolAccessConstraint::DenyNames(names) => {
                                MemberToolAccessConstraint::DenyNames(names.clone())
                            }
                            meerkat_contracts::wire::WireToolAccessConstraint::ReadOnly => {
                                MemberToolAccessConstraint::ReadOnly
                            }
                        })
                        .collect(),
                )
            }
        };
        MemberToolDeclaration {
            category_overrides: self.overlay.tool_category_overrides,
            callback_tools: CallbackToolSetDeclaration::Set(
                self.required_local_callback_tools.clone(),
            ),
            execution,
            application_policy: self.overlay.application_tool_policy.clone(),
        }
    }

    /// Compile a public declaration into a replacement of only this
    /// material's tool portion. Non-tool profile, prompt, identity, session,
    /// execution placement, and wiring facts are copied byte-for-byte.
    pub fn with_member_tool_declaration(
        &self,
        declaration: &MemberToolDeclaration,
    ) -> Result<Self, IdentityIntentError> {
        declaration.validate()?;
        let current = self.member_tool_declaration();
        let callback_tools = match &declaration.callback_tools {
            CallbackToolSetDeclaration::Inherit => match current.callback_tools {
                CallbackToolSetDeclaration::Set(tools) => tools,
                CallbackToolSetDeclaration::Inherit => {
                    unreachable!("durable member tool projection resolves callback inheritance")
                }
            },
            CallbackToolSetDeclaration::Set(tools) => tools.clone(),
        };
        let execution = match &declaration.execution {
            MemberToolAccessDeclaration::Inherit => current.execution,
            explicit => explicit.clone(),
        };
        let tool_access_policy = match execution {
            MemberToolAccessDeclaration::Inherit => {
                unreachable!("durable member tool projection resolves execution inheritance")
            }
            MemberToolAccessDeclaration::Unrestricted => None,
            MemberToolAccessDeclaration::Constraints(constraints) => {
                Some(WireResolvedToolAccessPolicy::Constraints(
                    constraints
                        .into_iter()
                        .map(|constraint| match constraint {
                            MemberToolAccessConstraint::AllowNames(names) => {
                                meerkat_contracts::wire::WireToolAccessConstraint::AllowNames(names)
                            }
                            MemberToolAccessConstraint::DenyNames(names) => {
                                meerkat_contracts::wire::WireToolAccessConstraint::DenyNames(names)
                            }
                            MemberToolAccessConstraint::ReadOnly => {
                                meerkat_contracts::wire::WireToolAccessConstraint::ReadOnly
                            }
                        })
                        .collect(),
                ))
            }
        };
        let application_policy = match &declaration.application_policy {
            ApplicationToolPolicyBinding::Inherit => current.application_policy,
            explicit => explicit.clone(),
        };

        let mut next = self.clone();
        next.overlay.tool_category_overrides = declaration.category_overrides;
        next.overlay.tool_access_policy = tool_access_policy;
        next.overlay.application_tool_policy = application_policy;
        next.required_local_callback_tools = callback_tools;
        next.validate()?;
        Ok(next)
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

/// Caller-chosen idempotency identity for one explicit expected-absent
/// adoption of an existing member into durable identity intent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdentityAdoptionId(String);

impl IdentityAdoptionId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentityIntentError> {
        let value = value.into();
        validate_text("identity_adoption_id", &value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Explicit initial-intent precondition. V1 deliberately has no upsert arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityAdoptionPrecondition {
    ExpectedAbsent,
}

/// Names the authority that owns local member topology after adoption.
///
/// `ExternalManaged` means the identity reconciler must abstain completely:
/// the mob-level topology provider remains the sole desired-state owner.
/// `IdentityOwned` enables exact comparison of the canonical `owned_wiring`
/// set and fail-closed repair classification.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityWiringCustody {
    #[default]
    ExternalManaged,
    IdentityOwned,
}

impl IdentityWiringCustody {
    fn is_external_managed(&self) -> bool {
        matches!(self, Self::ExternalManaged)
    }
}

/// Full caller-authored declaration for adopting one already-realized member.
///
/// The actor compiles `member` against its canonical `MobDefinition`, verifies
/// the exact current identity/session/role realization, and asks the store to
/// insert revision 1 only when no intent row exists. No roster inference can
/// supply omitted desired material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptMemberIdentityDeclaration {
    pub mob_id: MobId,
    pub agent_identity: AgentIdentity,
    pub request_id: IdentityAdoptionId,
    pub precondition: IdentityAdoptionPrecondition,
    pub declaration_scope: IdentityDeclarationScopeId,
    pub declaration_revision: u64,
    pub session: DesiredSessionTarget,
    pub member: IdentityProfileMemberDeclaration,
    #[serde(
        default,
        skip_serializing_if = "IdentityWiringCustody::is_external_managed"
    )]
    pub wiring_custody: IdentityWiringCustody,
    pub owned_wiring: BTreeSet<DesiredIdentityEdge>,
    pub convergence: IdentityConvergenceMode,
}

impl AdoptMemberIdentityDeclaration {
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_text("mob_id", self.mob_id.as_str())?;
        validate_identity(&self.agent_identity)?;
        validate_text("identity_adoption_id", self.request_id.as_str())?;
        validate_text(
            "identity_declaration_scope",
            self.declaration_scope.as_str(),
        )?;
        if self.declaration_revision == 0 {
            return Err(IdentityIntentError::InvalidDeclarationRevision);
        }
        self.member.validate()?;
        self.convergence.validate()?;
        if self.session.session_id.0.is_nil() {
            return Err(IdentityIntentError::NilSessionId);
        }
        if !matches!(
            self.session.authority_policy,
            DesiredSessionAuthorityPolicy::RequireExisting
        ) {
            return Err(IdentityIntentError::InvalidMutationTarget);
        }
        SessionLineageId::new(self.session.lineage_id.as_str().to_string()).map_err(|_| {
            IdentityIntentError::InvalidText {
                field: "session_lineage_id",
            }
        })?;
        if matches!(self.wiring_custody, IdentityWiringCustody::ExternalManaged)
            && !self.owned_wiring.is_empty()
        {
            return Err(IdentityIntentError::InvalidMutationTarget);
        }
        for edge in &self.owned_wiring {
            validate_identity(&edge.a)?;
            validate_identity(&edge.b)?;
            if edge.a >= edge.b {
                return Err(IdentityIntentError::NonCanonicalEdge(edge.clone()));
            }
            if edge.owner() != &self.agent_identity {
                return Err(IdentityIntentError::EdgeOwnedByDifferentIdentity {
                    identity: self.agent_identity.clone(),
                    edge: edge.clone(),
                });
            }
        }
        Ok(())
    }

    pub fn canonical_digest(&self) -> Result<String, IdentityIntentError> {
        self.validate()?;
        #[derive(Serialize)]
        struct DigestMaterial<'a> {
            domain: &'static str,
            request: &'a AdoptMemberIdentityDeclaration,
        }
        let bytes = serde_json::to_vec(&DigestMaterial {
            domain: "meerkat.identity.adoption.v1",
            request: self,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityAdoptionOutcome {
    Adopted { desired_revision: u64 },
    PreconditionConflict { actual_revision: u64 },
    RequestConflict { request_id: IdentityAdoptionId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptMemberIdentityDeclarationResult {
    pub adoption: IdentityAdoptionOutcome,
    pub convergence: IdentityConvergenceStatus,
}

impl IdentityAdoptionOutcome {
    #[must_use]
    pub fn to_wire(&self) -> meerkat_contracts::wire::WireIdentityAdoptionOutcome {
        use meerkat_contracts::wire::WireIdentityAdoptionOutcome as Wire;
        match self {
            Self::Adopted { desired_revision } => Wire::Adopted {
                desired_revision: *desired_revision,
            },
            Self::PreconditionConflict { actual_revision } => Wire::PreconditionConflict {
                actual_revision: *actual_revision,
            },
            Self::RequestConflict { request_id } => Wire::RequestConflict {
                request_id: request_id.as_str().to_string(),
            },
        }
    }
}

impl AdoptMemberIdentityDeclarationResult {
    #[must_use]
    pub fn to_wire(&self) -> meerkat_contracts::wire::MobAdoptMemberIdentityDeclarationResult {
        meerkat_contracts::wire::MobAdoptMemberIdentityDeclarationResult {
            adoption: self.adoption.to_wire(),
            convergence: self.convergence.to_wire(),
        }
    }
}

impl TryFrom<meerkat_contracts::wire::MobAdoptMemberIdentityDeclarationParams>
    for AdoptMemberIdentityDeclaration
{
    type Error = IdentityIntentError;

    fn try_from(
        value: meerkat_contracts::wire::MobAdoptMemberIdentityDeclarationParams,
    ) -> Result<Self, Self::Error> {
        use meerkat_contracts::wire::{
            WireDesiredExecution, WireDesiredSessionAuthorityPolicy,
            WireIdentityAdoptionPrecondition, WireIdentityWiringCustody,
        };

        let session_id = uuid::Uuid::parse_str(&value.session.session_id)
            .map(SessionId)
            .map_err(|error| IdentityIntentError::InvalidMemberMaterial(error.to_string()))?;
        let execution = match value.member.execution {
            WireDesiredExecution::ControllingSession => DesiredExecution::ControllingSession,
            WireDesiredExecution::AnyBoundHostSession => DesiredExecution::AnyBoundHostSession,
            WireDesiredExecution::PlacedSession { host_id } => {
                DesiredExecution::PlacedSession { host_id }
            }
            WireDesiredExecution::External { address, identity } => DesiredExecution::External {
                address: DesiredExternalAddress::parse(address)?,
                identity,
            },
        };
        let member = IdentityProfileMemberDeclaration {
            profile_name: ProfileName::from(value.member.profile_name),
            profile_override: value.member.profile_override,
            model_override: value.member.model_override,
            external_addressable_override: value.member.external_addressable_override,
            context: value.member.context,
            labels: value.member.labels,
            additional_instructions: value.member.additional_instructions,
            system_prompt_override: value.member.system_prompt_override,
            tool_access_policy: value.member.tool_access_policy,
            auth_binding: value.member.auth_binding,
            budget_limits: value.member.budget_limits,
            runtime_mode: value.member.runtime_mode,
            required_env_keys: value.member.required_env_keys,
            required_local_callback_tools: value
                .member
                .required_local_callback_tools
                .into_iter()
                .map(|tool| {
                    DesiredLocalCallbackTool::new(tool.name, tool.description, tool.input_schema)
                })
                .collect::<Result<Vec<_>, _>>()?,
            execution,
        };
        let request = Self {
            mob_id: MobId::from(value.mob_id),
            agent_identity: AgentIdentity::from(value.agent_identity),
            request_id: IdentityAdoptionId::new(value.request_id)?,
            precondition: match value.precondition {
                WireIdentityAdoptionPrecondition::ExpectedAbsent => {
                    IdentityAdoptionPrecondition::ExpectedAbsent
                }
            },
            declaration_scope: IdentityDeclarationScopeId::new(value.declaration_scope)?,
            declaration_revision: value.declaration_revision,
            session: DesiredSessionTarget {
                session_id,
                lineage_id: SessionLineageId::new(value.session.lineage_id).map_err(|_| {
                    IdentityIntentError::InvalidText {
                        field: "session_lineage_id",
                    }
                })?,
                lineage_generation: SessionGeneration::new(value.session.lineage_generation),
                authority_policy: match value.session.authority_policy {
                    WireDesiredSessionAuthorityPolicy::RequireExisting => {
                        DesiredSessionAuthorityPolicy::RequireExisting
                    }
                },
            },
            member,
            wiring_custody: match value.wiring_custody {
                WireIdentityWiringCustody::ExternalManaged => {
                    IdentityWiringCustody::ExternalManaged
                }
                WireIdentityWiringCustody::IdentityOwned => IdentityWiringCustody::IdentityOwned,
            },
            owned_wiring: value
                .owned_wiring
                .into_iter()
                .map(|edge| DesiredIdentityEdge {
                    a: AgentIdentity::from(edge.a),
                    b: AgentIdentity::from(edge.b),
                })
                .collect(),
            convergence: value.convergence.into(),
        };
        request.validate()?;
        Ok(request)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityAdoptionReceipt {
    pub schema_version: u32,
    pub mob_id: MobId,
    pub agent_identity: AgentIdentity,
    pub request_id: IdentityAdoptionId,
    pub request_digest: String,
    pub outcome: IdentityAdoptionOutcome,
    pub receipt_digest: String,
}

impl IdentityAdoptionReceipt {
    pub fn new(
        request: &AdoptMemberIdentityDeclaration,
        outcome: IdentityAdoptionOutcome,
    ) -> Result<Self, IdentityIntentError> {
        let mut receipt = Self {
            schema_version: IDENTITY_ADOPTION_RECEIPT_SCHEMA_VERSION,
            mob_id: request.mob_id.clone(),
            agent_identity: request.agent_identity.clone(),
            request_id: request.request_id.clone(),
            request_digest: request.canonical_digest()?,
            outcome,
            receipt_digest: String::new(),
        };
        receipt.receipt_digest = receipt.canonical_digest()?;
        receipt.validate()?;
        Ok(receipt)
    }

    fn canonical_digest(&self) -> Result<String, IdentityIntentError> {
        #[derive(Serialize)]
        struct DigestMaterial<'a> {
            domain: &'static str,
            schema_version: u32,
            mob_id: &'a MobId,
            agent_identity: &'a AgentIdentity,
            request_id: &'a IdentityAdoptionId,
            request_digest: &'a str,
            outcome: &'a IdentityAdoptionOutcome,
        }
        let bytes = serde_json::to_vec(&DigestMaterial {
            domain: "meerkat.identity.adoption_receipt.v1",
            schema_version: self.schema_version,
            mob_id: &self.mob_id,
            agent_identity: &self.agent_identity,
            request_id: &self.request_id,
            request_digest: &self.request_digest,
            outcome: &self.outcome,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        if self.schema_version != IDENTITY_ADOPTION_RECEIPT_SCHEMA_VERSION {
            return Err(IdentityIntentError::UnsupportedSchemaVersion {
                record: "identity_adoption_receipt",
                version: self.schema_version,
            });
        }
        validate_text("mob_id", self.mob_id.as_str())?;
        validate_identity(&self.agent_identity)?;
        validate_text("identity_adoption_id", self.request_id.as_str())?;
        validate_sha256_digest(&self.request_digest)?;
        if self.receipt_digest != self.canonical_digest()? {
            return Err(IdentityIntentError::DigestMismatch);
        }
        match &self.outcome {
            IdentityAdoptionOutcome::Adopted { desired_revision }
            | IdentityAdoptionOutcome::PreconditionConflict {
                actual_revision: desired_revision,
            } if *desired_revision == 0 => Err(IdentityIntentError::ZeroIntentRevision),
            IdentityAdoptionOutcome::RequestConflict { request_id }
                if request_id != &self.request_id =>
            {
                Err(IdentityIntentError::InvalidMutationTarget)
            }
            _ => Ok(()),
        }
    }
}

/// Seal revision 1 from the actor-compiled member material. Store backends
/// recompute incident-wiring cleanup evidence transactionally at insertion.
pub(crate) fn prepare_identity_adoption_record(
    request: &AdoptMemberIdentityDeclaration,
    material: DesiredMemberMaterial,
    committed_at_ms: u64,
) -> Result<IdentityIntentRecord, IdentityIntentError> {
    request.validate()?;
    material.validate()?;
    if material.profile_name != request.member.profile_name
        || material.execution != request.member.execution
    {
        return Err(IdentityIntentError::InvalidMutationTarget);
    }
    let member = DesiredMemberSpec {
        material,
        initial_delivery: None,
    };
    let intent = IdentityIntent::Present {
        identity: request.agent_identity.clone(),
        session: request.session.clone(),
        member: Box::new(member),
        wiring_custody: request.wiring_custody,
        owned_wiring: request.owned_wiring.clone(),
    };
    let mut record = IdentityIntentRecord {
        schema_version: IDENTITY_INTENT_SCHEMA_VERSION,
        mob_id: request.mob_id.clone(),
        intent_revision: 1,
        declaration_scope: Some(request.declaration_scope.clone()),
        declaration_revision: Some(request.declaration_revision),
        tombstone_generation: None,
        initial_delivery_generation_highwater: 0,
        retirement_plan: IdentityRetirementPlan::Targets {
            session: request.session.clone(),
            execution: request.member.execution.clone(),
            incident_wiring: request.owned_wiring.clone(),
        },
        convergence_directive: Some(IdentityConvergenceDirective {
            desired_revision: 1,
            expected_active_revision: 1,
            mode: request.convergence,
            drain_deadline_ms: match request.convergence {
                IdentityConvergenceMode::Drain { max_wait_ms } => {
                    Some(committed_at_ms.checked_add(max_wait_ms).ok_or(
                        IdentityIntentError::CounterExhausted {
                            counter: "identity_convergence_deadline_ms",
                        },
                    )?)
                }
                IdentityConvergenceMode::CancelActive => None,
            },
        }),
        intent_digest: intent.digest()?,
        authority_digest: String::new(),
        intent,
    };
    record.authority_digest = record.canonical_authority_digest()?;
    record.validate()?;
    Ok(record)
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
        #[serde(
            default,
            skip_serializing_if = "IdentityWiringCustody::is_external_managed"
        )]
        wiring_custody: IdentityWiringCustody,
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
            wiring_custody,
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
            if matches!(wiring_custody, IdentityWiringCustody::ExternalManaged)
                && !owned_wiring.is_empty()
            {
                return Err(IdentityIntentError::InvalidMutationTarget);
            }
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
    /// Provider scope/revision provenance sealed by the retired declaration
    /// facade. Both are absent or present together. Reconciliation never reads
    /// them as desired state.
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
    /// Durable convergence authority for replacing a realized member after an
    /// administrative desired-material update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convergence_directive: Option<IdentityConvergenceDirective>,
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
        if let Some(directive) = &self.convergence_directive {
            directive.mode.validate()?;
            if directive.desired_revision != self.intent_revision
                || directive.expected_active_revision == 0
                || match directive.mode {
                    IdentityConvergenceMode::Drain { .. } => directive.drain_deadline_ms.is_none(),
                    IdentityConvergenceMode::CancelActive => directive.drain_deadline_ms.is_some(),
                }
            {
                return Err(IdentityIntentError::InvalidConvergenceMode);
            }
        }
        match &self.intent {
            IdentityIntent::Present {
                member,
                session: _,
                identity: _,
                owned_wiring: _,
                wiring_custody: _,
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
        let legacy = AuthorityMaterial {
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
        };
        let bytes = if let Some(directive) = &self.convergence_directive {
            #[derive(Serialize)]
            struct ExtendedAuthorityMaterial<'a> {
                #[serde(flatten)]
                legacy: AuthorityMaterial<'a>,
                convergence_directive: &'a IdentityConvergenceDirective,
            }
            serde_json::to_vec(&ExtendedAuthorityMaterial {
                legacy,
                convergence_directive: directive,
            })
        } else {
            serde_json::to_vec(&legacy)
        }
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }
}

/// Caller-chosen idempotency identity for one desired-member mutation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemberToolMutationId(String);

impl MemberToolMutationId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentityIntentError> {
        let value = value.into();
        validate_text("member_tool_mutation_id", &value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MemberToolMutationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Requested treatment of already-admitted member work while desired
/// material converges. A finite drain never silently becomes cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityConvergenceMode {
    Drain { max_wait_ms: u64 },
    CancelActive,
}

/// Store-sealed replacement instruction surviving process restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConvergenceDirective {
    pub desired_revision: u64,
    pub expected_active_revision: u64,
    pub mode: IdentityConvergenceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drain_deadline_ms: Option<u64>,
}

impl IdentityConvergenceMode {
    pub fn validate(self) -> Result<(), IdentityIntentError> {
        if let Self::Drain { max_wait_ms } = self
            && (max_wait_ms == 0 || max_wait_ms > IDENTITY_CONVERGENCE_MAX_DRAIN_MS)
        {
            return Err(IdentityIntentError::InvalidConvergenceMode);
        }
        Ok(())
    }
}

impl From<meerkat_contracts::wire::WireIdentityConvergenceMode> for IdentityConvergenceMode {
    fn from(value: meerkat_contracts::wire::WireIdentityConvergenceMode) -> Self {
        match value {
            meerkat_contracts::wire::WireIdentityConvergenceMode::Drain { max_wait_ms } => {
                Self::Drain { max_wait_ms }
            }
            meerkat_contracts::wire::WireIdentityConvergenceMode::CancelActive => {
                Self::CancelActive
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyMemberToolDeclaration {
    pub mob_id: MobId,
    pub agent_identity: AgentIdentity,
    pub request_id: MemberToolMutationId,
    pub expected_intent_revision: u64,
    pub declaration: MemberToolDeclaration,
    pub convergence: IdentityConvergenceMode,
}

impl TryFrom<meerkat_contracts::wire::MobApplyMemberToolDeclarationParams>
    for ApplyMemberToolDeclaration
{
    type Error = IdentityIntentError;

    fn try_from(
        value: meerkat_contracts::wire::MobApplyMemberToolDeclarationParams,
    ) -> Result<Self, Self::Error> {
        let request = Self {
            mob_id: MobId::from(value.mob_id),
            agent_identity: AgentIdentity::from(value.agent_identity),
            request_id: MemberToolMutationId::new(value.request_id)?,
            expected_intent_revision: value.expected_intent_revision,
            declaration: value.declaration.try_into()?,
            convergence: value.convergence.into(),
        };
        request.validate()?;
        Ok(request)
    }
}

impl ApplyMemberToolDeclaration {
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_text("mob_id", self.mob_id.as_str())?;
        validate_identity(&self.agent_identity)?;
        validate_text("member_tool_mutation_id", self.request_id.as_str())?;
        if self.expected_intent_revision == 0 {
            return Err(IdentityIntentError::ZeroIntentRevision);
        }
        self.declaration.validate()?;
        self.convergence.validate()
    }

    pub fn canonical_digest(&self) -> Result<String, IdentityIntentError> {
        self.validate()?;
        #[derive(Serialize)]
        struct DigestMaterial<'a> {
            domain: &'static str,
            mob_id: &'a MobId,
            agent_identity: &'a AgentIdentity,
            request_id: &'a MemberToolMutationId,
            expected_intent_revision: u64,
            declaration: &'a MemberToolDeclaration,
            convergence: IdentityConvergenceMode,
        }
        let bytes = serde_json::to_vec(&DigestMaterial {
            domain: "meerkat.identity.member_tool_mutation.v1",
            mob_id: &self.mob_id,
            agent_identity: &self.agent_identity,
            request_id: &self.request_id,
            expected_intent_revision: self.expected_intent_revision,
            declaration: &self.declaration,
            convergence: self.convergence,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberToolCommitOutcome {
    Committed { desired_revision: u64 },
    NoChange { desired_revision: u64 },
    RevisionConflict { expected: u64, actual: u64 },
    RequestConflict { request_id: MemberToolMutationId },
    MemberAbsent,
    InvalidDeclaration { reason: String },
}

/// Immutable idempotency receipt for the desired-state commit only. Live
/// convergence is deliberately read fresh and never frozen into this row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityIntentMutationReceipt {
    pub schema_version: u32,
    pub mob_id: MobId,
    pub agent_identity: AgentIdentity,
    pub request_id: MemberToolMutationId,
    pub request_digest: String,
    pub outcome: MemberToolCommitOutcome,
    pub receipt_digest: String,
}

/// Caller-chosen idempotency identity for one blocked-convergence resolution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdentityConvergenceResolutionId(String);

impl IdentityConvergenceResolutionId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentityIntentError> {
        let value = value.into();
        validate_text("identity_convergence_resolution_id", &value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveIdentityConvergenceBlock {
    pub mob_id: MobId,
    pub agent_identity: AgentIdentity,
    pub request_id: IdentityConvergenceResolutionId,
    pub expected_desired_revision: u64,
    pub observed_active_revision: u64,
    pub convergence: IdentityConvergenceMode,
}

impl ResolveIdentityConvergenceBlock {
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_text("mob_id", self.mob_id.as_str())?;
        validate_identity(&self.agent_identity)?;
        validate_text(
            "identity_convergence_resolution_id",
            self.request_id.as_str(),
        )?;
        if self.expected_desired_revision == 0 || self.observed_active_revision == 0 {
            return Err(IdentityIntentError::ZeroIntentRevision);
        }
        self.convergence.validate()
    }

    pub fn canonical_digest(&self) -> Result<String, IdentityIntentError> {
        self.validate()?;
        #[derive(Serialize)]
        struct DigestMaterial<'a> {
            domain: &'static str,
            mob_id: &'a MobId,
            agent_identity: &'a AgentIdentity,
            request_id: &'a IdentityConvergenceResolutionId,
            expected_desired_revision: u64,
            observed_active_revision: u64,
            convergence: IdentityConvergenceMode,
        }
        let bytes = serde_json::to_vec(&DigestMaterial {
            domain: "meerkat.identity.convergence_resolution.v1",
            mob_id: &self.mob_id,
            agent_identity: &self.agent_identity,
            request_id: &self.request_id,
            expected_desired_revision: self.expected_desired_revision,
            observed_active_revision: self.observed_active_revision,
            convergence: self.convergence,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityConvergenceResolutionOutcome {
    Resolved {
        desired_revision: u64,
        active_revision: u64,
    },
    DesiredRevisionConflict {
        expected: u64,
        actual: u64,
    },
    ActiveRevisionConflict {
        expected: u64,
        actual: u64,
    },
    NotBlocked,
    MemberAbsent,
    RequestConflict {
        request_id: IdentityConvergenceResolutionId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConvergenceResolutionReceipt {
    pub schema_version: u32,
    pub mob_id: MobId,
    pub agent_identity: AgentIdentity,
    pub request_id: IdentityConvergenceResolutionId,
    pub request_digest: String,
    pub outcome: IdentityConvergenceResolutionOutcome,
    pub receipt_digest: String,
}

impl IdentityConvergenceResolutionReceipt {
    pub fn new(
        request: &ResolveIdentityConvergenceBlock,
        outcome: IdentityConvergenceResolutionOutcome,
    ) -> Result<Self, IdentityIntentError> {
        let mut receipt = Self {
            schema_version: IDENTITY_CONVERGENCE_RESOLUTION_RECEIPT_SCHEMA_VERSION,
            mob_id: request.mob_id.clone(),
            agent_identity: request.agent_identity.clone(),
            request_id: request.request_id.clone(),
            request_digest: request.canonical_digest()?,
            outcome,
            receipt_digest: String::new(),
        };
        receipt.receipt_digest = receipt.canonical_digest()?;
        receipt.validate()?;
        Ok(receipt)
    }

    fn canonical_digest(&self) -> Result<String, IdentityIntentError> {
        #[derive(Serialize)]
        struct DigestMaterial<'a> {
            domain: &'static str,
            schema_version: u32,
            mob_id: &'a MobId,
            agent_identity: &'a AgentIdentity,
            request_id: &'a IdentityConvergenceResolutionId,
            request_digest: &'a str,
            outcome: &'a IdentityConvergenceResolutionOutcome,
        }
        let bytes = serde_json::to_vec(&DigestMaterial {
            domain: "meerkat.identity.convergence_resolution_receipt.v1",
            schema_version: self.schema_version,
            mob_id: &self.mob_id,
            agent_identity: &self.agent_identity,
            request_id: &self.request_id,
            request_digest: &self.request_digest,
            outcome: &self.outcome,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        if self.schema_version != IDENTITY_CONVERGENCE_RESOLUTION_RECEIPT_SCHEMA_VERSION {
            return Err(IdentityIntentError::UnsupportedSchemaVersion {
                record: "identity_convergence_resolution_receipt",
                version: self.schema_version,
            });
        }
        validate_text("mob_id", self.mob_id.as_str())?;
        validate_identity(&self.agent_identity)?;
        validate_text(
            "identity_convergence_resolution_id",
            self.request_id.as_str(),
        )?;
        validate_sha256_digest(&self.request_digest)?;
        validate_sha256_digest(&self.receipt_digest)?;
        if self.canonical_digest()? != self.receipt_digest {
            return Err(IdentityIntentError::DigestMismatch);
        }
        Ok(())
    }
}

/// Pure store-side update preparation after the actor has supplied its exact
/// active-revision observation. This changes only convergence authority, not
/// desired member material or its intent revision.
pub fn prepare_identity_convergence_resolution(
    current: &IdentityIntentRecord,
    request: &ResolveIdentityConvergenceBlock,
    actual_active_revision: u64,
    committed_at_ms: u64,
) -> Result<
    (
        IdentityConvergenceResolutionOutcome,
        Option<IdentityIntentRecord>,
    ),
    IdentityIntentError,
> {
    current.validate()?;
    request.validate()?;
    if current.mob_id != request.mob_id || current.intent.identity() != &request.agent_identity {
        return Err(IdentityIntentError::InvalidMutationTarget);
    }
    if current.intent_revision != request.expected_desired_revision {
        return Ok((
            IdentityConvergenceResolutionOutcome::DesiredRevisionConflict {
                expected: request.expected_desired_revision,
                actual: current.intent_revision,
            },
            None,
        ));
    }
    if actual_active_revision != request.observed_active_revision {
        return Ok((
            IdentityConvergenceResolutionOutcome::ActiveRevisionConflict {
                expected: request.observed_active_revision,
                actual: actual_active_revision,
            },
            None,
        ));
    }
    if !matches!(current.intent, IdentityIntent::Present { .. }) {
        return Ok((IdentityConvergenceResolutionOutcome::MemberAbsent, None));
    }
    if current.convergence_directive.is_none() {
        return Ok((IdentityConvergenceResolutionOutcome::NotBlocked, None));
    }
    let drain_deadline_ms = match request.convergence {
        IdentityConvergenceMode::Drain { max_wait_ms } => {
            Some(committed_at_ms.checked_add(max_wait_ms).ok_or(
                IdentityIntentError::CounterExhausted {
                    counter: "identity_convergence_deadline_ms",
                },
            )?)
        }
        IdentityConvergenceMode::CancelActive => None,
    };
    let mut next = current.clone();
    next.convergence_directive = Some(IdentityConvergenceDirective {
        desired_revision: current.intent_revision,
        expected_active_revision: actual_active_revision,
        mode: request.convergence,
        drain_deadline_ms,
    });
    next.authority_digest = next.canonical_authority_digest()?;
    next.validate()?;
    Ok((
        IdentityConvergenceResolutionOutcome::Resolved {
            desired_revision: current.intent_revision,
            active_revision: actual_active_revision,
        },
        Some(next),
    ))
}

impl IdentityIntentMutationReceipt {
    pub fn new(
        request: &ApplyMemberToolDeclaration,
        outcome: MemberToolCommitOutcome,
    ) -> Result<Self, IdentityIntentError> {
        let mut receipt = Self {
            schema_version: IDENTITY_INTENT_MUTATION_RECEIPT_SCHEMA_VERSION,
            mob_id: request.mob_id.clone(),
            agent_identity: request.agent_identity.clone(),
            request_id: request.request_id.clone(),
            request_digest: request.canonical_digest()?,
            outcome,
            receipt_digest: String::new(),
        };
        receipt.receipt_digest = receipt.canonical_digest()?;
        receipt.validate()?;
        Ok(receipt)
    }

    fn canonical_digest(&self) -> Result<String, IdentityIntentError> {
        #[derive(Serialize)]
        struct DigestMaterial<'a> {
            domain: &'static str,
            schema_version: u32,
            mob_id: &'a MobId,
            agent_identity: &'a AgentIdentity,
            request_id: &'a MemberToolMutationId,
            request_digest: &'a str,
            outcome: &'a MemberToolCommitOutcome,
        }
        let bytes = serde_json::to_vec(&DigestMaterial {
            domain: "meerkat.identity.intent_mutation_receipt.v1",
            schema_version: self.schema_version,
            mob_id: &self.mob_id,
            agent_identity: &self.agent_identity,
            request_id: &self.request_id,
            request_digest: &self.request_digest,
            outcome: &self.outcome,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        if self.schema_version != IDENTITY_INTENT_MUTATION_RECEIPT_SCHEMA_VERSION {
            return Err(IdentityIntentError::UnsupportedSchemaVersion {
                record: "identity_intent_mutation_receipt",
                version: self.schema_version,
            });
        }
        validate_text("mob_id", self.mob_id.as_str())?;
        validate_identity(&self.agent_identity)?;
        validate_text("member_tool_mutation_id", self.request_id.as_str())?;
        validate_sha256_digest(&self.request_digest)?;
        validate_sha256_digest(&self.receipt_digest)?;
        if self.canonical_digest()? != self.receipt_digest {
            return Err(IdentityIntentError::DigestMismatch);
        }
        Ok(())
    }
}

/// Store-side pure preparation for the atomic intent plus receipt write.
pub fn prepare_member_tool_intent_mutation(
    current: &IdentityIntentRecord,
    request: &ApplyMemberToolDeclaration,
    committed_at_ms: u64,
) -> Result<(MemberToolCommitOutcome, Option<IdentityIntentRecord>), IdentityIntentError> {
    current.validate()?;
    request.validate()?;
    if current.mob_id != request.mob_id || current.intent.identity() != &request.agent_identity {
        return Err(IdentityIntentError::InvalidMutationTarget);
    }
    if current.intent_revision != request.expected_intent_revision {
        return Ok((
            MemberToolCommitOutcome::RevisionConflict {
                expected: request.expected_intent_revision,
                actual: current.intent_revision,
            },
            None,
        ));
    }
    let IdentityIntent::Present {
        identity,
        session,
        member,
        wiring_custody,
        owned_wiring,
    } = &current.intent
    else {
        return Ok((MemberToolCommitOutcome::MemberAbsent, None));
    };
    let next_material = member
        .material
        .with_member_tool_declaration(&request.declaration)?;
    if next_material == member.material {
        return Ok((
            MemberToolCommitOutcome::NoChange {
                desired_revision: current.intent_revision,
            },
            None,
        ));
    }
    let next_revision =
        current
            .intent_revision
            .checked_add(1)
            .ok_or(IdentityIntentError::CounterExhausted {
                counter: "intent_revision",
            })?;
    let mut next = current.clone();
    next.intent_revision = next_revision;
    let drain_deadline_ms = match request.convergence {
        IdentityConvergenceMode::Drain { max_wait_ms } => {
            Some(committed_at_ms.checked_add(max_wait_ms).ok_or(
                IdentityIntentError::CounterExhausted {
                    counter: "identity_convergence_deadline_ms",
                },
            )?)
        }
        IdentityConvergenceMode::CancelActive => None,
    };
    next.convergence_directive = Some(IdentityConvergenceDirective {
        desired_revision: next_revision,
        expected_active_revision: current.intent_revision,
        mode: request.convergence,
        drain_deadline_ms,
    });
    next.intent = IdentityIntent::Present {
        identity: identity.clone(),
        session: session.clone(),
        member: Box::new(DesiredMemberSpec {
            material: next_material,
            initial_delivery: member.initial_delivery.clone(),
        }),
        wiring_custody: *wiring_custody,
        owned_wiring: owned_wiring.clone(),
    };
    next.intent_digest = next.intent.digest()?;
    next.authority_digest = next.canonical_authority_digest()?;
    next.validate()?;
    Ok((
        MemberToolCommitOutcome::Committed {
            desired_revision: next_revision,
        },
        Some(next),
    ))
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
    SessionCreationConsumed,
    RetirementProven,
    ExternalBinding,
    InitialDelivery,
}

/// Identity scope of one immutable actuator receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "subject", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityOperationSubject {
    Identity { identity: AgentIdentity },
}

/// Stable lookup slot for an immutable receipt. One-shot slots deliberately
/// exclude content, so a different payload for the same semantic slot is a
/// typed conflict rather than a second effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "slot", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityOperationSlot {
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
    /// Sealed only after the first exact target session is observed. Once
    /// present, later absence is evidence loss and can never recreate history.
    SessionCreationConsumed {
        authority: IdentitySessionStoreAuthority,
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
            SessionCreationConsumed {
                tombstone_generation: u64,
                authority: &'a IdentitySessionStoreAuthority,
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
            IdentityOperationReceiptPayload::SessionCreationConsumed { authority } => {
                ReceiptRequestMaterial::SessionCreationConsumed {
                    tombstone_generation: self.tombstone_generation.unwrap_or(0),
                    authority,
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
        let IdentityOperationSubject::Identity { identity } = &self.subject;
        validate_identity(identity)?;
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
            IdentityOperationReceiptPayload::SessionCreationConsumed { authority } => {
                validate_identity_receipt_authority(self)?;
                let IdentityOperationSlot::SessionCreationConsumed {
                    tombstone_generation,
                    session_id,
                    lineage_id,
                    lineage_generation: _,
                } = &self.slot
                else {
                    return Err(IdentityIntentError::InvalidOperationReceipt);
                };
                authority.validate()?;
                if *tombstone_generation != normalized_tombstone
                    || authority.session_id() != session_id
                    || lineage_id.as_str().is_empty()
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
    if receipt.intent_revision.is_none_or(|revision| revision == 0) {
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
    pub replacement: IdentityReplacementCondition,
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
    CloseMemberAdmission,
    AwaitMemberDrain,
    DrainBlocked,
    CancelActiveMember,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityReplacementCondition {
    NotRequired,
    AdmissionOpen,
    Draining,
    DrainBlocked,
    CancelActive,
    Ready,
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum IdentitySessionObservationState {
    Unavailable {
        detail: String,
    },
    Missing {
        absence_version: String,
    },
    Matching {
        authority: IdentitySessionStoreAuthority,
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

/// Store-authorized session observation. The generated classifier receives
/// only [`IdentitySessionCondition`]; the actor retains the exact private
/// physical authority for receipt custody and bounded convergence checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySessionObservation {
    state: IdentitySessionObservationState,
}

impl IdentitySessionObservation {
    pub(crate) fn matching_authority(
        desired: &DesiredSessionTarget,
        authority: IdentitySessionStoreAuthority,
    ) -> Result<Self, IdentityIntentError> {
        validate_session_target(desired)?;
        authority.validate()?;
        if authority.session_id() != &desired.session_id {
            return Err(IdentityIntentError::SessionStoreAuthorityMismatch);
        }
        Ok(Self {
            state: IdentitySessionObservationState::Matching { authority },
        })
    }

    pub(crate) fn matching(
        desired: &DesiredSessionTarget,
        observed: &Session,
        authority: IdentitySessionStoreAuthority,
    ) -> Result<Self, IdentityIntentError> {
        validate_session_target(desired)?;
        authority.validate()?;
        if observed.id() != &desired.session_id || authority.session_id() != &desired.session_id {
            return Err(IdentityIntentError::SessionStoreAuthorityMismatch);
        }
        Ok(Self {
            state: IdentitySessionObservationState::Matching { authority },
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
            IdentitySessionObservationState::Matching { authority } => {
                Some(IdentityTargetObservationVersion::Version {
                    version: authority.observation_version()?,
                })
            }
            IdentitySessionObservationState::Malformed { version, .. }
            | IdentitySessionObservationState::IrrecoverablyCorrupt { version, .. } => {
                Some(IdentityTargetObservationVersion::Version {
                    version: version.clone(),
                })
            }
            IdentitySessionObservationState::AmbiguousDivergence { target, .. } => {
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

    /// Return the exact store-issued authority carried by a matching
    /// observation. No Session payload fact can construct this carrier.
    #[must_use]
    pub fn store_authority(&self) -> Option<&IdentitySessionStoreAuthority> {
        match &self.state {
            IdentitySessionObservationState::Matching { authority } => Some(authority),
            IdentitySessionObservationState::Unavailable { .. }
            | IdentitySessionObservationState::Missing { .. }
            | IdentitySessionObservationState::AmbiguousDivergence { .. }
            | IdentitySessionObservationState::Malformed { .. }
            | IdentitySessionObservationState::MalformedUnversioned { .. }
            | IdentitySessionObservationState::IrrecoverablyCorrupt { .. } => None,
        }
    }
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
    DrainBlocked,
}

/// Replaceable, output-only diagnostic.  It is never read by the classifier
/// and never grants an actuator permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConvergenceStatus {
    pub identity: AgentIdentity,
    pub intent_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_intent_revision: Option<u64>,
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
                | IdentityReconcileDecision::AwaitMemberDrain
                | IdentityReconcileDecision::AwaitExternalBindingCeremony
                | IdentityReconcileDecision::AwaitInitialDelivery,
            ) => IdentityConvergenceCondition::Suspended,
            Some(IdentityReconcileDecision::DrainBlocked) => {
                IdentityConvergenceCondition::DrainBlocked
            }
            Some(IdentityReconcileDecision::Converged) => IdentityConvergenceCondition::Converged,
            Some(IdentityReconcileDecision::Tombstoned) => IdentityConvergenceCondition::Tombstoned,
            Some(IdentityReconcileDecision::Quarantined) => {
                IdentityConvergenceCondition::Quarantined
            }
            Some(
                IdentityReconcileDecision::AcquireLease
                | IdentityReconcileDecision::CloseMemberAdmission
                | IdentityReconcileDecision::CancelActiveMember
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

    #[must_use]
    pub fn to_wire(&self) -> meerkat_contracts::wire::WireIdentityConvergenceStatus {
        use meerkat_contracts::wire::{
            WireIdentityConvergenceCondition as Condition, WireIdentityConvergenceStatus as Status,
            WireIdentityReconcileDecision as Decision,
        };
        let decision = self.decision.map(|decision| match decision {
            IdentityReconcileDecision::Backoff => Decision::Backoff,
            IdentityReconcileDecision::RepairBlocked => Decision::RepairBlocked,
            IdentityReconcileDecision::AcquireLease => Decision::AcquireLease,
            IdentityReconcileDecision::AwaitLease => Decision::AwaitLease,
            IdentityReconcileDecision::CloseMemberAdmission => Decision::CloseMemberAdmission,
            IdentityReconcileDecision::AwaitMemberDrain => Decision::AwaitMemberDrain,
            IdentityReconcileDecision::DrainBlocked => Decision::DrainBlocked,
            IdentityReconcileDecision::CancelActiveMember => Decision::CancelActiveMember,
            IdentityReconcileDecision::SealRetirementProven => Decision::SealRetirementProven,
            IdentityReconcileDecision::SealSessionCreationConsumed => {
                Decision::SealSessionCreationConsumed
            }
            IdentityReconcileDecision::EnsureSessionAuthority => Decision::EnsureSessionAuthority,
            IdentityReconcileDecision::EnsureRuntimeRegistration => {
                Decision::EnsureRuntimeRegistration
            }
            IdentityReconcileDecision::AwaitExternalBindingCeremony => {
                Decision::AwaitExternalBindingCeremony
            }
            IdentityReconcileDecision::EnsureExternalBindingReceipt => {
                Decision::EnsureExternalBindingReceipt
            }
            IdentityReconcileDecision::EnsureExternalBinding => Decision::EnsureExternalBinding,
            IdentityReconcileDecision::EnsureMemberMaterialization => {
                Decision::EnsureMemberMaterialization
            }
            IdentityReconcileDecision::EnsureInitialDeliveryReceipt => {
                Decision::EnsureInitialDeliveryReceipt
            }
            IdentityReconcileDecision::EnsureInitialDelivery => Decision::EnsureInitialDelivery,
            IdentityReconcileDecision::AwaitInitialDelivery => Decision::AwaitInitialDelivery,
            IdentityReconcileDecision::ReconcileWiring => Decision::ReconcileWiring,
            IdentityReconcileDecision::RetireMemberMaterialization => {
                Decision::RetireMemberMaterialization
            }
            IdentityReconcileDecision::RetireRuntimeRegistration => {
                Decision::RetireRuntimeRegistration
            }
            IdentityReconcileDecision::ReleaseSessionAuthority => Decision::ReleaseSessionAuthority,
            IdentityReconcileDecision::Converged => Decision::Converged,
            IdentityReconcileDecision::Tombstoned => Decision::Tombstoned,
            IdentityReconcileDecision::Quarantined => Decision::Quarantined,
        });
        let condition = match self.condition() {
            IdentityConvergenceCondition::Pending => Condition::Pending,
            IdentityConvergenceCondition::Reconciling => Condition::Reconciling,
            IdentityConvergenceCondition::Converged => Condition::Converged,
            IdentityConvergenceCondition::Backoff => Condition::Backoff,
            IdentityConvergenceCondition::RepairBlocked => Condition::RepairBlocked,
            IdentityConvergenceCondition::Quarantined => Condition::Quarantined,
            IdentityConvergenceCondition::Tombstoned => Condition::Tombstoned,
            IdentityConvergenceCondition::Suspended => Condition::Suspended,
            IdentityConvergenceCondition::DrainBlocked => Condition::DrainBlocked,
        };
        Status {
            agent_identity: self.identity.to_string(),
            desired_intent_revision: self.intent_revision,
            active_intent_revision: self.active_intent_revision,
            decision,
            condition,
            observed_at_ms: self.observed_at_ms,
            detail: self.detail.clone(),
        }
    }
}

impl MemberToolCommitOutcome {
    #[must_use]
    pub fn to_wire(&self) -> meerkat_contracts::wire::WireMemberToolCommitOutcome {
        use meerkat_contracts::wire::WireMemberToolCommitOutcome as Wire;
        match self {
            Self::Committed { desired_revision } => Wire::Committed {
                desired_revision: *desired_revision,
            },
            Self::NoChange { desired_revision } => Wire::NoChange {
                desired_revision: *desired_revision,
            },
            Self::RevisionConflict { expected, actual } => Wire::RevisionConflict {
                expected: *expected,
                actual: *actual,
            },
            Self::RequestConflict { request_id } => Wire::RequestConflict {
                request_id: request_id.to_string(),
            },
            Self::MemberAbsent => Wire::MemberAbsent,
            Self::InvalidDeclaration { reason } => Wire::InvalidDeclaration {
                reason: reason.clone(),
            },
        }
    }
}

impl IdentityConvergenceResolutionOutcome {
    #[must_use]
    pub fn to_wire(&self) -> meerkat_contracts::wire::WireIdentityConvergenceResolutionOutcome {
        use meerkat_contracts::wire::WireIdentityConvergenceResolutionOutcome as Wire;
        match self {
            Self::Resolved {
                desired_revision,
                active_revision,
            } => Wire::Resolved {
                desired_revision: *desired_revision,
                active_revision: *active_revision,
            },
            Self::DesiredRevisionConflict { expected, actual } => Wire::DesiredRevisionConflict {
                expected: *expected,
                actual: *actual,
            },
            Self::ActiveRevisionConflict { expected, actual } => Wire::ActiveRevisionConflict {
                expected: *expected,
                actual: *actual,
            },
            Self::NotBlocked => Wire::NotBlocked,
            Self::MemberAbsent => Wire::MemberAbsent,
            Self::RequestConflict { request_id } => Wire::RequestConflict {
                request_id: request_id.as_str().to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyMemberToolDeclarationResult {
    pub commit: MemberToolCommitOutcome,
    pub convergence: IdentityConvergenceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveIdentityConvergenceBlockResult {
    pub outcome: IdentityConvergenceResolutionOutcome,
    pub convergence: IdentityConvergenceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityIntentError {
    InvalidText {
        field: &'static str,
    },
    NilSessionId,
    CreateRequiresInitialGeneration,
    InvalidExternalAddress(String),
    InvalidExternalIdentity(String),
    SelfEdge(AgentIdentity),
    NonCanonicalEdge(DesiredIdentityEdge),
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
    InvalidMemberMaterial(String),
    InvalidMemberToolDeclaration(String),
    InvalidConvergenceMode,
    InvalidMutationTarget,
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
    InvalidSessionStoreAuthority,
    SessionStoreAuthorityMismatch,
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
            Self::InvalidMemberMaterial(detail) => {
                write!(formatter, "invalid desired member material: {detail}")
            }
            Self::InvalidMemberToolDeclaration(detail) => {
                write!(formatter, "invalid member tool declaration: {detail}")
            }
            Self::InvalidConvergenceMode => write!(
                formatter,
                "identity convergence drain must be between 1 and {IDENTITY_CONVERGENCE_MAX_DRAIN_MS}ms"
            ),
            Self::InvalidMutationTarget => formatter
                .write_str("member tool mutation does not match its physical mob and identity"),
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
            Self::InvalidSessionStoreAuthority => formatter
                .write_str("identity session store authority is internally incoherent"),
            Self::SessionStoreAuthorityMismatch => formatter.write_str(
                "store-issued session authority does not match the observed desired session",
            ),
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

fn has_prefixed_sha256(value: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| {
        value.strip_prefix(prefix).is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    })
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
pub(crate) fn identity_adoption_fixture(
    mob_id: MobId,
    agent_identity: AgentIdentity,
    request_id: &str,
    session_id: SessionId,
) -> (AdoptMemberIdentityDeclaration, IdentityIntentRecord) {
    use meerkat_contracts::wire::PortableToolConfig;

    let profile_name = ProfileName::from("worker");
    let profile = PortableProfile {
        model: "test-model".to_string(),
        provider: meerkat_core::Provider::Anthropic,
        self_hosted_server_id: None,
        image_generation_provider: None,
        auto_compact_threshold: None,
        resume_overrides: Vec::new(),
        skills: Vec::new(),
        tools: PortableToolConfig::default(),
        peer_description: String::new(),
        external_addressable: false,
        runtime_mode: WireMobRuntimeMode::AutonomousHost,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: None,
    };
    let execution = DesiredExecution::ControllingSession;
    let member = IdentityProfileMemberDeclaration {
        profile_name: profile_name.clone(),
        profile_override: Some(profile.clone()),
        model_override: None,
        external_addressable_override: None,
        context: None,
        labels: None,
        additional_instructions: None,
        system_prompt_override: Some(PortableSystemPrompt::Disable),
        tool_access_policy: None,
        auth_binding: None,
        budget_limits: None,
        runtime_mode: Some(WireMobRuntimeMode::AutonomousHost),
        required_env_keys: Vec::new(),
        required_local_callback_tools: Vec::new(),
        execution: execution.clone(),
    };
    let request = AdoptMemberIdentityDeclaration {
        mob_id,
        agent_identity,
        request_id: IdentityAdoptionId::new(request_id).expect("valid adoption id"),
        precondition: IdentityAdoptionPrecondition::ExpectedAbsent,
        declaration_scope: IdentityDeclarationScopeId::new("test-snapshot")
            .expect("valid declaration scope"),
        declaration_revision: 1,
        session: DesiredSessionTarget {
            lineage_id: SessionLineageId::for_session(&session_id),
            lineage_generation: SessionGeneration::INITIAL,
            session_id,
            authority_policy: DesiredSessionAuthorityPolicy::RequireExisting,
        },
        member,
        wiring_custody: IdentityWiringCustody::ExternalManaged,
        owned_wiring: BTreeSet::new(),
        convergence: IdentityConvergenceMode::Drain { max_wait_ms: 1_000 },
    };
    let material = DesiredMemberMaterial {
        profile_name,
        profile,
        definition_extract: PortableDefinitionExtract {
            profile_names: vec!["worker".to_string()],
            ..PortableDefinitionExtract::default()
        },
        overlay: DesiredMemberOverlay {
            context: None,
            labels: None,
            additional_instructions: None,
            system_prompt: Some(PortableSystemPrompt::Disable),
            tool_access_policy: None,
            tool_category_overrides: ToolCategoryOverrides::default(),
            application_tool_policy: ApplicationToolPolicyBinding::Unmanaged,
            auth_binding: None,
            budget_limits: None,
            runtime_mode: WireMobRuntimeMode::AutonomousHost,
        },
        required_env_keys: Vec::new(),
        required_local_callback_tools: Vec::new(),
        execution,
    };
    let record =
        prepare_identity_adoption_record(&request, material, 10).expect("valid adoption record");
    (request, record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_tool_apply_wire_lowers_through_one_validated_domain_boundary() {
        let declaration = MemberToolDeclaration {
            category_overrides: ToolCategoryOverrides::default(),
            callback_tools: CallbackToolSetDeclaration::Inherit,
            execution: MemberToolAccessDeclaration::Unrestricted,
            application_policy: ApplicationToolPolicyBinding::Unmanaged,
        };
        let params = meerkat_contracts::wire::MobApplyMemberToolDeclarationParams {
            mob_id: "tool-policy-mob".to_string(),
            agent_identity: "worker-1".to_string(),
            request_id: "apply-tool-policy-1".to_string(),
            expected_intent_revision: 7,
            declaration: declaration.to_wire(),
            convergence: meerkat_contracts::wire::WireIdentityConvergenceMode::CancelActive,
        };

        let request = ApplyMemberToolDeclaration::try_from(params.clone())
            .expect("lower valid wire request through the domain boundary");
        assert_eq!(request.mob_id.as_str(), params.mob_id);
        assert_eq!(
            request.agent_identity,
            AgentIdentity::from(params.agent_identity.as_str())
        );
        assert_eq!(request.request_id.as_str(), params.request_id.as_str());
        assert_eq!(
            request.expected_intent_revision,
            params.expected_intent_revision
        );
        assert_eq!(request.declaration, declaration);
        assert_eq!(request.convergence, IdentityConvergenceMode::CancelActive);

        let invalid = meerkat_contracts::wire::MobApplyMemberToolDeclarationParams {
            expected_intent_revision: 0,
            ..params
        };
        assert!(matches!(
            ApplyMemberToolDeclaration::try_from(invalid),
            Err(IdentityIntentError::ZeroIntentRevision)
        ));
    }

    #[test]
    fn identity_adoption_wire_is_strict_and_round_trips_full_declaration() {
        let (request, compiled) = identity_adoption_fixture(
            MobId::from("adoption-wire-mob"),
            AgentIdentity::from("worker-1"),
            "adoption-wire-request",
            SessionId::new(),
        );
        let persisted_intent = serde_json::to_value(&compiled.intent)
            .expect("serialize backward-compatible identity intent");
        assert!(
            persisted_intent.get("wiring_custody").is_none(),
            "default external custody must preserve the pre-field intent digest shape"
        );
        let value = serde_json::to_value(&request).expect("serialize adoption declaration");
        let wire: meerkat_contracts::wire::MobAdoptMemberIdentityDeclarationParams =
            serde_json::from_value(value.clone()).expect("decode strict wire declaration");
        let decoded = AdoptMemberIdentityDeclaration::try_from(wire)
            .expect("lower wire declaration into durable domain request");
        assert_eq!(decoded, request);

        let mut without_custody =
            serde_json::to_value(&request).expect("serialize default-custody adoption declaration");
        without_custody
            .as_object_mut()
            .expect("adoption request is an object")
            .remove("wiring_custody");
        let defaulted_wire: meerkat_contracts::wire::MobAdoptMemberIdentityDeclarationParams =
            serde_json::from_value(without_custody).expect("default external wiring custody");
        assert_eq!(
            AdoptMemberIdentityDeclaration::try_from(defaulted_wire)
                .expect("lower default custody")
                .wiring_custody,
            IdentityWiringCustody::ExternalManaged
        );

        let mut invalid = request.clone();
        invalid.owned_wiring.insert(
            DesiredIdentityEdge::new(
                invalid.agent_identity.clone(),
                AgentIdentity::from("worker-2"),
            )
            .expect("canonical edge"),
        );
        assert!(matches!(
            invalid.validate(),
            Err(IdentityIntentError::InvalidMutationTarget)
        ));

        let mut with_unknown = value;
        with_unknown
            .as_object_mut()
            .expect("adoption request is an object")
            .insert("ambient_default".to_string(), serde_json::json!(true));
        assert!(
            serde_json::from_value::<
                meerkat_contracts::wire::MobAdoptMemberIdentityDeclarationParams,
            >(with_unknown)
            .is_err()
        );
    }

    fn matching_facts() -> IdentityReconcileFacts {
        IdentityReconcileFacts {
            intent: IdentityAuthorityCondition::PresentRequireExisting,
            lease: IdentityLeaseCondition::HeldByCurrentIncarnation,
            replacement: IdentityReplacementCondition::NotRequired,
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
        assert_eq!(observation.store_authority(), None);
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

    fn session_creation_receipt(
        session_id: SessionId,
        authority: IdentitySessionStoreAuthority,
    ) -> IdentityOperationReceipt {
        let mut receipt = IdentityOperationReceipt {
            schema_version: IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION,
            mob_id: MobId::from("authority-receipt-mob"),
            subject: IdentityOperationSubject::Identity {
                identity: AgentIdentity::from("authority-receipt-member"),
            },
            effect_kind: IdentityOperationKind::SessionCreationConsumed,
            slot: IdentityOperationSlot::SessionCreationConsumed {
                tombstone_generation: 0,
                lineage_id: SessionLineageId::for_session(&session_id),
                lineage_generation: SessionGeneration::INITIAL,
                session_id,
            },
            receipt_id: OperationId::new(),
            intent_revision: Some(1),
            intent_digest: Some(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
            ),
            intent_authority_digest: Some(
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .to_string(),
            ),
            tombstone_generation: None,
            audit_lease_epoch: Some(1),
            request_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
            payload: IdentityOperationReceiptPayload::SessionCreationConsumed { authority },
        };
        receipt.request_digest = receipt.canonical_request_digest().unwrap();
        receipt
    }

    #[test]
    fn session_creation_receipt_carries_only_exact_store_authority() {
        let session_id = SessionId::new();
        let authority = IdentitySessionStoreAuthority::whole_blob_for_test(
            session_id.clone(),
            7,
            "row-sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let receipt = session_creation_receipt(session_id.clone(), authority.clone());

        receipt.validate().unwrap();
        let encoded = serde_json::to_value(&receipt).unwrap();
        assert_eq!(
            encoded.pointer("/payload/authority/store_revision"),
            Some(&serde_json::json!(7)),
        );
        assert_eq!(
            encoded.pointer("/payload/authority/token/profile"),
            Some(&serde_json::json!("whole_blob_v1")),
        );
        assert!(
            encoded.pointer("/payload/authority/checkpoint").is_none(),
            "receipt authority must not retain Session-owned checkpoint vocabulary",
        );

        let wrong_authority = IdentitySessionStoreAuthority::whole_blob_for_test(
            SessionId::new(),
            8,
            "row-sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        let wrong = session_creation_receipt(session_id, wrong_authority);
        assert!(matches!(
            wrong.validate(),
            Err(IdentityIntentError::InvalidOperationReceipt),
        ));
    }

    #[test]
    fn unversioned_session_corruption_reuses_strict_malformed_condition_serde() {
        let observation =
            IdentitySessionObservation::malformed_unversioned("unsupported persisted schema")
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
    fn replacement_conditions_have_one_generated_decision_each() {
        for (replacement, expected) in [
            (
                IdentityReplacementCondition::AdmissionOpen,
                IdentityReconcileDecision::CloseMemberAdmission,
            ),
            (
                IdentityReplacementCondition::Draining,
                IdentityReconcileDecision::AwaitMemberDrain,
            ),
            (
                IdentityReplacementCondition::DrainBlocked,
                IdentityReconcileDecision::DrainBlocked,
            ),
            (
                IdentityReplacementCondition::CancelActive,
                IdentityReconcileDecision::CancelActiveMember,
            ),
            (
                IdentityReplacementCondition::Ready,
                IdentityReconcileDecision::RetireMemberMaterialization,
            ),
        ] {
            let mut facts = matching_facts();
            facts.replacement = replacement;
            assert_eq!(classify_identity_reconciliation(facts), expected);
        }
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
    fn create_if_absent_is_consumed_after_the_first_matching_store_authority() {
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
            convergence_directive: None,
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
