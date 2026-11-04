//! [`DecisionExecutorLoop`]: the one executor of a decision store.
//!
//! The owner of each decision kind of a store registers a
//! [`DecisionExecutor`]; the loop is the only caller of the execution and
//! settlement calls of [`DurableDecisionService`] for those kinds, and it
//! keeps the obligations [`DecisionExecutionPermit`] states:
//!
//! - **One executor per decision.** The loop holds the store's executor
//!   claim for its lifetime ([`DurableDecisionService::claim_executor`]): a
//!   second loop over the same store, in this process or another, is refused
//!   at start. A pass takes the loop mutably and drives one decision at a
//!   time.
//! - **Never release after a commit.** A commit an executor reports is
//!   recorded executed; when that record fails, the permit stays outstanding
//!   and the next pass settles it first.
//! - **Settle first.** Within a pass the loop settles every permit it
//!   authorizes before it moves on, so a permit a store records as
//!   outstanding when a pass reads it has no live holder. The loop asks the
//!   executor's own record whether a commit landed under it before anything
//!   else: a landed commit is recorded executed, and only a permit with no
//!   commit is released.
//!
//! The approver who decided must still be current when the permit is
//! issued: the host answers through [`DecisionApproverCurrentness`], and a
//! decision whose approver is no longer current ends refused with no permit.
//!
//! A host drives passes ([`DecisionExecutorLoop::pass`]): once at start,
//! after it records a decision, and on a timer for the deadlines. A pass
//! reads the due and owed pages of the store, never every row.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::approval::ApprovalConsentDeliveryState;
use meerkat_core::approval::consent::ConsentEligibilityAnswer;
use meerkat_core::approval::decision::{
    DECISION_PAGE_MAX, DecisionExecutorClaim, DecisionId, DecisionKind, DecisionNotice,
    DecisionPhase, DecisionReference, StoredDecision, UnreadableDecision,
};

use super::permit::DecisionExecutionPermit;
use super::service::{DecisionError, DecisionListing, DecisionRefusal, DurableDecisionService};

/// How one execution of a held request ended.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecisionExecution {
    /// The held request was committed under the permit.
    Committed,
    /// Nothing was committed, and the attempt may be made again: the
    /// decision stays approved.
    Released,
    /// The owner refused the held request permanently, with this typed
    /// reference; nothing was committed.
    Refused(DecisionReference),
}

/// The executor cannot read its own record of an operation now.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the executor cannot read its record of the operation: {0}")]
pub struct DecisionExecutorRecordUnavailable(pub String);

/// The owner of one decision kind's held requests.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait DecisionExecutor: Send + Sync {
    /// The decision kind this executor executes.
    fn kind(&self) -> &DecisionKind;

    /// Execute the held request of `decision` under `permit`, at most once.
    /// A commit is made only for the permit's binding
    /// ([`DecisionExecutionPermit::binds`]), before its deadline
    /// ([`DecisionExecutionPermit::admits_commit_at`]), and seals its
    /// consent ([`DecisionExecutionPermit::consent`]).
    async fn execute(
        &self,
        decision: &StoredDecision,
        permit: &DecisionExecutionPermit,
    ) -> DecisionExecution;

    /// Whether a commit landed for `decision` under its outstanding permit,
    /// read from the executor's own record of the operation only.
    async fn committed(
        &self,
        decision: &StoredDecision,
    ) -> Result<bool, DecisionExecutorRecordUnavailable>;

    /// Deliver the notice `decision` owes its requester: its receipt
    /// (admitted, or failed terminally), or `None` when it cannot be
    /// delivered now and is asked again at a later pass.
    async fn deliver_notice(
        &self,
        decision: &StoredDecision,
    ) -> Option<ApprovalConsentDeliveryState>;
}

/// The host's current answer for the approver who decided a decision.
pub trait DecisionApproverCurrentness: Send + Sync {
    /// Whether the approver who decided `decision` may still do so: not
    /// revoked, and not forbidden by policy since. Evidence is checked again
    /// when the authorization commits; an unsettled answer applies nothing.
    fn approver_answer(&self, decision: &StoredDecision) -> ConsentEligibilityAnswer;
}

/// Why a decision was left for a later pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecisionDeferral {
    /// The owner of the approver's eligibility could not answer now.
    ApproverUnsettled,
    /// The executor could not read its own record of the operation.
    RecordUnavailable,
    /// The notice could not be delivered now.
    NoticeUndelivered,
}

/// What a pass did to one decision.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecisionPassOutcome {
    /// The held request was committed and recorded executed.
    Executed,
    /// The attempt released its permit with no commit.
    Released,
    /// The owner refused the held request; the decision ended refused.
    ExecutionRefused,
    /// The approver who decided is no longer current: no permit was issued,
    /// and the decision ended refused.
    ApproverNotCurrent,
    /// A permit outstanding at the pass, under which a commit had landed:
    /// recorded executed.
    SettledExecuted,
    /// A permit outstanding at the pass, under which no commit had landed:
    /// released.
    SettledReleased,
    /// The deadline passed: the expiry was recorded.
    Expired,
    /// The owed notice was settled with its receipt.
    NoticeSettled,
    /// Nothing could be done now; a later pass asks again.
    Deferred(DecisionDeferral),
    /// No executor is registered for the decision's kind: it was left as
    /// stored.
    NoExecutor,
    /// A call failed; the decision is left for a later pass.
    Failed(DecisionError),
}

/// What one pass did. It may gain fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecisionExecutorPass {
    /// Each decision the pass acted on, with what it did, in order.
    pub outcomes: Vec<(DecisionId, DecisionPassOutcome)>,
    /// The rows of the pages the pass read that this binary cannot read;
    /// none is executed.
    pub unreadable: Vec<UnreadableDecision>,
    /// A page read that failed; the pass read no further page of that
    /// queue.
    pub read_failures: Vec<DecisionError>,
}

impl DecisionExecutorPass {
    fn record(&mut self, decision_id: &DecisionId, outcome: DecisionPassOutcome) {
        self.outcomes.push((decision_id.clone(), outcome));
    }
}

/// The loop could not start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DecisionExecutorLoopError {
    /// Two executors were registered for one decision kind.
    #[error("two executors are registered for the decision kind {0}")]
    DuplicateKind(DecisionKind),
    /// The store's executor claim was refused: another executor holds it
    /// ([`DecisionError::ExecutorHeld`]), the owner serves a candidate
    /// launch, or the store cannot serve the claim.
    #[error(transparent)]
    Decision(#[from] DecisionError),
}

/// The one executor of a decision store's registered kinds.
pub struct DecisionExecutorLoop {
    service: DurableDecisionService,
    executors: BTreeMap<DecisionKind, Arc<dyn DecisionExecutor>>,
    approver: Arc<dyn DecisionApproverCurrentness>,
    _claim: DecisionExecutorClaim,
}

impl std::fmt::Debug for DecisionExecutorLoop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionExecutorLoop")
            .field("service", &self.service)
            .field("kinds", &self.executors.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// The page after `after` when `next` advances past it.
fn next_page(after: Option<&str>, next: Option<String>) -> Option<String> {
    next.filter(|next| after != Some(next.as_str()))
}

impl DecisionExecutorLoop {
    /// Claim the executor role of `service`'s store and start the loop for
    /// `executors`, one per decision kind. Nothing else is read or written.
    ///
    /// # Errors
    /// [`DecisionExecutorLoopError::DuplicateKind`] for two executors of one
    /// kind (nothing is claimed); [`DecisionError::ExecutorHeld`] while
    /// another executor holds the store; [`DecisionError::CandidateLaunch`]
    /// for a candidate launch, which starts no executor; other store
    /// failures.
    pub fn start(
        service: DurableDecisionService,
        executors: impl IntoIterator<Item = Arc<dyn DecisionExecutor>>,
        approver: Arc<dyn DecisionApproverCurrentness>,
    ) -> Result<Self, DecisionExecutorLoopError> {
        let mut by_kind = BTreeMap::new();
        for executor in executors {
            let kind = executor.kind().clone();
            if by_kind.insert(kind.clone(), executor).is_some() {
                return Err(DecisionExecutorLoopError::DuplicateKind(kind));
            }
        }
        let claim = service.claim_executor()?;
        Ok(Self {
            service,
            executors: by_kind,
            approver,
            _claim: claim,
        })
    }

    /// One pass over the store's work: first the decisions due an expiry
    /// observation (their expiry is recorded), then the owed ones (approved
    /// decisions are settled first and executed; ended ones get their
    /// notice). A decision of a kind with no executor is left as stored.
    pub async fn pass(&mut self) -> DecisionExecutorPass {
        let mut pass = DecisionExecutorPass::default();
        let mut after: Option<String> = None;
        loop {
            let listing = match self.service.due(after.as_deref(), DECISION_PAGE_MAX) {
                Ok(listing) => listing,
                Err(error) => {
                    pass.read_failures.push(error);
                    break;
                }
            };
            let DecisionListing {
                decisions,
                unreadable,
                next_after,
                ..
            } = listing;
            pass.unreadable.extend(unreadable);
            for row in decisions {
                self.observe_expiry(&row, &mut pass);
            }
            match next_page(after.as_deref(), next_after) {
                Some(next) => after = Some(next),
                None => break,
            }
        }
        let mut after: Option<String> = None;
        loop {
            let listing = match self.service.owed(after.as_deref(), DECISION_PAGE_MAX) {
                Ok(listing) => listing,
                Err(error) => {
                    pass.read_failures.push(error);
                    break;
                }
            };
            let DecisionListing {
                decisions,
                unreadable,
                next_after,
                ..
            } = listing;
            pass.unreadable.extend(unreadable);
            for row in decisions {
                self.work(row, &mut pass).await;
            }
            match next_page(after.as_deref(), next_after) {
                Some(next) => after = Some(next),
                None => break,
            }
        }
        pass
    }

    fn executor_of(&self, row: &StoredDecision) -> Option<Arc<dyn DecisionExecutor>> {
        self.executors.get(&row.descriptor.kind).cloned()
    }

    /// Record the expiry a due decision shows.
    fn observe_expiry(&self, row: &StoredDecision, pass: &mut DecisionExecutorPass) {
        if self.executor_of(row).is_none() {
            pass.record(&row.decision_id, DecisionPassOutcome::NoExecutor);
            return;
        }
        match self.service.get(&row.decision_id) {
            Ok(read) if read.state.phase == DecisionPhase::Expired => {
                pass.record(&row.decision_id, DecisionPassOutcome::Expired);
            }
            Ok(_) => {}
            Err(error) => pass.record(&row.decision_id, DecisionPassOutcome::Failed(error)),
        }
    }

    /// The work one owed decision owes.
    async fn work(&self, row: StoredDecision, pass: &mut DecisionExecutorPass) {
        let Some(executor) = self.executor_of(&row) else {
            pass.record(&row.decision_id, DecisionPassOutcome::NoExecutor);
            return;
        };
        match row.state.phase {
            DecisionPhase::Approved => {
                let row = if row.state.permit_issued {
                    match self.settle_first(executor.as_ref(), &row, pass).await {
                        Some(released) => released,
                        None => return,
                    }
                } else {
                    row
                };
                self.execute(executor.as_ref(), &row, pass).await;
            }
            _ if row.state.notice == DecisionNotice::Owed => {
                self.deliver_notice(executor.as_ref(), &row, pass).await;
            }
            _ => {}
        }
    }

    /// Settle a permit outstanding when the pass read it: it has no live
    /// holder. A landed commit is recorded executed; with none, the permit
    /// is released, and the decision returned when it is still approved,
    /// for execution now.
    async fn settle_first(
        &self,
        executor: &dyn DecisionExecutor,
        row: &StoredDecision,
        pass: &mut DecisionExecutorPass,
    ) -> Option<StoredDecision> {
        let decision_id = &row.decision_id;
        match executor.committed(row).await {
            Ok(true) => {
                let outcome = match self.service.record_executed(
                    decision_id,
                    &row.descriptor.operation_id,
                    &row.descriptor.request_digest,
                ) {
                    Ok(_) => DecisionPassOutcome::SettledExecuted,
                    Err(error) => DecisionPassOutcome::Failed(error),
                };
                pass.record(decision_id, outcome);
                None
            }
            Ok(false) => match self.service.release_after_restart(decision_id) {
                Ok(released) => {
                    pass.record(decision_id, DecisionPassOutcome::SettledReleased);
                    (released.state.phase == DecisionPhase::Approved
                        && !released.state.permit_issued)
                        .then_some(released)
                }
                Err(error) => {
                    pass.record(decision_id, DecisionPassOutcome::Failed(error));
                    None
                }
            },
            Err(_) => {
                pass.record(
                    decision_id,
                    DecisionPassOutcome::Deferred(DecisionDeferral::RecordUnavailable),
                );
                None
            }
        }
    }

    /// Authorize an approved decision with no outstanding permit, execute
    /// its held request and settle the permit.
    async fn execute(
        &self,
        executor: &dyn DecisionExecutor,
        row: &StoredDecision,
        pass: &mut DecisionExecutorPass,
    ) {
        let decision_id = &row.decision_id;
        let answer = self.approver.approver_answer(row);
        let permit = match self.service.authorize_execution(
            decision_id,
            &row.descriptor.operation_id,
            &row.descriptor.request_digest,
            &answer,
        ) {
            Ok(permit) => permit,
            Err(error) => {
                let outcome = match error {
                    DecisionError::Refused {
                        refusal: DecisionRefusal::NotEligible,
                        ..
                    } => DecisionPassOutcome::ApproverNotCurrent,
                    DecisionError::Refused {
                        refusal: DecisionRefusal::Expired,
                        ..
                    } => DecisionPassOutcome::Expired,
                    DecisionError::Unsettled(_) => {
                        DecisionPassOutcome::Deferred(DecisionDeferral::ApproverUnsettled)
                    }
                    error => DecisionPassOutcome::Failed(error),
                };
                pass.record(decision_id, outcome);
                return;
            }
        };
        let outcome = match executor.execute(row, &permit).await {
            DecisionExecution::Committed => match self.service.record_executed(
                decision_id,
                permit.operation_id(),
                permit.request_digest(),
            ) {
                Ok(_) => DecisionPassOutcome::Executed,
                Err(error) => DecisionPassOutcome::Failed(error),
            },
            DecisionExecution::Released => match self.service.release_execution(permit) {
                Ok(_) => DecisionPassOutcome::Released,
                Err(error) => DecisionPassOutcome::Failed(error),
            },
            DecisionExecution::Refused(reference) => {
                match self.service.close_refused(permit, &reference) {
                    Ok(_) => DecisionPassOutcome::ExecutionRefused,
                    Err(error) => DecisionPassOutcome::Failed(error),
                }
            }
        };
        pass.record(decision_id, outcome);
    }

    /// Deliver the notice an ended decision owes and settle it.
    async fn deliver_notice(
        &self,
        executor: &dyn DecisionExecutor,
        row: &StoredDecision,
        pass: &mut DecisionExecutorPass,
    ) {
        let outcome = match executor.deliver_notice(row).await {
            Some(receipt) => match self.service.settle_notice(&row.decision_id, &receipt) {
                Ok(_) => DecisionPassOutcome::NoticeSettled,
                Err(error) => DecisionPassOutcome::Failed(error),
            },
            None => DecisionPassOutcome::Deferred(DecisionDeferral::NoticeUndelivered),
        };
        pass.record(&row.decision_id, outcome);
    }
}
