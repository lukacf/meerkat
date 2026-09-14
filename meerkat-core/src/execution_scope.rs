//! Neutral persisted execution-scope content and non-deserializable handles.
//!
//! Records are not permits. Generated runtime authority must validate their
//! committed origin and current fences before installing a live handle.

mod accounting;
mod dispatch;
mod model;
pub use accounting::{ScopedEffectTokenAccounting, ScopedTokenAccountingStatus};
pub(crate) use dispatch::callback_tool_effect_id;
pub use dispatch::{
    RunExecutionContext, ScopedEffectCustody, ScopedEffectFeedback, ScopedEffectHost,
    ScopedEffectOutcome, ScopedEffectSettlement, ScopedExecutionContext, ScopedToolEffectSupport,
};
pub use model::{
    EvaluatedModelRequestPolicy, ScopedModelAttemptResolution, ScopedModelEffectCustody,
    ScopedModelEffectSupport, ScopedModelPreparationError, ScopedModelRequest,
};

use std::collections::BTreeSet;
use std::num::NonZeroU64;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::lifecycle::{InputId, RunId};
use crate::ops::{OperationId, ToolAccessConstraint, ToolAccessPolicy};
use crate::{
    ProviderNativeToolPolicy, RealmId, RuntimeEpochId, SessionId, ToolExecutionPolicy,
    ToolMutationClass, ToolNameSet,
};

macro_rules! scope_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(uuid::Uuid);

        impl $name {
            pub const fn from_uuid(value: uuid::Uuid) -> Self {
                Self(value)
            }
            pub const fn as_uuid(&self) -> &uuid::Uuid {
                &self.0
            }
        }
    };
}

scope_id!(RunEffectScopeId);
scope_id!(ExecutionGrantId);
scope_id!(ScopedEffectClaimId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopedEffectKind {
    ModelComputation,
    ToolDispatch,
    DescendantAdmission,
}

/// Content identity of the resolved physical invocation, not permission.
/// The digest includes the owning adapter's exact arguments and target binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopedEffectTarget {
    ModelComputation {
        request_id: OperationId,
        attempt: u32,
        invocation_digest: [u8; 32],
    },
    ToolDispatch {
        call_id: String,
        tool: crate::ToolName,
        invocation_digest: [u8; 32],
        mutation: ToolMutationClass,
    },
    DescendantAdmission {
        invocation_digest: [u8; 32],
    },
}

/// Stable logical lineage within one persisted run scope, independent of route.
pub fn model_attempt_chain_id(scope: RunEffectScopeId, request: &OperationId) -> OperationId {
    OperationId(uuid::Uuid::new_v5(scope.as_uuid(), request.0.as_bytes()))
}

impl ScopedEffectTarget {
    pub const fn kind(&self) -> ScopedEffectKind {
        match self {
            Self::ModelComputation { .. } => ScopedEffectKind::ModelComputation,
            Self::ToolDispatch { .. } => ScopedEffectKind::ToolDispatch,
            Self::DescendantAdmission { .. } => ScopedEffectKind::DescendantAdmission,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionGrantRef {
    pub id: ExecutionGrantId,
    pub issuer_realm: RealmId,
    pub generation: NonZeroU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedExecutorBinding {
    pub session_id: SessionId,
    pub realm: RealmId,
    pub runtime_epoch: RuntimeEpochId,
    /// Exact runtime binding generation, including zero for session-owned bindings.
    pub binding_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAdmissionCommitRef {
    pub revision: NonZeroU64,
    pub digest: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEffectScopeFormat {
    V1,
}

/// Residual counters include zero: exhausted permission must survive restart
/// as exhausted rather than falling back to session-wide launch defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedEffectBudget {
    pub model_computations: u64,
    pub tool_dispatches: u64,
    pub descendant_admissions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ScopedRunPolicyWire")]
pub struct ScopedRunPolicyRecord {
    tool_access: ToolAccessPolicy,
    allowed_mutations: BTreeSet<ToolMutationClass>,
}

impl ScopedRunPolicyRecord {
    pub fn new(
        tool_access: ToolAccessPolicy,
        allowed_mutations: BTreeSet<ToolMutationClass>,
    ) -> Result<Self, ScopeRecordError> {
        ToolExecutionPolicy::resolve(tool_access.clone())
            .map_err(|_| ScopeRecordError::UnresolvedToolPolicy)?;
        Ok(Self {
            tool_access,
            allowed_mutations,
        })
    }

    pub fn tool_access(&self) -> &ToolAccessPolicy {
        &self.tool_access
    }
    pub fn allowed_mutations(&self) -> &BTreeSet<ToolMutationClass> {
        &self.allowed_mutations
    }

    /// Construct candidate intersection content. Only the owning generated
    /// claim seam can turn it into permission to start a physical effect.
    pub fn intersect(&self, child: &Self) -> Result<Self, ScopeRecordError> {
        let tool_access = self
            .tool_access
            .clone()
            .conjoin(child.tool_access.clone())
            .map_err(|_| ScopeRecordError::UnresolvedToolPolicy)?;
        Self::new(
            tool_access,
            self.allowed_mutations
                .intersection(&child.allowed_mutations)
                .copied()
                .collect(),
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopedRunPolicyWire {
    tool_access: StrictScopeToolPolicy,
    allowed_mutations: BTreeSet<ToolMutationClass>,
}

#[derive(Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum StrictScopeToolPolicy {
    AllowList(ToolNameSet),
    DenyList(ToolNameSet),
    ReadOnly,
    Constraints(Vec<StrictScopeToolConstraint>),
}

#[derive(Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum StrictScopeToolConstraint {
    AllowNames(ToolNameSet),
    DenyNames(ToolNameSet),
    ReadOnly,
}

impl From<StrictScopeToolConstraint> for ToolAccessConstraint {
    fn from(value: StrictScopeToolConstraint) -> Self {
        match value {
            StrictScopeToolConstraint::AllowNames(names) => Self::AllowNames(names),
            StrictScopeToolConstraint::DenyNames(names) => Self::DenyNames(names),
            StrictScopeToolConstraint::ReadOnly => Self::ReadOnly,
        }
    }
}

impl TryFrom<ScopedRunPolicyWire> for ScopedRunPolicyRecord {
    type Error = ScopeRecordError;
    fn try_from(value: ScopedRunPolicyWire) -> Result<Self, Self::Error> {
        let policy = match value.tool_access {
            StrictScopeToolPolicy::AllowList(names) => ToolAccessPolicy::AllowList(names),
            StrictScopeToolPolicy::DenyList(names) => ToolAccessPolicy::DenyList(names),
            StrictScopeToolPolicy::ReadOnly => ToolAccessPolicy::ReadOnly,
            StrictScopeToolPolicy::Constraints(constraints) => {
                ToolAccessPolicy::Constraints(constraints.into_iter().map(Into::into).collect())
            }
        };
        Self::new(policy, value.allowed_mutations)
    }
}

/// Exact callback content selected by the generated continuation admission.
/// Like the enclosing scope record, these bytes cannot issue execution authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedCallbackContinuationRecord {
    pub target: crate::session::CallbackBatchIdentity,
    pub results_digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunEffectScopeRecord {
    pub format: RunEffectScopeFormat,
    pub request_id: OperationId,
    pub grant: ExecutionGrantRef,
    pub executor: ScopedExecutorBinding,
    pub input_id: InputId,
    pub run_id: RunId,
    pub admission_commit: ExecutionAdmissionCommitRef,
    pub parent_scope: Option<RunEffectScopeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_continuation: Option<ScopedCallbackContinuationRecord>,
    pub policy: ScopedRunPolicyRecord,
    pub remaining: ScopedEffectBudget,
}

impl RunEffectScopeRecord {
    pub fn validate_callback_continuation(
        &self,
        scope_id: RunEffectScopeId,
    ) -> Result<(), &'static str> {
        if let Some(continuation) = &self.callback_continuation
            && (continuation.target.session_id() != &self.executor.session_id
                || continuation.target.run_id() == &self.run_id
                || continuation.target.execution_scope().is_none()
                || continuation.target.execution_scope() == Some(scope_id))
        {
            return Err("callback continuation must bind an earlier scoped run of this session");
        }
        Ok(())
    }
}

/// In-memory run scope cannot be restored by deserializing record content.
///
/// ```compile_fail
/// use meerkat_core::execution_scope::ScopedRunAuthority;
/// let forged = serde_json::from_str::<ScopedRunAuthority>("{}");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedRunAuthority {
    scope_id: RunEffectScopeId,
    record: Arc<RunEffectScopeRecord>,
}

impl ScopedRunAuthority {
    pub const fn scope_id(&self) -> RunEffectScopeId {
        self.scope_id
    }

    pub fn record(&self) -> &RunEffectScopeRecord {
        &self.record
    }
    pub const fn native_tools(&self) -> ProviderNativeToolPolicy {
        ProviderNativeToolPolicy::DisableAll
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "RunExecutionAuthorityRecord",
    into = "RunExecutionAuthorityRecord"
)]
pub enum RunExecutionAuthority {
    #[default]
    SessionPolicy,
    Scoped(ScopedRunAuthority),
}

impl RunExecutionAuthority {
    pub const fn is_session_policy(&self) -> bool {
        matches!(self, Self::SessionPolicy)
    }
}

impl From<RunExecutionAuthority> for RunExecutionAuthorityRecord {
    fn from(value: RunExecutionAuthority) -> Self {
        match value {
            RunExecutionAuthority::SessionPolicy => Self::SessionPolicy {},
            RunExecutionAuthority::Scoped(scope) => Self::Scoped {
                scope_id: scope.scope_id,
                record: Box::new((*scope.record).clone()),
            },
        }
    }
}

impl TryFrom<RunExecutionAuthorityRecord> for RunExecutionAuthority {
    type Error = ScopeRecordError;

    fn try_from(value: RunExecutionAuthorityRecord) -> Result<Self, Self::Error> {
        match value {
            RunExecutionAuthorityRecord::SessionPolicy {} => Ok(Self::SessionPolicy),
            RunExecutionAuthorityRecord::Scoped { .. } => {
                Err(ScopeRecordError::RestorationRequired)
            }
        }
    }
}

#[cfg(all(
    meerkat_internal_generated_authority_bridge,
    not(target_arch = "wasm32"),
    not(test),
))]
#[allow(improper_ctypes_definitions, unsafe_code)]
unsafe extern "Rust" {
    #[link_name = concat!(
        "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_live_request_scope_",
        env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn runtime_live_request_scope_bridge_token_is_valid(
        token: &(dyn std::any::Any + Send + Sync),
    ) -> bool;
}

#[cfg(all(
    meerkat_internal_generated_authority_bridge,
    not(target_arch = "wasm32"),
    not(test),
))]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_core_runtime_generated_live_request_scope_build_v1_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) extern "Rust" fn runtime_generated_live_request_scope_build(
    token: &'static (dyn std::any::Any + Send + Sync),
    scope_id: RunEffectScopeId,
    record: RunEffectScopeRecord,
    scope_revision: NonZeroU64,
) -> Result<ScopedRunAuthority, String> {
    let valid = unsafe { runtime_live_request_scope_bridge_token_is_valid(token) };
    if !valid {
        return Err("scoped run requires the generated LiveRequest owner bridge".into());
    }
    if scope_revision <= record.admission_commit.revision {
        return Err("run scope commit must follow its exact admission".into());
    }
    record.validate_callback_continuation(scope_id)?;
    Ok(ScopedRunAuthority {
        scope_id,
        record: Arc::new(record),
    })
}

#[cfg(all(
    meerkat_internal_generated_authority_bridge,
    not(target_arch = "wasm32"),
    not(test),
))]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_core_runtime_generated_live_request_effect_build_v1_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) extern "Rust" fn runtime_generated_live_request_effect_build(
    token: &'static (dyn std::any::Any + Send + Sync),
    scope: &ScopedRunAuthority,
    claim: ScopedEffectClaimRecord<ScopedEffectTarget>,
) -> Result<ScopedEffectStartPermit<ScopedEffectTarget>, String> {
    let valid = unsafe { runtime_live_request_scope_bridge_token_is_valid(token) };
    if !valid {
        return Err("effect start requires the generated LiveRequest owner bridge".into());
    }
    claim
        .validate_scope_binding(scope.scope_id(), scope.record(), &claim.target)
        .map_err(|error| error.to_string())?;
    Ok(ScopedEffectStartPermit { claim })
}

/// One callback-application permission from a newly committed generated claim.
/// This is not an ordinary Applied receipt and cannot be restored from content.
///
/// ```compile_fail
/// use meerkat_core::execution_scope::ScopedCallbackApplicationPermit;
/// fn duplicate(permit: ScopedCallbackApplicationPermit) { let _ = permit.clone(); }
/// ```
///
/// ```compile_fail
/// use meerkat_core::execution_scope::ScopedCallbackApplicationPermit;
/// let forged = serde_json::from_str::<ScopedCallbackApplicationPermit>("{}");
/// ```
#[derive(Debug)]
pub struct ScopedCallbackApplicationPermit {
    scope: ScopedRunAuthority,
}

impl ScopedCallbackApplicationPermit {
    pub fn scope(&self) -> &ScopedRunAuthority {
        &self.scope
    }
}

#[cfg(all(
    meerkat_internal_generated_authority_bridge,
    not(target_arch = "wasm32"),
    not(test),
))]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_core_runtime_generated_live_callback_application_build_v1_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) extern "Rust" fn runtime_generated_live_callback_application_build(
    token: &'static (dyn std::any::Any + Send + Sync),
    scope: ScopedRunAuthority,
    claim_revision: NonZeroU64,
) -> Result<ScopedCallbackApplicationPermit, String> {
    if !unsafe { runtime_live_request_scope_bridge_token_is_valid(token) }
        || claim_revision <= scope.record().admission_commit.revision
        || scope.record().callback_continuation.is_none()
    {
        return Err("callback application requires a newly committed generated claim".into());
    }
    scope
        .record()
        .validate_callback_continuation(scope.scope_id())?;
    Ok(ScopedCallbackApplicationPermit { scope })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunExecutionAuthorityRecord {
    SessionPolicy {},
    Scoped {
        scope_id: RunEffectScopeId,
        record: Box<RunEffectScopeRecord>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedEffectClaimRecord<Target> {
    pub claim_id: ScopedEffectClaimId,
    pub scope_id: RunEffectScopeId,
    pub request_id: OperationId,
    pub effect_id: OperationId,
    pub target: Target,
    pub executor: ScopedExecutorBinding,
    pub input_id: InputId,
    pub run_id: RunId,
    pub grant: ExecutionGrantRef,
    pub candidate_policy_revision: ScopedEffectPolicyRevision,
    pub commit: ExecutionAdmissionCommitRef,
}

/// Exact policy content observed for a claim, not a policy owner or permit.
/// An immutable unmanaged policy has no fabricated generation or counter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopedEffectPolicyRevision {
    TrustedHost {
        ordinary_policy: crate::PolicyDigest,
        revision: NonZeroU64,
    },
    Immutable {
        ordinary_policy: crate::PolicyDigest,
    },
    Managed {
        ordinary_policy: crate::PolicyDigest,
        provider_id: crate::PolicyProviderId,
        policy_id: crate::PolicyId,
        generation: crate::PolicyProviderGeneration,
        revision: NonZeroU64,
        digest: crate::PolicyDigest,
    },
}

impl ScopedEffectPolicyRevision {
    pub fn validate(&self) -> Result<(), ScopeRecordError> {
        let ordinary = match self {
            Self::TrustedHost {
                ordinary_policy, ..
            }
            | Self::Immutable { ordinary_policy } => ordinary_policy,
            Self::Managed {
                ordinary_policy,
                provider_id,
                policy_id,
                digest,
                ..
            } => {
                crate::PolicyProviderId::new(provider_id.as_str())
                    .map_err(|_| ScopeRecordError::InvalidPolicyIdentity)?;
                crate::PolicyId::new(policy_id.as_str())
                    .map_err(|_| ScopeRecordError::InvalidPolicyIdentity)?;
                crate::PolicyDigest::parse(digest.as_str())
                    .map_err(|_| ScopeRecordError::InvalidPolicyIdentity)?;
                ordinary_policy
            }
        };
        crate::PolicyDigest::parse(ordinary.as_str())
            .map_err(|_| ScopeRecordError::InvalidPolicyIdentity)?;
        Ok(())
    }
}

impl<Target: PartialEq> ScopedEffectClaimRecord<Target> {
    /// Validate the persisted join, not current permission. Restoration must
    /// additionally consult generated authority for revocation, cancellation,
    /// expiry, current executor fences, and whether this claim was spent.
    pub fn validate_scope_binding(
        &self,
        scope_id: RunEffectScopeId,
        scope: &RunEffectScopeRecord,
        target: &Target,
    ) -> Result<(), ScopeRecordError> {
        self.candidate_policy_revision.validate()?;
        if self.scope_id != scope_id
            || self.request_id != scope.request_id
            || self.executor != scope.executor
            || self.input_id != scope.input_id
            || self.run_id != scope.run_id
            || self.grant != scope.grant
        {
            return Err(ScopeRecordError::ClaimScopeMismatch);
        }
        if &self.target != target {
            return Err(ScopeRecordError::ClaimTargetMismatch);
        }
        if self.commit.revision <= scope.admission_commit.revision {
            return Err(ScopeRecordError::ClaimBeforeAdmission);
        }
        Ok(())
    }
}

/// One-use effect-start handoff. There is no Clone or Deserialize route from
/// stored content to a new physical invocation.
///
/// ```compile_fail
/// use meerkat_core::execution_scope::ScopedEffectStartPermit;
/// fn duplicate(permit: ScopedEffectStartPermit<meerkat_core::ToolName>) {
///     let _ = permit.clone();
/// }
/// ```
///
/// ```compile_fail
/// use meerkat_core::execution_scope::ScopedEffectStartPermit;
/// let forged = serde_json::from_str::<ScopedEffectStartPermit<meerkat_core::ToolName>>("{}");
/// ```
#[derive(Debug)]
pub struct ScopedEffectStartPermit<Target> {
    claim: ScopedEffectClaimRecord<Target>,
}

impl<Target> ScopedEffectStartPermit<Target> {
    pub fn claim(&self) -> &ScopedEffectClaimRecord<Target> {
        &self.claim
    }
    pub fn into_claim(self) -> ScopedEffectClaimRecord<Target> {
        self.claim
    }

    /// Relinquish start permission without invoking the physical adapter.
    pub fn into_not_started(self) -> ScopedEffectNotStartedProof<Target> {
        ScopedEffectNotStartedProof { claim: self.claim }
    }
}

/// Local custody of an unconsumed start permit, not recoverable from content.
///
/// ```compile_fail
/// use meerkat_core::execution_scope::{ScopedEffectNotStartedProof, ScopedEffectTarget};
/// fn duplicate(proof: ScopedEffectNotStartedProof<ScopedEffectTarget>) {
///     let another = proof.clone();
/// }
/// ```
///
/// ```compile_fail
/// use meerkat_core::execution_scope::{ScopedEffectNotStartedProof, ScopedEffectTarget};
/// let forged = serde_json::from_str::<ScopedEffectNotStartedProof<ScopedEffectTarget>>("{}");
/// ```
#[derive(Debug)]
pub struct ScopedEffectNotStartedProof<Target> {
    claim: ScopedEffectClaimRecord<Target>,
}

impl<Target> ScopedEffectNotStartedProof<Target> {
    pub fn claim(&self) -> &ScopedEffectClaimRecord<Target> {
        &self.claim
    }

    pub fn into_claim(self) -> ScopedEffectClaimRecord<Target> {
        self.claim
    }
}

/// The feature owning the child/job supplies its own canonical target type.
/// This persisted proposal is not an inheritable execution permit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescendantExecutionScopeRecord<Target> {
    pub parent_scope: RunEffectScopeId,
    pub parent_request_id: OperationId,
    pub parent_input_id: InputId,
    pub parent_run_id: RunId,
    pub parent_executor: ScopedExecutorBinding,
    pub grant: ExecutionGrantRef,
    pub target: Target,
    pub intersected_policy: ScopedRunPolicyRecord,
}

impl<Target> DescendantExecutionScopeRecord<Target> {
    pub fn validate_parent(
        &self,
        parent_scope: RunEffectScopeId,
        parent: &RunEffectScopeRecord,
    ) -> Result<(), ScopeRecordError> {
        if self.parent_scope != parent_scope
            || self.parent_request_id != parent.request_id
            || self.parent_input_id != parent.input_id
            || self.parent_run_id != parent.run_id
            || self.parent_executor != parent.executor
            || self.grant != parent.grant
        {
            return Err(ScopeRecordError::DescendantParentMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScopeRecordError {
    #[error("scoped execution requires current generated restoration, not record deserialization")]
    RestorationRequired,
    #[error("execution scope requires a concrete resolved tool policy")]
    UnresolvedToolPolicy,
    #[error("effect claim has an invalid policy content identity")]
    InvalidPolicyIdentity,
    #[error("effect claim does not bind the exact persisted request, run, executor, and grant")]
    ClaimScopeMismatch,
    #[error("effect claim names a different invocation target")]
    ClaimTargetMismatch,
    #[error("effect claim must be committed after request admission")]
    ClaimBeforeAdmission,
    #[error("descendant scope does not bind the exact parent lineage")]
    DescendantParentMismatch,
}

#[cfg(test)]
mod callback_primitive_tests {
    use super::*;
    use crate::lifecycle::run_primitive::{
        ConversationAppend, ConversationAppendRole, CoreRenderable, RunApplyBoundary, RunPrimitive,
        RuntimeExecutionKind, RuntimeTurnMetadata, StagedRunInput,
    };

    #[test]
    fn callback_primitive_requires_its_exact_exclusive_empty_run_start()
    -> Result<(), Box<dyn std::error::Error>> {
        let scope_id = RunEffectScopeId::from_uuid(uuid::Uuid::from_u128(7));
        let record: RunEffectScopeRecord = serde_json::from_value(serde_json::json!({
            "format": "v1",
            "request_id": uuid::Uuid::from_u128(1),
            "grant": {"id": uuid::Uuid::from_u128(2), "issuer_realm": "owner", "generation": 1},
            "executor": {
                "session_id": uuid::Uuid::from_u128(3),
                "realm": "owner",
                "runtime_epoch": uuid::Uuid::from_u128(4),
                "binding_generation": 0,
            },
            "input_id": uuid::Uuid::from_u128(5),
            "run_id": uuid::Uuid::from_u128(6),
            "admission_commit": {"revision": 1, "digest": vec![1; 32]},
            "parent_scope": null,
            "callback_continuation": {
                "target": {
                    "session_id": uuid::Uuid::from_u128(3),
                    "run_id": uuid::Uuid::from_u128(8),
                    "execution_scope": uuid::Uuid::from_u128(9),
                    "execution_boundary": uuid::Uuid::from_u128(10),
                    "batch_digest": vec![255; 32],
                },
                "results_digest": vec![254; 32],
            },
            "policy": {"tool_access": {"type":"read_only"}, "allowed_mutations":["read_only"]},
            "remaining": {"model_computations":1, "tool_dispatches":1, "descendant_admissions":0},
        }))?;
        let scope = ScopedRunAuthority {
            scope_id,
            record: Arc::new(record.clone()),
        };
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            execution_authority: RunExecutionAuthority::Scoped(scope),
            boundary: RunApplyBoundary::RunStart,
            appends: Vec::new(),
            contributing_input_ids: vec![record.input_id.clone()],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ResumePending),
                ..Default::default()
            }),
        });
        primitive.validate_execution_authority(&record.run_id)?;
        assert_eq!(
            primitive.callback_continuation(),
            record.callback_continuation.as_ref()
        );
        assert!(serde_json::from_value::<RunPrimitive>(serde_json::to_value(&primitive)?).is_err());
        assert!(
            primitive
                .validate_execution_authority(&RunId::new())
                .is_err()
        );
        for corruption in 0..7 {
            let mut wrong = primitive.clone();
            let RunPrimitive::StagedInput(staged) = &mut wrong else {
                return Err("staged primitive".into());
            };
            match corruption {
                0 => staged.appends.push(ConversationAppend {
                    role: ConversationAppendRole::User,
                    content: CoreRenderable::text("not another request"),
                    identity: None,
                }),
                1 => staged.boundary = RunApplyBoundary::RunCheckpoint,
                2 => staged.contributing_input_ids.push(InputId::new()),
                3 => staged.turn_metadata = None,
                4 => staged.turn_metadata = Some(RuntimeTurnMetadata::default()),
                5 => {
                    staged.turn_metadata = Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        ..Default::default()
                    });
                }
                _ => {
                    let RunExecutionAuthority::Scoped(scope) = &mut staged.execution_authority
                    else {
                        return Err("scoped primitive".into());
                    };
                    let record = Arc::make_mut(&mut scope.record);
                    record.run_id = record
                        .callback_continuation
                        .as_ref()
                        .ok_or("continuation")?
                        .target
                        .run_id()
                        .clone();
                }
            }
            assert!(wrong.validate_execution_authority(&record.run_id).is_err());
        }
        Ok(())
    }
}
