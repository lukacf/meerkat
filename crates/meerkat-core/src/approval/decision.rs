//! Durable decisions: one held operation, decided by an attested party.
//!
//! A durable decision holds one operation (its request, sealed by the
//! operation id and request digest its owner computed) until an attested
//! party decides it, its requester or the host withdraws it, or its deadline
//! passes. The operation's owner then executes the held request at most
//! once, under an execution permit.
//!
//! This module holds the decision vocabulary every owner shares: the
//! identity and digest types ([`DecisionId`], [`Sha256Digest`], ...), the
//! canonical parts a rendering is presented in ([`canonical_parts`]), the
//! attested decision and attributed review a decision is made with
//! ([`AttestedDecision`], [`AttributedReview`]) and their durable evidence
//! ([`DecisionEvidence`]), the stored row ([`StoredDecision`]), the consent
//! provenance an executed operation seals ([`ConsentProvenance`]), and the
//! store contract ([`DecisionStore`]) with its in-memory store.
//!
//! The lifecycle of a decision is owned by the generated
//! `DurableDecisionMachine`; its owner service lives in `meerkat-runtime`
//! (`decision::DurableDecisionService`) and is the only writer of rows. A row
//! is never executable by itself: execution needs the owner's permit.
//!
//! This kind is separate from the in-run consent of an approval
//! ([`super::consent`]): it shares no record, file or machine with it.

mod evidence;
mod keys;
mod parts;
mod record;
mod store;
#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests;

pub use evidence::{
    AttestedDecision, AttributedReview, AuthenticatedDecisionEvent, DECISION_REASON_MAX_BYTES,
    DecisionEvidence, DecisionReviewVerdict, DecisionTier,
};
pub use keys::{
    ApproverId, DECISION_APPROVER_ID_MAX_BYTES, DECISION_REFERENCE_MAX_BYTES,
    DECISION_RENDERING_MAX_BYTES, DECISION_REQUEST_MAX_BYTES, DECISION_TOKEN_MAX_BYTES, DecisionId,
    DecisionIngress, DecisionKind, DecisionOperationId, DecisionPrincipalKey, DecisionReference,
    DecisionRenderer, DecisionRendering, DecisionRequestBody, DecisionSubjectKey, Sha256Digest,
};
pub use parts::{
    CanonicalPart, PresentationAudience, PresentationLink, PresentationRecord, canonical_parts,
};
pub use record::{
    ConsentProvenance, DECISION_PRESENTATIONS_MAX, DecisionCleanup, DecisionDescriptor,
    DecisionHoldRequest, DecisionMachineState, DecisionNotice, DecisionOutcome, DecisionPhase,
    DecisionStage, StoredDecision,
};
pub use store::{
    DECISION_PAGE_MAX, DecisionExecutorClaim, DecisionListFilter, DecisionPage, DecisionRow,
    DecisionStore, InMemoryDecisionStore, UnreadableDecision, decision_is_due, decision_owes_work,
};

/// The authority label of an approval branch: a one-time member forked from
/// a grantee to run the approved work of one durable decision, whose id is
/// the label's value. A host treats a member with this label as private to
/// no person: no conversation, viewer or memory of anyone's reaches it.
pub const APPROVAL_BRANCH_LABEL: &str = "decision.approval_branch";

/// The decision kinds decided by people only: a hold of one of them that
/// names a reviewer is refused
/// ([`DecisionContractError::ReviewStageNotAllowed`]), and each one's
/// executor refuses an execution whose provenance is not a person's. A
/// model reviewer's allow can therefore never approve a grant, a tool
/// grant, a release, a gate or a change of an agent (its creation, update,
/// archive, unarchive, removal or adoption): an agent never approves a
/// change of an agent. Nor can it answer the questions an agent asks a
/// person (`ask_user.questions`): a model never answers for one.
///
/// The list must grow with each kind decided by people only, whichever crate
/// owns the kind: a kind missing from it keeps a review stage.
pub const DECIDED_BY_PEOPLE_ONLY: &[&str] = &[
    "grant.child",
    "tool_grant.issue",
    "route.release",
    "gating.release",
    "identity.create",
    "identity.update",
    "identity.archive",
    "identity.unarchive",
    "identity.remove",
    "identity.adopt",
    "ask_user.questions",
];

/// Whether decisions of `kind` are decided by people only
/// ([`DECIDED_BY_PEOPLE_ONLY`]).
#[must_use]
pub fn decided_by_people_only(kind: &DecisionKind) -> bool {
    DECIDED_BY_PEOPLE_ONLY.contains(&kind.as_str())
}

/// A durable decision value is not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DecisionContractError {
    /// The value is not a valid `what`.
    #[error("invalid {what}")]
    Invalid { what: &'static str },
    /// A decision party must be a principal with an explicit trust domain.
    #[error("a decision party must be a qualified principal")]
    UnqualifiedPrincipal,
    /// A reviewer must be an agent principal.
    #[error("a reviewer must be an agent principal")]
    ReviewerNotAgent,
    /// The value is larger than its bound.
    #[error("the {what} exceeds {max_bytes} bytes")]
    TooLarge {
        what: &'static str,
        max_bytes: usize,
    },
    /// A subject digest the caller computed is not the digest Core computes
    /// from the rendering.
    #[error("the given subject digest is not the rendering's digest")]
    SubjectDigestMismatch,
    /// A hold names a reviewer for a kind decided by people only
    /// ([`DECIDED_BY_PEOPLE_ONLY`]): nothing is held.
    #[error(
        "review_stage_not_allowed: decisions of {kind} are decided by people only, so a hold names no reviewer"
    )]
    ReviewStageNotAllowed { kind: DecisionKind },
    /// A value could not be encoded.
    #[error("decision value could not be encoded: {0}")]
    Encoding(String),
}
