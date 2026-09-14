//! Neutral persisted execution-scope content and non-deserializable handles.
//!
//! Records are not permits. Generated runtime authority must validate their
//! committed origin and current fences before installing a live handle.

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
    pub binding_generation: NonZeroU64,
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
    pub policy: ScopedRunPolicyRecord,
    pub remaining: ScopedEffectBudget,
}

/// In-memory run scope cannot be restored by deserializing record content.
///
/// ```compile_fail
/// use meerkat_core::execution_scope::ScopedRunAuthority;
/// let forged = serde_json::from_str::<ScopedRunAuthority>("{}");
/// ```
#[derive(Debug, Clone)]
pub struct ScopedRunAuthority {
    record: Arc<RunEffectScopeRecord>,
}

impl ScopedRunAuthority {
    pub fn record(&self) -> &RunEffectScopeRecord {
        &self.record
    }
    pub const fn native_tools(&self) -> ProviderNativeToolPolicy {
        ProviderNativeToolPolicy::DisableAll
    }
}

#[derive(Debug, Clone)]
pub enum RunExecutionAuthority {
    SessionPolicy,
    Scoped(ScopedRunAuthority),
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
pub struct ScopedEffectClaimRecord {
    pub claim_id: ScopedEffectClaimId,
    pub scope_id: RunEffectScopeId,
    pub effect_id: OperationId,
    pub session_id: SessionId,
    pub input_id: InputId,
    pub run_id: RunId,
    pub grant_generation: NonZeroU64,
    pub candidate_policy_revision: NonZeroU64,
    pub commit: ExecutionAdmissionCommitRef,
}

/// One-use effect-start handoff. There is no Clone or Deserialize route from
/// stored content to a new physical invocation.
///
/// ```compile_fail
/// use meerkat_core::execution_scope::ScopedEffectStartPermit;
/// fn duplicate(permit: ScopedEffectStartPermit) { let _ = permit.clone(); }
/// ```
///
/// ```compile_fail
/// use meerkat_core::execution_scope::ScopedEffectStartPermit;
/// let forged = serde_json::from_str::<ScopedEffectStartPermit>("{}");
/// ```
#[derive(Debug)]
pub struct ScopedEffectStartPermit {
    claim: ScopedEffectClaimRecord,
}

impl ScopedEffectStartPermit {
    pub fn claim(&self) -> &ScopedEffectClaimRecord {
        &self.claim
    }
    pub fn into_claim(self) -> ScopedEffectClaimRecord {
        self.claim
    }
}

/// The feature owning the child/job supplies its own canonical target type.
/// This persisted proposal is not an inheritable execution permit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescendantExecutionScopeRecord<Target> {
    pub parent_scope: RunEffectScopeId,
    pub grant: ExecutionGrantRef,
    pub target: Target,
    pub intersected_policy: ScopedRunPolicyRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScopeRecordError {
    #[error("execution scope requires a concrete resolved tool policy")]
    UnresolvedToolPolicy,
}
