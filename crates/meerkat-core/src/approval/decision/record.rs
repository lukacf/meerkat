//! The stored row of one durable decision, and what it is built from.

use serde::{Deserialize, Serialize};

use super::{
    ApproverId, DecisionContractError, DecisionEvidence, DecisionId, DecisionKind,
    DecisionOperationId, DecisionPrincipalKey, DecisionReference, DecisionRenderer,
    DecisionRendering, DecisionRequestBody, DecisionSubjectKey, DecisionTier, PresentationRecord,
    Sha256Digest,
};
use crate::approval::ApprovalConsentDeliveryState;

/// Most presentation records a row keeps: the latest complete one per
/// audience.
pub const DECISION_PRESENTATIONS_MAX: usize = 16;

/// Where a held decision stands.
///
/// Matched exhaustively: the phases are the decision machine's, and a new
/// one is a machine change every owner must handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionPhase {
    /// Held, and not yet decided, withdrawn or expired.
    Open,
    /// Approved with attested evidence; the held request not yet executed.
    Approved,
    /// Rejected with attested evidence. Terminal.
    Rejected,
    /// Observed at or after its deadline undecided, or approved and never
    /// executed. Terminal.
    Expired,
    /// Withdrawn by its requester or the host before a decision. Terminal.
    Withdrawn,
    /// The held request was executed. Terminal.
    Executed,
    /// The operation's owner refused the held request permanently. Terminal.
    ExecutionRefused,
}

impl DecisionPhase {
    /// The phase's stable token (`open`, `approved`, ...).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Withdrawn => "withdrawn",
            Self::Executed => "executed",
            Self::ExecutionRefused => "execution_refused",
        }
    }
}

/// Who decides an open decision now.
///
/// Matched exhaustively: the stages are the decision machine's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStage {
    /// An authenticated human acting for the approver id.
    Human,
    /// The named reviewer.
    Review,
}

/// Where the requester notice of a decision stands.
///
/// Matched exhaustively: the notice states are the decision machine's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionNotice {
    /// The requester is not routable: no notice is ever owed.
    NotRequired,
    /// The requester is routable and the decision has not ended.
    NotDue,
    /// The decision ended and its notice is owed.
    Owed,
    /// The notice was delivered.
    Delivered,
    /// The notice failed terminally.
    Failed,
}

impl DecisionNotice {
    /// The notice state's stable token (`not_required`, `owed`, ...).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::NotDue => "not_due",
            Self::Owed => "owed",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
        }
    }
}

/// The state of a decision's generated `DurableDecisionMachine`, as its row
/// stores it: the machine's fields by name. The owner service rebuilds the
/// machine from it, and the machine's invariants refuse a torn one.
///
/// The party fields hold the machine's party keys: a principal key
/// ([`DecisionPrincipalKey`]) for the requester and the reviewer, and the
/// approver id under the `approver:` prefix.
///
/// Built field by field: its fields are the machine's, and a new one is a
/// machine change every builder must handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionMachineState {
    pub phase: DecisionPhase,
    pub decision_id: String,
    pub deadline_at_ms: u64,
    pub subject_digest: String,
    pub operation_id: String,
    pub request_digest: String,
    pub requester: String,
    pub approver_id: String,
    #[serde(default)]
    pub reviewer: Option<String>,
    pub stage: DecisionStage,
    /// When a complete presentation first reached the approver (the
    /// earliest, recorded or carried), in UNIX milliseconds.
    #[serde(default)]
    pub first_presented_to_approver_at_ms: Option<u64>,
    pub presented_to_reviewer: bool,
    #[serde(default)]
    pub evidence_digest: Option<String>,
    #[serde(default)]
    pub decided_at_ms: Option<u64>,
    #[serde(default)]
    pub ended_at_ms: Option<u64>,
    pub permit_issued: bool,
    #[serde(default)]
    pub executed_request_digest: Option<String>,
    pub notice: DecisionNotice,
}

/// What a caller asks to hold: one operation, sealed by the operation id and
/// request digest its owner computed, for an approver id (and a reviewer, in
/// the review stage) until a deadline. Built with [`Self::new`]; it may gain
/// optional fields.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DecisionHoldRequest {
    pub kind: DecisionKind,
    pub subject_key: DecisionSubjectKey,
    pub operation_id: DecisionOperationId,
    /// The held request's digest, as the operation's owner computes it.
    pub request_digest: Sha256Digest,
    pub requester: DecisionPrincipalKey,
    pub approver_id: ApproverId,
    /// The reviewer of a review-stage decision.
    pub reviewer: Option<DecisionPrincipalKey>,
    /// The sealed deadline, in UNIX milliseconds.
    pub deadline_at_ms: u64,
    pub request: DecisionRequestBody,
    pub rendering: DecisionRendering,
    pub renderer: DecisionRenderer,
    /// A subject digest the caller computed, if any. Core computes the
    /// subject digest from the rendering itself; a given one must equal it.
    pub expected_subject_digest: Option<Sha256Digest>,
}

impl DecisionHoldRequest {
    /// The hold of the operation `operation_id` (its request `request`,
    /// whose digest is `request_digest`) for the subject `subject_key` of
    /// `kind`, asked by `requester` for `approver_id` until `deadline_at_ms`,
    /// presented as `rendering` by `renderer`. It names no reviewer and no
    /// expected subject digest.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        kind: DecisionKind,
        subject_key: DecisionSubjectKey,
        operation_id: DecisionOperationId,
        request_digest: Sha256Digest,
        request: DecisionRequestBody,
        requester: DecisionPrincipalKey,
        approver_id: ApproverId,
        deadline_at_ms: u64,
        rendering: DecisionRendering,
        renderer: DecisionRenderer,
    ) -> Self {
        Self {
            kind,
            subject_key,
            operation_id,
            request_digest,
            requester,
            approver_id,
            reviewer: None,
            deadline_at_ms,
            request,
            rendering,
            renderer,
            expected_subject_digest: None,
        }
    }

    /// This hold in the review stage of `reviewer`.
    #[must_use]
    pub fn with_reviewer(mut self, reviewer: DecisionPrincipalKey) -> Self {
        self.reviewer = Some(reviewer);
        self
    }

    /// This hold with the subject digest the caller computed, which must be
    /// the rendering's.
    #[must_use]
    pub fn with_expected_subject_digest(mut self, digest: Sha256Digest) -> Self {
        self.expected_subject_digest = Some(digest);
        self
    }
}

/// A decision's immutable descriptor: what was held, for whom and until
/// when. The decision's machine never interprets it; the owner service does,
/// and binds the machine to it by digests and keys. Built by
/// [`Self::seal`]; it may gain fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DecisionDescriptor {
    pub kind: DecisionKind,
    pub subject_key: DecisionSubjectKey,
    pub operation_id: DecisionOperationId,
    pub request_digest: Sha256Digest,
    pub requester: DecisionPrincipalKey,
    pub approver_id: ApproverId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<DecisionPrincipalKey>,
    pub deadline_at_ms: u64,
    /// The host's clock reading of the hold, in UNIX milliseconds.
    pub created_at_ms: u64,
    pub request: DecisionRequestBody,
    pub rendering: DecisionRendering,
    pub renderer: DecisionRenderer,
    /// The rendering's digest, computed by Core.
    pub subject_digest: Sha256Digest,
}

impl DecisionDescriptor {
    /// Seal `request` at the clock reading `created_at_ms`: the subject
    /// digest is computed from the rendering, never taken from the caller.
    ///
    /// # Errors
    /// [`DecisionContractError::SubjectDigestMismatch`] when the request
    /// carries a subject digest other than the rendering's.
    pub fn seal(
        request: DecisionHoldRequest,
        created_at_ms: u64,
    ) -> Result<Self, DecisionContractError> {
        let subject_digest = request.rendering.subject_digest();
        if request
            .expected_subject_digest
            .as_ref()
            .is_some_and(|expected| *expected != subject_digest)
        {
            return Err(DecisionContractError::SubjectDigestMismatch);
        }
        Ok(Self {
            kind: request.kind,
            subject_key: request.subject_key,
            operation_id: request.operation_id,
            request_digest: request.request_digest,
            requester: request.requester,
            approver_id: request.approver_id,
            reviewer: request.reviewer,
            deadline_at_ms: request.deadline_at_ms,
            created_at_ms,
            request: request.request,
            rendering: request.rendering,
            renderer: request.renderer,
            subject_digest,
        })
    }

    /// The id of the decision this descriptor holds.
    #[must_use]
    pub fn decision_id(&self) -> DecisionId {
        DecisionId::derive(&self.kind, &self.subject_key)
    }
}

/// A decided decision's outcome: who decided, and the evidence. Built by
/// [`Self::of`]; it may gain fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DecisionOutcome {
    pub tier: DecisionTier,
    pub evidence: DecisionEvidence,
}

impl DecisionOutcome {
    /// The outcome `evidence` establishes: its tier is the evidence's.
    #[must_use]
    pub fn of(evidence: DecisionEvidence) -> Self {
        Self {
            tier: evidence.tier(),
            evidence,
        }
    }
}

/// One durable decision's row: its immutable descriptor, the state of its
/// generated machine, and the records its owner service keeps beside it.
/// Only the owner service writes rows, each write conditional on the
/// revision it read. A hold builds one with [`Self::held`]; it may gain
/// records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct StoredDecision {
    pub decision_id: DecisionId,
    pub descriptor: DecisionDescriptor,
    pub state: DecisionMachineState,
    /// The latest complete presentation per audience, at most
    /// [`DECISION_PRESENTATIONS_MAX`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presentations: Vec<PresentationRecord>,
    /// Set exactly when the decision was decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<DecisionOutcome>,
    /// The owner's typed permanent refusal of an execution-refused decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_refusal: Option<DecisionReference>,
    /// The receipt that settled the requester notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_receipt: Option<ApprovalConsentDeliveryState>,
    /// The row's revision: 1 at the hold, one more at each write.
    pub revision: u64,
}

impl StoredDecision {
    /// The row a hold creates: `descriptor`, under the id it derives, and
    /// the machine state the hold left, at revision 1, with no records
    /// beside them yet.
    #[must_use]
    pub fn held(descriptor: DecisionDescriptor, state: DecisionMachineState) -> Self {
        Self {
            decision_id: descriptor.decision_id(),
            descriptor,
            state,
            presentations: Vec::new(),
            outcome: None,
            execution_refusal: None,
            notice_receipt: None,
            revision: 1,
        }
    }
}

/// Where the consent an executed operation commits under came from. An
/// operation's owner seals it with the operation.
///
/// `Decided` comes from a durable decision's execution permit, which the
/// decision's owner service mints only for an approved decision with the
/// held binding before its deadline. `Imported` names consent decided
/// outside Meerkat; only an owner's import path accepts it. More kinds may
/// be added: a commit refuses one it does not know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConsentProvenance {
    /// Consent decided by a durable decision.
    Decided {
        decision_id: DecisionId,
        subject_digest: Sha256Digest,
        renderer: DecisionRenderer,
        tier: DecisionTier,
        approver_id: ApproverId,
        decided_at_ms: u64,
        evidence: DecisionEvidence,
    },
    /// Consent decided outside Meerkat and imported with its record.
    Imported {
        source: DecisionReference,
        #[serde(rename = "ref")]
        reference: DecisionReference,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approver_id: Option<ApproverId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decided_at_ms: Option<u64>,
    },
}
