//! [`DecisionExecutorLoop`]: an executor of the decision kinds of a store.
//!
//! The owner of each decision kind of a store registers a
//! [`DecisionExecutor`]; for those kinds the loop is the only caller of the
//! execution and settlement calls of [`DurableDecisionService`] (the host
//! makes none of its own), and it keeps the obligations
//! [`DecisionExecutionPermit`] states:
//!
//! - **One executor per decision.** The loop holds the executor claim of
//!   each kind it executes for its lifetime
//!   ([`DurableDecisionService::claim_executor_kind`]). Another loop that
//!   registers the kind over the same store, in this process or another,
//!   executes none of its decisions while the claim lives: each of its
//!   passes lists the kind as unclaimed ([`DecisionExecutorPass::unclaimed`])
//!   and asks for the claim again. Loops of different kinds of one store run
//!   side by side. A pass takes the loop mutably and drives one decision at
//!   a time.
//! - **Never release after a commit.** A commit an executor reports is
//!   recorded executed; when that record fails, the permit stays outstanding
//!   and the next pass settles it first.
//! - **Settle first.** Within a pass the loop settles every permit it
//!   authorizes before it moves on. A permit a store records as outstanding
//!   when a pass reads it was left by an earlier executor of its kind, an
//!   earlier pass that was dropped, or a settlement call that failed, and
//!   nothing holds it any more: an execution whose future is dropped commits
//!   nothing after the drop that the executor's own record does not report
//!   ([`DecisionExecutor::execute`]). The loop asks that record whether a
//!   commit landed under the permit before anything else: a landed commit is
//!   recorded executed, and only a permit with no commit is released.
//!
//! The party that decided must still be current when the permit is issued:
//! the host answers through [`DecisionApproverCurrentness`], and a decision
//! whose party the host refuses ends refused with no permit. An answer the
//! host cannot give now, or evidence an owner change made stale before the
//! authorization committed, decides nothing: the decision is asked for again
//! at a later pass.
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

use super::DecisionWriteMode;
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
    ///
    /// The returned future may be dropped at any await (the host drops the
    /// pass). Nothing it started may commit after that unless
    /// [`Self::committed`] reports the commit: commit inline, or make the
    /// effect idempotent on the decision's operation id and have
    /// [`Self::committed`] read it. The next pass settles the permit from
    /// that record first, and executes again only when it reports no commit.
    async fn execute(
        &self,
        decision: &StoredDecision,
        permit: &DecisionExecutionPermit,
    ) -> DecisionExecution;

    /// Whether a commit landed for `decision` under its outstanding permit,
    /// read from the executor's own record of the operation only. `false`
    /// only when no commit can land under that permit any more: the permit
    /// is released, and the decision may execute again.
    async fn committed(
        &self,
        decision: &StoredDecision,
    ) -> Result<bool, DecisionExecutorRecordUnavailable>;

    /// Deliver the notice `decision` owes its requester: its receipt
    /// (admitted, or failed terminally), or `None` when it cannot be
    /// delivered now and is asked again at a later pass. A delivery whose
    /// receipt was not recorded is asked again too, so a notice is delivered
    /// at least once: deliver it idempotently per decision.
    async fn deliver_notice(
        &self,
        decision: &StoredDecision,
    ) -> Option<ApprovalConsentDeliveryState>;
}

/// The host's current answer for the party that decided a decision.
pub trait DecisionApproverCurrentness: Send + Sync {
    /// Whether the party that decided `decision` may still do so: the human
    /// who acted for its approver id or, for a decision its review stage
    /// allowed, its reviewer (its outcome's evidence names which). Answer for
    /// the party as it decided, not for its login: a login's natural expiry
    /// is not a revocation. Refuse only a party revoked or forbidden by
    /// policy since; that ends the decision. An unsettled answer, or
    /// evidence no longer current when the authorization commits, applies
    /// nothing and is asked for again at a later pass.
    fn approver_answer(&self, decision: &StoredDecision) -> ConsentEligibilityAnswer;
}

/// Why a decision was left for a later pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecisionDeferral {
    /// The owner of the deciding party's eligibility could not answer now,
    /// its evidence was no longer current when the authorization committed,
    /// or its answer is of a kind this version does not know.
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
    /// The host refused the party that decided: no permit was issued, and
    /// the decision ended refused.
    ApproverNotCurrent,
    /// A permit outstanding at the pass, under which a commit had landed:
    /// recorded executed.
    SettledExecuted,
    /// A permit outstanding at the pass, under which no commit had landed:
    /// released. At or past the deadline the release records the expiry,
    /// and the decision owes its notice.
    SettledReleased,
    /// The deadline passed: the expiry was recorded.
    Expired,
    /// The owed notice was settled with its receipt.
    NoticeSettled,
    /// Nothing could be done now; a later pass asks again.
    Deferred(DecisionDeferral),
    /// No executor of the decision's kind is registered in this loop. It
    /// was left as stored.
    NoExecutor,
    /// An executor of the decision's kind is registered in this loop, but
    /// the loop holds no claim of the kind: another executor holds it and
    /// executes the decision, or the claim could not be taken (the kind's
    /// entry in [`DecisionExecutorPass::unclaimed`] has the refusal). It was
    /// left as stored, and the next pass asks for the claim again.
    KindNotClaimed,
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
    /// Each registered kind whose executor claim the pass could not take,
    /// in kind order, with the refusal: [`DecisionError::ExecutorHeld`]
    /// while another executor holds the kind (or, as an executor of 0.9.10
    /// does, the whole store). The pass executed none of its decisions; the
    /// next pass asks for the claim again.
    pub unclaimed: Vec<(DecisionKind, DecisionError)>,
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
    /// The owner serves a candidate launch ([`DecisionError::CandidateLaunch`]),
    /// or the store cannot serve an executor claim.
    #[error(transparent)]
    Decision(#[from] DecisionError),
}

/// An executor of the registered decision kinds of a store, for each kind
/// whose executor claim it holds.
pub struct DecisionExecutorLoop {
    service: DurableDecisionService,
    kinds: BTreeMap<DecisionKind, RegisteredKind>,
    approver: Arc<dyn DecisionApproverCurrentness>,
}

/// One registered kind: its executor, and the loop's claim of the kind while
/// it holds one.
struct RegisteredKind {
    executor: Arc<dyn DecisionExecutor>,
    claim: Option<DecisionExecutorClaim>,
}

impl std::fmt::Debug for DecisionExecutorLoop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let claimed = |held: bool| {
            self.kinds
                .iter()
                .filter(move |(_, registered)| registered.claim.is_some() == held)
                .map(|(kind, _)| kind)
                .collect::<Vec<_>>()
        };
        f.debug_struct("DecisionExecutorLoop")
            .field("service", &self.service)
            .field("claimed_kinds", &claimed(true))
            .field("unclaimed_kinds", &claimed(false))
            .finish_non_exhaustive()
    }
}

/// The page after `after` when `next` advances past it.
fn next_page(after: Option<&str>, next: Option<String>) -> Option<String> {
    next.filter(|next| after != Some(next.as_str()))
}

impl DecisionExecutorLoop {
    /// Start the loop for `executors`, one per decision kind, and claim the
    /// executor role of each kind in `service`'s store. A kind another
    /// executor holds is not claimed: the loop starts without it, and each
    /// pass asks for its claim again ([`DecisionExecutorPass::unclaimed`]).
    /// Nothing else is read or written.
    ///
    /// # Errors
    /// [`DecisionExecutorLoopError::DuplicateKind`] for two executors of one
    /// kind, and [`DecisionError::CandidateLaunch`] for a candidate launch,
    /// which starts no executor (nothing is claimed for either); a store that
    /// cannot serve an executor claim ([`DecisionError::Unavailable`]: the
    /// claims taken are released).
    pub fn start(
        service: DurableDecisionService,
        executors: impl IntoIterator<Item = Arc<dyn DecisionExecutor>>,
        approver: Arc<dyn DecisionApproverCurrentness>,
    ) -> Result<Self, DecisionExecutorLoopError> {
        let mut kinds = BTreeMap::new();
        for executor in executors {
            let kind = executor.kind().clone();
            let registered = RegisteredKind {
                executor,
                claim: None,
            };
            if kinds.insert(kind.clone(), registered).is_some() {
                return Err(DecisionExecutorLoopError::DuplicateKind(kind));
            }
        }
        if service.write_mode() == DecisionWriteMode::CandidateLaunch {
            return Err(DecisionError::CandidateLaunch.into());
        }
        for (kind, registered) in &mut kinds {
            match service.claim_executor_kind(kind) {
                Ok(claim) => registered.claim = Some(claim),
                Err(DecisionError::ExecutorHeld) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(Self {
            service,
            kinds,
            approver,
        })
    }

    /// One pass over the store's work: first a claim of each registered
    /// kind the loop does not hold, then the decisions due an expiry
    /// observation (their expiry is recorded), then the owed ones (approved
    /// decisions are settled first and executed; ended ones get their
    /// notice). A decision of a kind with no executor in this loop, or of a
    /// kind whose claim it does not hold, is left as stored.
    ///
    /// A pass may be dropped at any await: a permit it issued and did not
    /// settle stays outstanding, and the next pass, of this loop or the
    /// next, settles it first.
    pub async fn pass(&mut self) -> DecisionExecutorPass {
        let mut pass = DecisionExecutorPass::default();
        self.claim_unclaimed_kinds(&mut pass);
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

    /// Claim each registered kind the loop holds no claim of. A kind whose
    /// claim is refused is listed in `pass` with the refusal, and asked for
    /// again at the next pass.
    fn claim_unclaimed_kinds(&mut self, pass: &mut DecisionExecutorPass) {
        for (kind, registered) in &mut self.kinds {
            if registered.claim.is_some() {
                continue;
            }
            match self.service.claim_executor_kind(kind) {
                Ok(claim) => registered.claim = Some(claim),
                Err(error) => pass.unclaimed.push((kind.clone(), error)),
            }
        }
    }

    /// The executor of `row`'s kind, when this loop holds the kind's claim;
    /// otherwise the outcome the pass records for the row.
    fn executor_of(
        &self,
        row: &StoredDecision,
    ) -> Result<Arc<dyn DecisionExecutor>, DecisionPassOutcome> {
        match self.kinds.get(&row.descriptor.kind) {
            Some(registered) if registered.claim.is_some() => Ok(Arc::clone(&registered.executor)),
            Some(_) => Err(DecisionPassOutcome::KindNotClaimed),
            None => Err(DecisionPassOutcome::NoExecutor),
        }
    }

    /// Record the expiry a due decision shows.
    fn observe_expiry(&self, row: &StoredDecision, pass: &mut DecisionExecutorPass) {
        if let Err(outcome) = self.executor_of(row) {
            pass.record(&row.decision_id, outcome);
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
        let executor = match self.executor_of(&row) {
            Ok(executor) => executor,
            Err(outcome) => {
                pass.record(&row.decision_id, outcome);
                return;
            }
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
