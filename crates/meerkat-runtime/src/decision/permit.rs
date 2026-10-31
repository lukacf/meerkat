//! [`DecisionExecutionPermit`]: the right to execute one approved decision's
//! held request.

use meerkat_core::approval::decision::{
    ConsentProvenance, DecisionId, DecisionOperationId, Sha256Digest,
};

/// The machine's authorization to execute one approved decision's held
/// request.
///
/// Only [`DurableDecisionService::authorize_execution`] mints one, from the
/// machine's authorization of an approved decision with the held binding
/// before its deadline: the decision's machine records the permit as
/// outstanding, and the decision then waits for its settlement instead of
/// expiring. The operation's owner commits under it at most once and
/// settles it: [`DurableDecisionService::record_executed`] when the commit
/// landed, [`DurableDecisionService::release_execution`] when it did not,
/// or [`DurableDecisionService::close_refused`] for a permanent refusal.
/// The last two consume the permit, so a settled permit can never pass
/// [`Self::binds`] or [`Self::admits_commit_at`] again.
///
/// It carries the held binding, which an owner's commit compares with its
/// own ([`Self::binds`]), the deadline, at or after which a commit refuses
/// it ([`Self::admits_commit_at`]), and the decided consent the commit
/// seals. It is not serializable: it never leaves the process that was
/// granted it.
///
/// # The executor's obligations
///
/// The machine records that a permit is outstanding, not who holds it or
/// whether a commit landed under it. Its owner, the executor, keeps these:
///
/// - **One executor per decision.** One executor (one process) holds the
///   permit of a decision at a time. Authorizing again before the deadline
///   answers an equal permit for that executor's own retry, never for a
///   second executor. Authorize again only when the previous permit object
///   is gone (a crash, or a settlement call that failed): never hold two
///   permits of one decision, since settling one leaves its twin able to
///   pass [`Self::binds`] and [`Self::admits_commit_at`].
/// - **Never release after a commit.** Once a commit landed under the
///   permit, its only settlement is `record_executed`, accepted at any
///   time. A release or a refusal after a commit would let the decision
///   expire, or end refused, while the committed operation seals its
///   consent.
/// - **Settle first on recovery.** After a restart, or after a settlement
///   call that failed, the permit object is gone and the decision still
///   waits. Before anything else for that decision, the executor reads its
///   own record of the operation: a landed commit is recorded with
///   `record_executed`; only when no commit landed does it release
///   ([`DurableDecisionService::release_after_restart`]) or refuse
///   ([`DurableDecisionService::close_refused_after_restart`]) the
///   outstanding permit.
///
/// A dropped permit leaves its decision waiting (it never expires) until
/// the executor settles it as above.
///
/// [`DurableDecisionService::authorize_execution`]: super::DurableDecisionService::authorize_execution
/// [`DurableDecisionService::record_executed`]: super::DurableDecisionService::record_executed
/// [`DurableDecisionService::release_execution`]: super::DurableDecisionService::release_execution
/// [`DurableDecisionService::close_refused`]: super::DurableDecisionService::close_refused
/// [`DurableDecisionService::release_after_restart`]: super::DurableDecisionService::release_after_restart
/// [`DurableDecisionService::close_refused_after_restart`]: super::DurableDecisionService::close_refused_after_restart
#[derive(Debug, PartialEq, Eq)]
#[must_use = "an issued permit waits for its settlement"]
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

    /// Consume the permit for its settlement: the decision it settles.
    pub(super) fn into_decision_id(self) -> DecisionId {
        self.decision_id
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
