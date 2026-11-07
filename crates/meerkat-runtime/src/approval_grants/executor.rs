//! [`GrantDecisionExecutor`]: the executor of `grant.child` decisions, and
//! the cleanup of what their executions leave outside the decision store.
//!
//! An execution opens an approval branch of the agent that asked and issues
//! the branch its controller grant before it issues the approval-carried
//! grant. Before it opens anything it opens the decision's cleanup fact
//! (`DurableDecisionService::owe_cleanup`), which stays owed until the branch
//! is retired, the controller grant revoked and the fact settled
//! (`DurableDecisionService::settle_cleanup`). Every path that ends the
//! approved work (its watcher, a refused admission, an ended decision's
//! notice) cleans up the same way, and Core's pass sweeps whatever a path
//! left: it reads the unrevoked branch controller grants from the grant
//! owner and the open branches from the host, and reads each one's decision.

use std::collections::BTreeSet;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_authorization::grants::{
    ApprovalChildRefusal, ChildGrantRefusal, ChildGrantValidity, GrantApproval,
    GrantDecisionAssurance, GrantDecisionTier, GrantPrincipal, GrantRefusal,
};
use meerkat_authorization_contracts::constraints::LifetimeBound;
use meerkat_authorization_contracts::derived_child::DerivedChildRestrictions;
use meerkat_authorization_contracts::work_association::GrantLineageRef;
use meerkat_core::approval::decision::{
    ConsentProvenance, DecisionEvidence, DecisionId, DecisionKind, DecisionPhase,
    DecisionReference, DecisionTier, StoredDecision,
};
use meerkat_core::approval::{ApprovalAssurance, ApprovalConsentDeliveryState};

use super::ApprovalGrants;
use super::body::{
    GrantChildRequest, approval_branch_principal, approval_grant_id, branch_controller_grant_id,
    decision_of_branch_controller,
};
use super::ingress::ApprovalCarriedIngress;
use super::offer::{approval_child_request, branch_controller_intent, child_terms};
use super::seams::{AccessOutcome, ApprovalBranchError, ApprovalBranchRequest};
use crate::decision::{
    DecisionDeferral, DecisionError, DecisionExecution, DecisionExecutionPermit, DecisionExecutor,
    DecisionExecutorRecordUnavailable, DecisionPassOutcome, DecisionRefusal,
};

/// The executor of `grant.child` decisions (2.a.2). Register it only in the
/// runtime that holds the grant owner's single-writer custody.
///
/// It executes an approved offer at most once: the decider's provenance
/// must be a person's; the offering grant's lineage must still resolve; the
/// effective terms of now must equal the presented ones (`terms_changed`
/// otherwise); it opens the decision's cleanup, then the approval branch
/// (idempotent per decision), issues the branch's controller grant
/// (recovered on retry), then the approval-carried grant, whose id derives
/// from the decision. A commit landed exactly when that grant exists. The
/// decision's notice admits the approved work into the branch the execution
/// opened, then tells the origin it was approved; an admission that fails
/// for good tells the origin it was refused. An ended decision that executed
/// nothing retires any branch it opened and tells the origin how it ended,
/// with no content. Each pass of the loop sweeps the cleanup that is due
/// ([`DecisionExecutor::sweep`]).
pub struct GrantDecisionExecutor {
    grants: ApprovalGrants,
}

impl GrantDecisionExecutor {
    /// The executor of `grants`' decisions.
    #[must_use]
    pub fn new(grants: ApprovalGrants) -> Self {
        Self { grants }
    }
}

impl std::fmt::Debug for GrantDecisionExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrantDecisionExecutor")
            .finish_non_exhaustive()
    }
}

fn refused(reference: &str) -> DecisionExecution {
    match DecisionReference::new(reference) {
        Ok(reference) => DecisionExecution::Refused(reference),
        Err(_) => DecisionExecution::Released,
    }
}

/// The approval a permit's provenance carries: a person's decision for the
/// rule's approver id, with the decider's assurance. `None` for anything
/// else: kinds of this track are decided by people only.
fn approval_of(
    decision: &StoredDecision,
    permit: &DecisionExecutionPermit,
    request: &GrantChildRequest,
) -> Option<GrantApproval> {
    let ConsentProvenance::Decided {
        decision_id,
        tier: DecisionTier::Human,
        approver_id,
        evidence: DecisionEvidence::Human { assurance, .. },
        ..
    } = permit.consent()
    else {
        return None;
    };
    if decision_id != &decision.decision_id {
        return None;
    }
    let assurance = match assurance {
        ApprovalAssurance::PersonAuthenticated => GrantDecisionAssurance::PersonAuthenticated,
        ApprovalAssurance::ServiceCredential => GrantDecisionAssurance::ServiceCredential,
        ApprovalAssurance::DeviceTrusted => GrantDecisionAssurance::DeviceTrusted,
    };
    let terms = &request.terms;
    Some(GrantApproval {
        decision_id: decision_id.clone(),
        approver_id: approver_id.clone(),
        tier: GrantDecisionTier::Human,
        assurance,
        request_digest: permit.request_digest().clone(),
        requester: GrantPrincipal::new(terms.requester.clone()).ok()?,
        work: terms.work.clone(),
        operation: terms.operation.clone(),
        uses: terms.uses,
        review_before_return: terms.review_before_return.clone(),
    })
}

/// The window end of `restrictions`, when they are bounded by a window.
fn window_end(
    restrictions: &meerkat_authorization_contracts::constraints::ExecutionRestrictions,
) -> Option<u64> {
    match restrictions.lifetime.bound() {
        LifetimeBound::Window { expires_at_ms, .. } => Some(expires_at_ms),
        _ => None,
    }
}

/// How far undoing what an execution created got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cleanup {
    /// Done: nothing of it is left to undo, or its fact is settled.
    Done,
    /// A step could not be taken now; the next pass takes it.
    Pending,
}

/// Whether the cleanup of a decision is due now, as a sweep reads it.
enum Due {
    /// Its work is over: undo what its execution created.
    Now,
    /// Its execution may still be in flight, or its work may still run.
    NotYet,
    /// It cannot be told now: nothing is done this pass.
    Unknown,
}

impl ApprovalGrants {
    /// The issued approval-carried grant of `decision`, when one landed.
    fn issued_approval(
        &self,
        decision: &DecisionId,
    ) -> Result<Option<GrantLineageRef>, GrantRefusal> {
        let id = approval_grant_id(decision).ok_or(GrantRefusal::Denied)?;
        self.0.grants.issued_reference(&id)
    }

    /// Undo what the execution of `decision` created outside the decision
    /// store: retire its approval branch (retiring a retired or never
    /// opened branch is done), then revoke the branch's controller grant
    /// when one was issued and is not revoked. Each step is taken once; a
    /// step that cannot be taken now is logged and left to the next pass,
    /// which finds the unrevoked controller grant or the open branch again.
    pub(crate) async fn undo(&self, decision: &DecisionId) -> Cleanup {
        let inner = &self.0;
        match inner.branches.retire(decision).await {
            Ok(()) => {}
            Err(ApprovalBranchError::Refused(reason)) => tracing::warn!(
                decision = %decision,
                %reason,
                "the host refused to retire the approval branch; its controller grant is \
                 revoked all the same"
            ),
            Err(error) => {
                tracing::warn!(
                    decision = %decision,
                    %error,
                    "the approval branch cannot be retired now; the next pass retires it"
                );
                return Cleanup::Pending;
            }
        }
        let Some(id) = branch_controller_grant_id(decision) else {
            tracing::error!(
                decision = %decision,
                "the decision derives no branch controller grant id"
            );
            return Cleanup::Pending;
        };
        let reference = match inner.grants.unrevoked_grants(|candidate| *candidate == id) {
            Ok(mut unrevoked) => match unrevoked.pop() {
                Some(reference) => reference,
                // Never issued, or revoked already.
                None => return Cleanup::Done,
            },
            Err(refusal) => {
                tracing::warn!(
                    decision = %decision,
                    %refusal,
                    "the grant owner cannot say now whether the branch controller grant is \
                     revoked; the next pass revokes it"
                );
                return Cleanup::Pending;
            }
        };
        let issuer = match inner.grants.retained_grant(&reference) {
            Ok(retained) => retained.issuer,
            Err(refusal) => {
                tracing::warn!(
                    decision = %decision,
                    %refusal,
                    "the branch controller grant cannot be read now; the next pass revokes it"
                );
                return Cleanup::Pending;
            }
        };
        let Some(custody) = inner.controller_custody.upgrade() else {
            tracing::warn!(
                decision = %decision,
                "the runtime's custody is gone; the next pass revokes the branch controller grant"
            );
            return Cleanup::Pending;
        };
        match custody.revoke_controller(&inner.grants, &issuer, &reference) {
            Ok(()) => Cleanup::Done,
            Err(refusal) => {
                tracing::warn!(
                    decision = %decision,
                    %refusal,
                    "the branch controller grant cannot be revoked now; the next pass revokes it"
                );
                Cleanup::Pending
            }
        }
    }

    /// Undo what the execution of `decision` created ([`Self::undo`]), then
    /// settle the decision's cleanup fact. A decision that owes none (its
    /// execution never opened one) is done once both steps are.
    pub(crate) async fn clean_up(&self, decision: &DecisionId) -> Cleanup {
        if self.undo(decision).await == Cleanup::Pending {
            return Cleanup::Pending;
        }
        match self.0.decisions.settle_cleanup(decision) {
            Ok(_)
            | Err(DecisionError::Refused {
                refusal: DecisionRefusal::NotOwed,
                ..
            }) => Cleanup::Done,
            Err(error) => {
                tracing::warn!(
                    decision = %decision,
                    %error,
                    "the cleanup fact cannot be settled now; it settles when the decision \
                     is next cleaned up"
                );
                Cleanup::Pending
            }
        }
    }

    /// The approved work of the executed decision `decision`, in the branch
    /// its execution opened: seat, bind, enroll and admit it, then tell its
    /// origin it was approved, and watch it, so the branch is cleaned up
    /// when the work ends or the grant's window does. A redelivered notice
    /// admits nothing new: the admission is idempotent per decision (the
    /// approved work's input carries the decision's work id as its
    /// idempotency key), and a branch that already ended is never opened
    /// again. `None` to be asked again at a later pass.
    async fn admit_approved_work(
        &self,
        decision: &StoredDecision,
        request: &GrantChildRequest,
    ) -> Option<ApprovalConsentDeliveryState> {
        let inner = &self.0;
        let decision_id = &decision.decision_id;
        let grantee = &request.approval_branch_of;
        let approval = match self.issued_approval(decision_id) {
            Ok(Some(approval)) => approval,
            Ok(None) => {
                return self
                    .admission_refused(decision_id, request, "the approval grant was never issued")
                    .await;
            }
            Err(_) => return None,
        };
        // Only the branch the execution opened runs the approved work. Once
        // it ended (its work ran, or its window passed, before this notice's
        // receipt was recorded), the notice settles and nothing is forked.
        match inner.branches.open_branches().await {
            Ok(open) if open.contains(decision_id) => {}
            Ok(_) => {
                tracing::info!(
                    decision = %decision_id,
                    "the approval branch already ended: the notice settles and admits nothing"
                );
                return Some(ApprovalConsentDeliveryState::Failed {
                    reason: "the approval branch had already ended".to_owned(),
                });
            }
            Err(_) => return None,
        }
        let Some(branch) = approval_branch_principal(grantee, decision_id) else {
            return self
                .admission_refused(
                    decision_id,
                    request,
                    "the asking agent has no branch principal",
                )
                .await;
        };
        let Some(basis) = inner.branches.grantee_basis(grantee) else {
            return self
                .admission_refused(
                    decision_id,
                    request,
                    "the asking agent can no longer be branched",
                )
                .await;
        };
        let controller = match branch_controller_grant_id(decision_id)
            .map(|id| inner.grants.issued_reference(&id))
        {
            Some(Ok(Some(controller))) => controller,
            Some(Ok(None)) | None => {
                return self
                    .admission_refused(
                        decision_id,
                        request,
                        "the branch controller grant was never issued",
                    )
                    .await;
            }
            Some(Err(_)) => return None,
        };
        let expires_at_ms = match inner.grants.retained_grant(&approval) {
            Ok(retained) => match window_end(&retained.restrictions) {
                Some(end) => end,
                None => {
                    return self
                        .admission_refused(decision_id, request, "the approval grant has no window")
                        .await;
                }
            },
            Err(GrantRefusal::Unavailable) => return None,
            Err(_) => {
                return self
                    .admission_refused(decision_id, request, "the approval grant is not retained")
                    .await;
            }
        };
        if self.now_ms().is_some_and(|now| now >= expires_at_ms) {
            return self
                .admission_refused(
                    decision_id,
                    request,
                    "the approval grant's window ended before its work was admitted",
                )
                .await;
        }
        let seat = match inner.branches.seat(decision_id).await {
            Ok(seat) => seat,
            Err(ApprovalBranchError::Unavailable(_)) => return None,
            Err(_) => {
                return self
                    .admission_refused(
                        decision_id,
                        request,
                        "the approval branch cannot run the approved work",
                    )
                    .await;
            }
        };
        let mut approval_lineage = request.parent_lineage.clone();
        approval_lineage.push(approval);
        let mut controller_lineage = basis.controller_grant_lineage.clone();
        controller_lineage.push(controller);
        let bound = match ApprovalCarriedIngress::bind(
            decision_id,
            request,
            &branch,
            approval_lineage,
            controller_lineage,
            seat,
            &inner.decision_ingress,
        ) {
            Ok(bound) => bound,
            Err(error) => {
                tracing::error!(decision = %decision_id, %error, "approved work cannot be bound");
                return self
                    .admission_refused(decision_id, request, "the approved work cannot be bound")
                    .await;
            }
        };
        let custody = match inner.branches.enroll(&bound.association) {
            Ok(custody) => custody,
            Err(ApprovalBranchError::Unavailable(_)) => return None,
            Err(_) => {
                return self
                    .admission_refused(decision_id, request, "the approved work cannot be enrolled")
                    .await;
            }
        };
        let admission = match inner
            .branches
            .admit(decision_id, bound.input, custody)
            .await
        {
            Ok(admission) => admission,
            Err(ApprovalBranchError::Unavailable(_)) => return None,
            Err(_) => {
                return self
                    .admission_refused(
                        decision_id,
                        request,
                        "the approval branch refused the approved work",
                    )
                    .await;
            }
        };
        // The origin learns its request was approved once the work is
        // admitted, never before; nothing of the work reaches it.
        inner
            .replies
            .notify_origin(&request.origin, decision_id, AccessOutcome::Approved)
            .await?;
        let receipt = ApprovalConsentDeliveryState::Admitted {
            session_id: admission.session_id,
            input_id: admission.input_id,
        };
        // One watcher per decision in this process: a redelivered notice
        // admits nothing new and watches nothing twice.
        let watching = inner
            .watched
            .lock()
            .is_ok_and(|mut watched| watched.insert(decision_id.clone()));
        if watching {
            let watcher = self.clone();
            let decision_id = decision_id.clone();
            let ended = admission.ended;
            crate::tokio::spawn(async move {
                let left_ms = watcher
                    .now_ms()
                    .map_or(0, |now| expires_at_ms.saturating_sub(now));
                crate::tokio::select! {
                    () = ended => {}
                    () = crate::tokio::time::sleep(Duration::from_millis(left_ms)) => {}
                }
                let _ = watcher.clean_up(&decision_id).await;
                if let Ok(mut watched) = watcher.0.watched.lock() {
                    watched.remove(&decision_id);
                }
            });
        }
        Some(receipt)
    }

    /// The approved work of `decision_id` cannot be admitted for good: tell
    /// its origin the request was refused (no answer is coming), undo what
    /// the execution created (a step left now is the next pass's), and
    /// settle the notice failed.
    async fn admission_refused(
        &self,
        decision_id: &DecisionId,
        request: &GrantChildRequest,
        reason: &str,
    ) -> Option<ApprovalConsentDeliveryState> {
        self.0
            .replies
            .notify_origin(&request.origin, decision_id, AccessOutcome::Refused)
            .await?;
        let _ = self.clean_up(decision_id).await;
        Some(ApprovalConsentDeliveryState::Failed {
            reason: reason.to_owned(),
        })
    }

    /// Tell the origin of an ended decision that executed nothing how it
    /// ended, once what its execution created is undone: its notice settles
    /// after its cleanup, so a cleanup left now is asked for again with the
    /// notice.
    async fn notify_ended(
        &self,
        decision: &StoredDecision,
        request: &GrantChildRequest,
        outcome: AccessOutcome,
    ) -> Option<ApprovalConsentDeliveryState> {
        if self.clean_up(&decision.decision_id).await == Cleanup::Pending {
            return None;
        }
        self.0
            .replies
            .notify_origin(&request.origin, &decision.decision_id, outcome)
            .await
    }

    /// Whether the cleanup of `decision` is due, read from its decision
    /// (`branch_open`: whether the host lists its branch open, `None` when
    /// the host could not list its branches now).
    fn cleanup_due(&self, decision: &DecisionId, branch_open: Option<bool>) -> Due {
        match self.0.decisions.get(decision) {
            Ok(row) => match row.state.phase {
                // Its execution may be in flight, or run again.
                DecisionPhase::Open | DecisionPhase::Approved => Due::NotYet,
                // Nothing runs once its branch is gone, or its window passed.
                DecisionPhase::Executed => match branch_open {
                    Some(false) => Due::Now,
                    _ => self.window_passed(decision),
                },
                DecisionPhase::Rejected
                | DecisionPhase::Expired
                | DecisionPhase::Withdrawn
                | DecisionPhase::ExecutionRefused => Due::Now,
            },
            // A decision this binary cannot read (or that is gone) is
            // cleaned up once its approval grant's window must have passed.
            Err(DecisionError::Unreadable { .. } | DecisionError::NotFound(_)) => {
                self.window_passed(decision)
            }
            Err(error) => {
                tracing::warn!(
                    decision = %decision,
                    %error,
                    "the decision cannot be read now; its cleanup waits for the next pass"
                );
                Due::Unknown
            }
        }
    }

    /// Whether the window of the approval-carried grant of `decision` has
    /// passed, read from the grant owner: with no such grant issued, no
    /// work can run in the branch, and its cleanup is due now.
    fn window_passed(&self, decision: &DecisionId) -> Due {
        let approval = match self.issued_approval(decision) {
            Ok(Some(approval)) => approval,
            Ok(None) => return Due::Now,
            Err(_) => return Due::Unknown,
        };
        match self.0.grants.retained_grant(&approval) {
            Ok(retained) => match (window_end(&retained.restrictions), self.now_ms()) {
                (_, None) => Due::Unknown,
                (Some(end), Some(now)) if now < end => Due::NotYet,
                _ => Due::Now,
            },
            Err(GrantRefusal::Unavailable) => Due::Unknown,
            Err(_) => Due::Now,
        }
    }

    /// Core's sweep of the cleanup that is due, from durable state: the
    /// unrevoked branch controller grants, read from the grant owner's
    /// resident state (restored whole from its journal: no store is read
    /// and nothing is woken), and the branches the host lists open; for
    /// each, one read of its decision. A branch whose decision may still
    /// execute, or whose work may still run within its window, is kept; for
    /// every other one the branch is retired, its controller grant revoked
    /// and its decision's cleanup fact settled. Each step that cannot be
    /// taken now is left for the next pass. The loop runs it at each pass
    /// ([`DecisionExecutor::sweep`]).
    pub(crate) async fn sweep(&self) -> Vec<(DecisionId, DecisionPassOutcome)> {
        let inner = &self.0;
        let mut hits = BTreeSet::new();
        match inner
            .grants
            .unrevoked_grants(|id| decision_of_branch_controller(id).is_some())
        {
            Ok(grants) => hits.extend(
                grants
                    .iter()
                    .filter_map(|grant| decision_of_branch_controller(&grant.grant_id)),
            ),
            Err(refusal) => tracing::warn!(
                %refusal,
                "the grant owner cannot list the branch controller grants now"
            ),
        }
        let open = match inner.branches.open_branches().await {
            Ok(open) => Some(open.into_iter().collect::<BTreeSet<_>>()),
            Err(error) => {
                tracing::warn!(%error, "the host cannot list its approval branches now");
                None
            }
        };
        if let Some(open) = &open {
            hits.extend(open.iter().cloned());
        }
        let mut outcomes = Vec::new();
        for decision in hits {
            let branch_open = open.as_ref().map(|open| open.contains(&decision));
            let outcome = match self.cleanup_due(&decision, branch_open) {
                Due::NotYet => continue,
                Due::Unknown => DecisionPassOutcome::Deferred(DecisionDeferral::CleanupUnsettled),
                Due::Now => match self.clean_up(&decision).await {
                    Cleanup::Done => DecisionPassOutcome::CleanupSettled,
                    Cleanup::Pending => {
                        DecisionPassOutcome::Deferred(DecisionDeferral::CleanupUnsettled)
                    }
                },
            };
            outcomes.push((decision, outcome));
        }
        outcomes
    }

    /// Run Core's sweep now ([`Self::sweep`]), as the loop's pass does, and
    /// answer how many decisions it cleaned up. A host may call it, for
    /// example at start; the loop needs no host to call it.
    pub async fn sweep_branches(&self) -> usize {
        self.sweep()
            .await
            .iter()
            .filter(|(_, outcome)| *outcome == DecisionPassOutcome::CleanupSettled)
            .count()
    }
}

#[async_trait]
impl DecisionExecutor for GrantDecisionExecutor {
    fn kind(&self) -> &DecisionKind {
        &self.grants.0.kind
    }

    async fn execute(
        &self,
        decision: &StoredDecision,
        permit: &DecisionExecutionPermit,
    ) -> DecisionExecution {
        let inner = &self.grants.0;
        let Ok(request) = GrantChildRequest::of(decision) else {
            return refused("malformed_request");
        };
        let Some(approval) = approval_of(decision, permit, &request) else {
            return refused("not_a_human_decision");
        };
        let decision_id = &decision.decision_id;
        // A commit that already landed under this decision is its commit.
        match self.grants.issued_approval(decision_id) {
            Ok(Some(_)) => return DecisionExecution::Committed,
            Ok(None) => {}
            Err(_) => return DecisionExecution::Released,
        }
        let grantee = &request.approval_branch_of;
        let terms = &request.terms;
        let Some(parent_ref) = request.parent_lineage.last() else {
            return refused("malformed_request");
        };
        let parent = match inner.grants.retained_grant(parent_ref) {
            Ok(parent) => parent,
            Err(GrantRefusal::Unavailable) => return DecisionExecution::Released,
            Err(_) => return refused("parent_refused"),
        };
        // The offering grant must still resolve before anything is opened
        // or issued.
        match inner.grants.resolve_lineage(
            &request.parent_lineage,
            &terms.issuer,
            parent.represented_subject.as_ref(),
        ) {
            Ok(_) => {}
            Err(GrantRefusal::Unavailable) => return DecisionExecution::Released,
            Err(_) => return refused("parent_refused"),
        }
        // The effective terms of now must be the presented ones.
        let Some(now_ms) = self.grants.now_ms() else {
            return DecisionExecution::Released;
        };
        let requested = approval_child_request(now_ms, request.duration_ms);
        let Ok(derived) =
            DerivedChildRestrictions::new(parent.restrictions.clone(), requested.clone())
        else {
            return refused("parent_refused");
        };
        let now_terms = child_terms(
            &parent_ref.grant_id,
            &parent,
            &terms.issuer,
            &terms.grantee,
            derived.effective(),
            &request.operation,
            terms.uses,
            &terms.requester,
            &terms.work,
        );
        if now_terms.as_ref() != Some(terms) {
            return refused("terms_changed");
        }
        // The branch, for the one subject the offering grant acts for.
        let Some(branch) = approval_branch_principal(grantee, decision_id) else {
            return refused("approval_branch_unavailable");
        };
        if branch != terms.grantee {
            return refused("malformed_request");
        }
        let Some(basis) = inner.branches.grantee_basis(grantee) else {
            return refused("approval_branch_unavailable");
        };
        if basis.represented_subject != parent.represented_subject {
            return refused("approval_branch_unavailable");
        }
        let Some(expires_at_ms) = window_end(derived.effective()) else {
            return refused("terms_unbounded");
        };
        let Some(intent) = branch_controller_intent(decision_id, grantee, &basis, &branch) else {
            return DecisionExecution::Released;
        };
        // The decision owes the cleanup of what follows before any of it
        // exists.
        if let Err(error) = inner.decisions.owe_cleanup(permit) {
            tracing::warn!(
                decision = %decision_id,
                %error,
                "the cleanup fact cannot be opened now; the execution is tried again"
            );
            return DecisionExecution::Released;
        }
        match inner
            .branches
            .open(&ApprovalBranchRequest {
                decision: decision_id.clone(),
                grantee: grantee.clone(),
                branch: branch.clone(),
                expires_at_ms,
            })
            .await
        {
            Ok(()) => {}
            Err(ApprovalBranchError::Unavailable(_)) => return DecisionExecution::Released,
            Err(_) => {
                let _ = self.grants.undo(decision_id).await;
                return refused("approval_branch_unavailable");
            }
        }
        // The branch's controller grant: a child of the grantee's own,
        // recovered on retry.
        let validity = ChildGrantValidity {
            not_before_ms: 0,
            expires_at_ms: permit.deadline_at_ms(),
            deadline: meerkat_core::time_compat::Instant::now()
                + Duration::from_millis(permit.deadline_at_ms().saturating_sub(now_ms)),
        };
        match inner
            .grants
            .issue_or_recover_child_under_exclusion(&intent, || Ok(validity))
        {
            Ok(_) => {}
            Err(ChildGrantRefusal::Denied) => {
                let _ = self.grants.undo(decision_id).await;
                return refused("approval_branch_unavailable");
            }
            Err(_) => return DecisionExecution::Released,
        }
        // The approval-carried grant, before the permit's deadline.
        let Some(id) = approval_grant_id(decision_id) else {
            return DecisionExecution::Released;
        };
        if !self
            .grants
            .now_ms()
            .is_some_and(|now| permit.admits_commit_at(now))
        {
            return DecisionExecution::Released;
        }
        match inner.grants.issue_approval_child(
            &terms.issuer,
            parent_ref,
            id,
            terms.grantee.clone(),
            requested,
            approval,
        ) {
            Ok(_) => DecisionExecution::Committed,
            Err(ApprovalChildRefusal::Unavailable) => DecisionExecution::Released,
            Err(ApprovalChildRefusal::AlreadyIssued | ApprovalChildRefusal::DecisionCarried) => {
                match self.grants.issued_approval(decision_id) {
                    Ok(Some(_)) => DecisionExecution::Committed,
                    Ok(None) => refused("decision_carried"),
                    Err(_) => DecisionExecution::Released,
                }
            }
            Err(refusal) => {
                let _ = self.grants.undo(decision_id).await;
                refused(match refusal {
                    ApprovalChildRefusal::BelowRule => "assurance_below_rule",
                    ApprovalChildRefusal::TermsDiffer => "terms_changed",
                    ApprovalChildRefusal::Unbounded => "terms_unbounded",
                    ApprovalChildRefusal::NoChildRule => "no_child_approval_rule",
                    _ => "parent_refused",
                })
            }
        }
    }

    async fn committed(
        &self,
        decision: &StoredDecision,
    ) -> Result<bool, DecisionExecutorRecordUnavailable> {
        self.grants
            .issued_approval(&decision.decision_id)
            .map(|issued| issued.is_some())
            .map_err(|refusal| DecisionExecutorRecordUnavailable(refusal.to_string()))
    }

    async fn deliver_notice(
        &self,
        decision: &StoredDecision,
    ) -> Option<ApprovalConsentDeliveryState> {
        let Ok(request) = GrantChildRequest::of(decision) else {
            return Some(ApprovalConsentDeliveryState::Failed {
                reason: "not a readable grant.child request".to_owned(),
            });
        };
        let outcome = match decision.state.phase {
            DecisionPhase::Executed => {
                return self.grants.admit_approved_work(decision, &request).await;
            }
            DecisionPhase::Rejected => AccessOutcome::Declined,
            DecisionPhase::Expired => AccessOutcome::Expired,
            DecisionPhase::Withdrawn => AccessOutcome::Withdrawn,
            DecisionPhase::ExecutionRefused => AccessOutcome::Refused,
            DecisionPhase::Open | DecisionPhase::Approved => return None,
        };
        self.grants.notify_ended(decision, &request, outcome).await
    }

    async fn sweep(&self) -> Vec<(DecisionId, DecisionPassOutcome)> {
        self.grants.sweep().await
    }
}
