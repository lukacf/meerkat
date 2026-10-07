//! Protected historical observations in the existing native input row.
//!
//! These records never authorize, settle, or replay an operation. No public
//! event exporter is their custody owner. A staged record becomes durable only
//! when the existing native row transaction commits it.

use std::sync::Arc;

use meerkat_core::authorization::{
    OperationObservationError, OperationObservedOutcome, OperationRefusalKind,
};
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::{InputId, OperationId, PrincipalRef, RunId, SessionId};
use serde::{Deserialize, Serialize};

use crate::evidence::EvidenceDigest;
use crate::resource::ResourceRef;
use crate::work_association::{GrantLineageRef, WorkAuthorityBasis};

/// Actual nonsecret model destination. No prompt, credential secret, response,
/// or inferred account identity is captured here.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditModelTarget {
    pub model: String,
    pub provider: meerkat_core::Provider,
    pub self_hosted_server_id: Option<String>,
    pub auth_binding: Option<meerkat_core::AuthBindingRef>,
    pub credential: Option<meerkat_core::connection::AuthCredentialIdentity>,
    pub wire_model: String,
    pub backend_profile_id: Option<String>,
    pub backend_kind: String,
    pub endpoint: String,
    pub hosted_capabilities: Vec<meerkat_core::ServerToolKind>,
    pub usage: AuditModelUse,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditModelUse {
    Inference,
    ControllerInference,
    Compaction,
    Live,
}

/// Historical names of the exact plan's declared owners. Process pointers and
/// live binding fingerprints are deliberately not serialized as authority.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditToolOwner {
    pub authority_key: String,
    pub owner_key: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuditTarget {
    Model(AuditModelTarget),
    Tool {
        call_id: String,
        tool_name: String,
        /// Protected correlation digest, never exposed in safe projections.
        arguments_digest: EvidenceDigest,
        owners: Vec<AuditToolOwner>,
    },
    ProviderHostedTool {
        call_id: String,
        tool_name: String,
        target: AuditModelTarget,
    },
    Blob {
        reference: meerkat_core::blob::BlobRef,
        usage: AuditSourceUse,
    },
    Memory {
        scope: meerkat_core::memory::MemorySearchScope,
        usage: AuditSourceUse,
    },
    Transcript {
        session_id: SessionId,
        range: meerkat_core::memory::MessageRange,
        usage: AuditSourceUse,
    },
    ExternalSource {
        resource: ResourceRef,
        usage: AuditSourceUse,
    },
    Publication {
        recipient: AuditRecipient,
        mode: AuditPublicationMode,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuditRecipient {
    Principal { principal: PrincipalRef },
    Destination { resource: ResourceRef },
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSourceUse {
    Read,
    Hydrate,
    Retain,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditPublicationMode {
    Buffered,
    Stream,
    Replay,
    Live,
}

/// Observation of the same actual local owner publication used for policy
/// compilation. Its sequence is process-local diagnostic data, not a durable
/// policy epoch, separate grant registry, or proof of external freshness.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditPolicyRead {
    pub publication_sequence: u64,
    pub observed_at_ms: u64,
    /// Complete paths/bases resolved by the real grant-backed policy for this
    /// successful preparation. Refusals never claim every conjunct resolved.
    pub operation_authorities: Vec<WorkAuthorityBasis>,
    pub controller_lineages: Vec<Vec<GrantLineageRef>>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuditObservation {
    Prepared {
        target: Arc<AuditTarget>,
        policy: AuditPolicyRead,
    },
    Refused {
        target: Arc<AuditTarget>,
        reason: OperationRefusalKind,
    },
    AuthorizationUnavailable {
        target: Arc<AuditTarget>,
    },
    /// A synchronous boundary attempt, not proof the physical body ran.
    Entry,
    Outcome {
        outcome: OperationObservedOutcome,
    },
}

/// Protected data submitted to the native row-bound sink. Construction and
/// deserialization confer no permission, acceptance, or durable receipt.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationAuditObservation {
    pub operation_id: OperationId,
    pub execution_scope: OperationExecutionScope,
    pub run_id: Option<RunId>,
    /// Scalar historical observation of the owner revision. Decoding this
    /// string cannot reconstruct `CanonicalContextRevision` authority.
    pub context_revision: Option<String>,
    pub observation: AuditObservation,
}

/// Parties projected by the actual native owner from its retained row. The
/// audit producer cannot replace these with model or wire-supplied principals.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeAuditContributor {
    pub input_id: InputId,
    pub requester: PrincipalRef,
    pub logical_executor: PrincipalRef,
    pub represented_subject: Option<PrincipalRef>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredAuthorizationAuditObservation {
    pub contributors: Arc<[NativeAuditContributor]>,
    pub observation: AuthorizationAuditObservation,
}

/// A host-installed sink bound to the existing native input row. This trait
/// only stages payload for that row's next existing commit; it never adds a
/// transaction or grants permission. Production contexts require the real
/// native implementation, not a logging-only or lossy event exporter.
pub trait AuthorizationAuditSink: Send + Sync {
    fn append(
        &self,
        observation: AuthorizationAuditObservation,
    ) -> Result<(), OperationObservationError>;
}

macro_rules! redacted_debug {
    ($($ty:ty),+ $(,)?) => {$ (
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($ty), "([REDACTED])"))
            }
        }
    )+ };
}
redacted_debug!(
    AuditModelTarget,
    AuditTarget,
    AuditRecipient,
    AuditToolOwner,
    AuditPolicyRead,
    AuditObservation,
    AuthorizationAuditObservation,
    NativeAuditContributor,
    StoredAuthorizationAuditObservation
);

/// Public projections contain no identity, target, argument digest, policy,
/// grant, or source data. Detailed reads remain at the protected native owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeAuditObservation {
    AuthorizationUnavailable,
    Prepared,
    Refused,
    EntryAttempt,
    Returned,
}

impl AuthorizationAuditObservation {
    #[must_use]
    pub fn safe_projection(&self) -> SafeAuditObservation {
        match self.observation {
            AuditObservation::Prepared { .. } => SafeAuditObservation::Prepared,
            AuditObservation::AuthorizationUnavailable { .. } => {
                SafeAuditObservation::AuthorizationUnavailable
            }
            AuditObservation::Refused { .. } => SafeAuditObservation::Refused,
            AuditObservation::Entry => SafeAuditObservation::EntryAttempt,
            AuditObservation::Outcome { .. } => SafeAuditObservation::Returned,
        }
    }
}
