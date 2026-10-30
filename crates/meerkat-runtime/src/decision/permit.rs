//! [`DecisionExecutionPermit`]: the right to execute one approved decision's
//! held request.

use meerkat_core::approval::decision::{
    ConsentProvenance, DecisionId, DecisionOperationId, Sha256Digest,
};

/// The machine's authorization to execute one approved decision's held
/// request.
///
/// Only [`DurableDecisionService::authorize_execution`] mints one, and only
/// for an approved decision, with the held binding, before its deadline:
/// the decision's machine records the permit as outstanding, and the
/// decision then waits for its settlement instead of expiring. The
/// operation's owner commits under it at most once and settles it:
/// [`DurableDecisionService::record_executed`] when the commit landed,
/// [`DurableDecisionService::release_execution`] when it did not, or
/// [`DurableDecisionService::close_refused`] for a permanent refusal.
///
/// It carries the held binding, which an owner's commit compares with its
/// own, the deadline, at or after which a commit refuses it
/// ([`Self::admits_commit_at`]), and the decided consent the commit seals.
/// It is not serializable: it never leaves the process that was granted it.
///
/// [`DurableDecisionService::authorize_execution`]: super::DurableDecisionService::authorize_execution
/// [`DurableDecisionService::record_executed`]: super::DurableDecisionService::record_executed
/// [`DurableDecisionService::release_execution`]: super::DurableDecisionService::release_execution
/// [`DurableDecisionService::close_refused`]: super::DurableDecisionService::close_refused
#[derive(Debug, PartialEq, Eq)]
pub struct DecisionExecutionPermit {
    decision_id: DecisionId,
    operation_id: DecisionOperationId,
    request_digest: Sha256Digest,
    deadline_at_ms: u64,
    consent: ConsentProvenance,
}

impl DecisionExecutionPermit {
    pub(super) fn new(
        decision_id: DecisionId,
        operation_id: DecisionOperationId,
        request_digest: Sha256Digest,
        deadline_at_ms: u64,
        consent: ConsentProvenance,
    ) -> Self {
        Self {
            decision_id,
            operation_id,
            request_digest,
            deadline_at_ms,
            consent,
        }
    }

    /// The decision this permit executes.
    #[must_use]
    pub fn decision_id(&self) -> &DecisionId {
        &self.decision_id
    }

    /// The held operation's id.
    #[must_use]
    pub fn operation_id(&self) -> &DecisionOperationId {
        &self.operation_id
    }

    /// The held request's digest.
    #[must_use]
    pub fn request_digest(&self) -> &Sha256Digest {
        &self.request_digest
    }

    /// The decision's sealed deadline, in UNIX milliseconds.
    #[must_use]
    pub fn deadline_at_ms(&self) -> u64 {
        self.deadline_at_ms
    }

    /// Whether a commit at the clock reading `now_ms` may use this permit:
    /// only before its deadline.
    #[must_use]
    pub fn admits_commit_at(&self, now_ms: u64) -> bool {
        now_ms < self.deadline_at_ms
    }

    /// Whether the permit is for exactly this binding.
    #[must_use]
    pub fn binds(&self, operation_id: &DecisionOperationId, request_digest: &Sha256Digest) -> bool {
        self.operation_id == *operation_id && self.request_digest == *request_digest
    }

    /// The decided consent the commit seals: always
    /// [`ConsentProvenance::Decided`].
    #[must_use]
    pub fn consent(&self) -> &ConsentProvenance {
        &self.consent
    }
}
