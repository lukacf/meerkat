//! Exact original-task continuation evidence, supplied by the delivery owner.
//!
//! Toolkit asks the original owner to admit or reconcile one immutable result.
//! It never picks a new task or infers business-task completion.

use crate::binding::RequirementOutcome;
use crate::request::{ContextReference, TaskContinuation};
use async_trait::async_trait;
use meerkat_authorization_contracts::{
    evidence::HistoricalEvidenceRef, work_association::OriginalWorkRef,
};
use meerkat_core::auth::PrincipalRef;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};
use toolkit_package::{ContentDigest, Id, Revision, canonical_json};

/// Immutable terminal observation. Ready is historical evidence; future calls
/// still require current grants and bindings from their respective owners.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SetupTerminalOutcome {
    /// The newly approved B instruction was admitted in A's actual session.
    /// This does not widen A or assert that B finished the business task.
    ApprovedServiceAdmitted {
        selection_digest: ContentDigest,
        selection_generation: u64,
        observed_at_ms: u64,
        admission: crate::setup::ApprovedServiceInput,
    },
    Ready {
        selection_digest: ContentDigest,
        selection_generation: u64,
        observed_at_ms: u64,
        roles: BTreeMap<Id, RequirementOutcome>,
    },
    Unavailable {
        detail: ContextReference,
    },
    Failed {
        detail: ContextReference,
    },
    Cancelled {
        detail: ContextReference,
    },
    Expired {
        deadline_ms: u64,
    },
}

/// Constructed only from a recovered terminal acquisition. Host implementations
/// inspect it through getters; it cannot be deserialized from model input.
#[derive(Clone, Serialize)]
pub struct ContinuationObligation {
    pub(crate) tenant: Id,
    pub(crate) deployment: Id,
    pub(crate) operation: Id,
    pub(crate) original_work: OriginalWorkRef,
    pub(crate) requester: PrincipalRef,
    pub(crate) origin: HistoricalEvidenceRef,
    pub(crate) admitted_policy_revision: Revision,
    pub(crate) continuation: TaskContinuation,
    pub(crate) continuation_digest: ContentDigest,
    pub(crate) terminal_revision: u64,
    pub(crate) terminal_at_ms: u64,
    pub(crate) outcome: SetupTerminalOutcome,
    /// Present for every terminal outcome once B wins the pre-entry selection.
    /// The delivery owner observes B, or authenticates permanent B non-entry
    /// before delivering the exact recorded attachment failure under A.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) approved_service: Option<crate::setup::ApprovedServiceSelection>,
    /// Terminal native cause; a source-owner non-entry proof is separately
    /// required before this can be delivered to the original task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) service_attachment_failure: Option<crate::setup::ApprovedServiceAttachmentFailure>,
    /// Optional typed observation from the same terminal record. No server text
    /// or independent authority is conveyed by these bounded fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) service_attachment_observation:
        Option<crate::setup::ApprovedServiceAttachmentObservation>,
    /// Bounded helper explanation from the same canonical terminal record.
    /// None preserves the canonical bytes of pre-existing generic obligations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) helper_detail: Option<String>,
}

impl fmt::Debug for ContinuationObligation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContinuationObligation")
            .field("operation", &self.operation)
            .field("terminal_revision", &self.terminal_revision)
            .finish_non_exhaustive()
    }
}

impl ContinuationObligation {
    pub fn tenant(&self) -> &Id {
        &self.tenant
    }
    pub fn deployment(&self) -> &Id {
        &self.deployment
    }
    pub fn operation(&self) -> &Id {
        &self.operation
    }
    pub fn original_work(&self) -> &OriginalWorkRef {
        &self.original_work
    }
    pub fn requester(&self) -> &PrincipalRef {
        &self.requester
    }
    pub fn origin(&self) -> &HistoricalEvidenceRef {
        &self.origin
    }
    pub fn admitted_policy_revision(&self) -> &Revision {
        &self.admitted_policy_revision
    }
    pub fn continuation(&self) -> &TaskContinuation {
        &self.continuation
    }
    pub fn continuation_digest(&self) -> &ContentDigest {
        &self.continuation_digest
    }
    pub fn terminal_revision(&self) -> u64 {
        self.terminal_revision
    }
    pub fn terminal_at_ms(&self) -> u64 {
        self.terminal_at_ms
    }
    pub fn outcome(&self) -> &SetupTerminalOutcome {
        &self.outcome
    }
    pub fn approved_service(&self) -> Option<&crate::setup::ApprovedServiceSelection> {
        self.approved_service.as_ref()
    }

    pub fn service_attachment_failure(&self) -> Option<crate::setup::ApprovedServiceAttachmentFailure> {
        self.service_attachment_failure
    }

    pub fn service_attachment_observation(
        &self,
    ) -> Option<crate::setup::ApprovedServiceAttachmentObservation> {
        self.service_attachment_observation
    }

    /// Human-facing report text, not tool logs or authentication payloads.
    /// This is model-authored content; bounds do not guarantee absence of secrets.
    pub fn helper_detail(&self) -> Option<&str> {
        self.helper_detail.as_deref()
    }

    pub fn digest(&self) -> Result<ContentDigest, ContinuationError> {
        Ok(ContentDigest::of_bytes(&canonical_json(self)?))
    }

    /// Stable within the original task owner, regardless of retry or worker.
    /// Different content must conflict under this key, never mint another key.
    pub fn delivery_identity(&self) -> Result<ContentDigest, ContinuationError> {
        Ok(ContentDigest::of_bytes(&canonical_json(&(
            "toolkit-continuation-v2",
            &self.tenant,
            &self.deployment,
            &self.operation,
            &self.continuation.owner,
            &self.continuation.delivery_key,
            self.terminal_revision,
        ))?))
    }
}

/// Data returned by the trusted owner port. Serialization is storage/wire
/// transport, never authentication. The owner must authenticate the referenced
/// durable admission; Toolkit then checks its exact obligation binding.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationAdmissionEvidence {
    pub receipt: ContextReference,
    pub obligation_digest: ContentDigest,
    pub delivery_identity: ContentDigest,
    pub runtime: Id,
    pub session: Id,
    pub durable_input: ContextReference,
    /// Selected store clock after authoritative native input admission was
    /// observed. This is not native admission time or an authorization deadline.
    /// The old `admitted_at_ms` field is deliberately not a serde alias: old
    /// records with that field require owner reconciliation, not reinterpretation.
    pub observed_at_ms: u64,
}

impl ContinuationAdmissionEvidence {
    /// A later observation can advance its own clock without changing the
    /// exact native admission. The first observation remains in canonical
    /// storage; no receipt, owner, input or obligation field is ignored here.
    pub(crate) fn confirms(&self, earlier: &Self) -> bool {
        self.receipt == earlier.receipt
            && self.obligation_digest == earlier.obligation_digest
            && self.delivery_identity == earlier.delivery_identity
            && self.runtime == earlier.runtime
            && self.session == earlier.session
            && self.durable_input == earlier.durable_input
            && self.observed_at_ms >= earlier.observed_at_ms
    }

    pub(crate) fn check(
        &self,
        obligation: &ContinuationObligation,
        receipt: &ContextReference,
    ) -> Result<(), ContinuationError> {
        if &self.receipt != receipt
            || self.receipt.owner != obligation.continuation.owner
            || self.obligation_digest != obligation.digest()?
            || self.delivery_identity != obligation.delivery_identity()?
            || self.observed_at_ms < obligation.terminal_at_ms
        {
            return Err(ContinuationError::InvalidReceipt);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContinuationError {
    #[error("continuation owner denied the exact task, audience or receipt")]
    Denied,
    #[error("original continuation owner is unavailable")]
    OwnerUnavailable,
    #[error("runtime lacks the required durable exact admission contract")]
    Unsupported,
    #[error("continuation owner operation exceeded its bounded wait")]
    Timeout,
    #[error("receipt does not bind the exact terminal obligation")]
    InvalidReceipt,
    #[error("continuation encoding failed")]
    Encoding(#[from] serde_json::Error),
}

/// Trusted host composition binds this port to the original task/delivery
/// authority. A model cannot select an implementation. Each call authenticates
/// the receipt and checks current access to this observation, task custody and
/// the exact selected audience within its original ceiling. Receipt observation
/// is not permission to repeat delivery, install code or read research context.
/// `None` means no authenticated admission is available yet.
#[async_trait]
pub trait SetupContinuationOwner: Send + Sync {
    async fn verify_admission(
        &self,
        obligation: &ContinuationObligation,
        receipt: &ContextReference,
    ) -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError>;
}

/// Trusted owner observations, not assignable acquisition states. Negative
/// evidence references are authenticated by the injected owner and bound to
/// this obligation. In particular, an empty receipt lookup is not SafeToRetry.
pub enum ContinuationReconciliation {
    Applied(ContinuationAdmissionEvidence),
    /// The owner proves non-application with all old attempts fenced/drained,
    /// or atomic exact-key admission deduplicates late and new calls. Draining
    /// alone does not establish non-application. The owner durably retains this
    /// fact for the complete retry lifetime; this is technical replay safety,
    /// not current dispatch authority or a transient previous-worker permit.
    SafeToRetry {
        evidence: ContextReference,
    },
    Pending {
        evidence: ContextReference,
    },
    /// No original-task input was admitted and no outstanding old call can
    /// admit one later. An inbox submission may have committed and then settled
    /// as refused/stranded. Current access denial alone proves neither fact.
    DefinitivelyRejected {
        evidence: ContextReference,
    },
}

/// Host-bound continuation admission, using existing runtime/delivery owners.
/// Every method must check current authority for its own operation. Preflight
/// is not a cached permit: admit checks exact task custody and selected audience
/// at the admission boundary, under the original ceiling. The delivery identity
/// binds the complete obligation exactly, including after input retirement.
/// A Toolkit lease does not fence remote calls or authorize a message.
#[async_trait]
pub trait SetupContinuationDelivery: SetupContinuationOwner {
    async fn authorize_delivery(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<(), ContinuationError>;

    async fn admit(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationAdmissionEvidence, ContinuationError>;

    /// Checks current observation/reconciliation authority without reviving
    /// expired setup permission. Unavailability, unsupported lookup and denial
    /// are errors, never negative proof about an earlier attempt.
    async fn reconcile(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationReconciliation, ContinuationError>;
}

/// Host executor settings, never model-supplied tool arguments. Each owner
/// call is bounded independently. The lease must cover two sequential owner
/// calls plus the host's positive settlement budget. The host schedules future
/// steps; unexpected storage/scheduling delay still fails closed at the fence.
pub struct ContinuationStepOptions {
    pub worker: Id,
    pub lease_ms: u64,
    pub owner_timeout_ms: u64,
    pub settlement_budget_ms: u64,
}

/// One mechanical outbox step. These observations introduce no delivery queue
/// or lifecycle. Dispatch-budget exhaustion does not stop reconciliation.
pub enum ContinuationStep {
    Acknowledged(crate::setup::SetupControlReceipt),
    Pending,
    /// Technical exact-replay safety was recorded. A subsequent dispatch still
    /// needs current owner authority and remaining attempts; this is no permit.
    RetryAuthorized,
    OwnerRejected,
    /// New dispatch is unavailable. Future observation steps must still recover
    /// any late owner commit from an earlier attempt.
    DispatchBudgetExhausted,
}
