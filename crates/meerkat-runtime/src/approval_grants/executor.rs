//! [`GrantDecisionExecutor`]: the executor of `grant.child` decisions.

use std::time::Duration;

use async_trait::async_trait;
use meerkat_authorization::grants::{
    ApprovalChildRefusal, ChildGrantRefusal, ChildGrantValidity, GrantApproval,
    GrantDecisionAssurance, GrantDecisionTier, GrantPrincipal, GrantRefusal,
};
use meerkat_authorization_contracts::constraints::LifetimeBound;
use meerkat_authorization_contracts::derived_child::DerivedChildRestrictions;
use meerkat_authorization_contracts::work_association::GrantLineageRef;
use meerkat_core::PrincipalRef;
use meerkat_core::approval::decision::{
    ConsentProvenance, DecisionEvidence, DecisionId, DecisionKind, DecisionPhase,
    DecisionReference, DecisionTier, StoredDecision,
};
use meerkat_core::approval::{ApprovalAssurance, ApprovalConsentDeliveryState};

use super::ApprovalGrants;
use super::body::{
    GrantChildRequest, approval_branch_principal, approval_grant_id, branch_controller_grant_id,
};
use super::ingress::ApprovalCarriedIngress;
use super::offer::{approval_child_request, branch_controller_intent, child_terms};
use super::seams::{AccessOutcome, ApprovalBranchError, ApprovalBranchRequest};
use crate::decision::{
    DecisionExecution, DecisionExecutionPermit, DecisionExecutor, DecisionExecutorRecordUnavailable,
};

/// How many times a retirement or a revocation is tried before it is left.
const RETIRE_ATTEMPTS: u32 = 8;

/// The executor of `grant.child` decisions (2.a.2). Register it only in the
/// runtime that holds the grant owner's single-writer custody.
///
/// It executes an approved offer at most once: the decider's provenance
/// must be a person's; the effective terms of now must equal the presented
/// ones (`terms_changed` otherwise); it opens the approval branch (idempotent
/// per decision), issues the branch's controller grant (recovered on
/// retry), then the approval-carried grant, whose id derives from the
/// decision. A commit landed exactly when that grant exists. The decision's
/// notice admits the approved work into the branch and tells the origin it
/// was approved; an ended decision that executed nothing retires any branch
/// it opened and tells the origin how it ended, with no content.
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

impl ApprovalGrants {
    /// The issued approval-carried grant of `decision`, when one landed.
    fn issued_approval(
        &self,
        decision: &DecisionId,
    ) -> Result<Option<GrantLineageRef>, GrantRefusal> {
        let id = approval_grant_id(decision).ok_or(GrantRefusal::Denied)?;
        self.0.grants.issued_reference(&id)
    }

    /// Retire the approval branch of `decision` of `grantee`, then revoke
    /// its controller grant once nothing runs under it. Both are retried a
    /// bounded number of times; a retired or never opened branch, and a
    /// controller grant never issued or already revoked, are done.
    pub(crate) async fn retire_branch(&self, decision: &DecisionId, grantee: &PrincipalRef) {
        let inner = &self.0;
        for attempt in 0..RETIRE_ATTEMPTS {
            match inner.branches.retire(decision).await {
                Ok(()) => break,
                Err(ApprovalBranchError::Refused(_)) => break,
                Err(_) => backoff(attempt).await,
            }
        }
        let Some(id) = branch_controller_grant_id(decision) else {
            return;
        };
        let Ok(Some(reference)) = inner.grants.issued_reference(&id) else {
            return;
        };
        for attempt in 0..RETIRE_ATTEMPTS {
            let Some(custody) = inner.controller_custody.upgrade() else {
                return;
            };
            match custody.revoke_controller(&inner.grants, grantee, &reference) {
                Ok(()) | Err(GrantRefusal::Denied) => return,
                Err(_) => {
                    drop(custody);
                    backoff(attempt).await;
                }
            }
        }
        tracing::warn!(
            decision = %decision,
            "approval branch controller grant left unrevoked after its branch retired"
        );
    }

    /// The approved work of the executed decision `decision`: notify its
    /// origin, open (or find) its branch, bind, enroll and admit it, and
    /// watch it, so the branch is retired when the work ends or the grant's
    /// window does. `None` to be asked again at a later pass.
    async fn admit_approved_work(
        &self,
        decision: &StoredDecision,
        request: &GrantChildRequest,
    ) -> Option<ApprovalConsentDeliveryState> {
        let inner = &self.0;
        let failed = |reason: &str| {
            Some(ApprovalConsentDeliveryState::Failed {
                reason: reason.to_owned(),
            })
        };
        let decision_id = &decision.decision_id;
        let grantee = &request.approval_branch_of;
        let approval = match self.issued_approval(decision_id) {
            Ok(Some(approval)) => approval,
            Ok(None) => return failed("the approval grant was never issued"),
            Err(_) => return None,
        };
        let Some(branch) = approval_branch_principal(grantee, decision_id) else {
            return failed("the asking agent has no branch principal");
        };
        let Some(basis) = inner.branches.grantee_basis(grantee) else {
            self.retire_branch(decision_id, grantee).await;
            return failed("the asking agent can no longer be branched");
        };
        let controller = match branch_controller_grant_id(decision_id)
            .map(|id| inner.grants.issued_reference(&id))
        {
            Some(Ok(Some(controller))) => controller,
            Some(Ok(None)) | None => return failed("the branch controller grant was never issued"),
            Some(Err(_)) => return None,
        };
        let expires_at_ms = match inner.grants.retained_grant(&approval) {
            Ok(retained) => match window_end(&retained.restrictions) {
                Some(end) => end,
                None => return failed("the approval grant has no window"),
            },
            Err(GrantRefusal::Unavailable) => return None,
            Err(_) => return failed("the approval grant is not retained"),
        };
        if self.now_ms().is_some_and(|now| now >= expires_at_ms) {
            self.retire_branch(decision_id, grantee).await;
            return failed("the approval grant's window ended before its work was admitted");
        }
        // The origin learns first that its request was approved and where
        // the answer goes: nothing of the work reaches it.
        inner
            .replies
            .notify_origin(&request.origin, decision_id, AccessOutcome::Approved)
            .await?;
        let seat = match inner
            .branches
            .open(&ApprovalBranchRequest {
                decision: decision_id.clone(),
                grantee: grantee.clone(),
                branch: branch.clone(),
                expires_at_ms,
            })
            .await
        {
            Ok(()) => inner.branches.seat(decision_id).await,
            Err(error) => Err(error),
        };
        let seat = match seat {
            Ok(seat) => seat,
            Err(ApprovalBranchError::Unavailable(_)) => return None,
            Err(_) => {
                self.retire_branch(decision_id, grantee).await;
                return failed("the approval branch cannot be opened");
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
                self.retire_branch(decision_id, grantee).await;
                return failed("the approved work cannot be bound");
            }
        };
        let custody = match inner.branches.enroll(&bound.association) {
            Ok(custody) => custody,
            Err(ApprovalBranchError::Unavailable(_)) => return None,
            Err(_) => {
                self.retire_branch(decision_id, grantee).await;
                return failed("the approved work cannot be enrolled");
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
                self.retire_branch(decision_id, grantee).await;
                return failed("the approval branch refused the approved work");
            }
        };
        let receipt = ApprovalConsentDeliveryState::Admitted {
            session_id: admission.session_id,
            input_id: admission.input_id,
        };
        let watcher = self.clone();
        let decision_id = decision_id.clone();
        let grantee = grantee.clone();
        let ended = admission.ended;
        crate::tokio::spawn(async move {
            let left_ms = watcher
                .now_ms()
                .map_or(0, |now| expires_at_ms.saturating_sub(now));
            crate::tokio::select! {
                () = ended => {}
                () = crate::tokio::time::sleep(Duration::from_millis(left_ms)) => {}
            }
            watcher.retire_branch(&decision_id, &grantee).await;
        });
        Some(receipt)
    }

    /// Tell the origin of an ended decision that executed nothing how it
    /// ended, after retiring any branch it opened.
    async fn notify_ended(
        &self,
        decision: &StoredDecision,
        request: &GrantChildRequest,
        outcome: AccessOutcome,
    ) -> Option<ApprovalConsentDeliveryState> {
        self.retire_branch(&decision.decision_id, &request.approval_branch_of)
            .await;
        self.0
            .replies
            .notify_origin(&request.origin, &decision.decision_id, outcome)
            .await
    }

    /// Retire every open approval branch whose approval grant's window has
    /// ended, or whose decision ended without one, and revoke its controller
    /// grant. The host calls it at start and on its decision timer: a branch
    /// whose watcher ended with its process is retired by the end of its
    /// window at the latest. Answers how many branches it retired.
    pub async fn sweep_branches(&self) -> usize {
        let Ok(open) = self.0.branches.open_branches().await else {
            return 0;
        };
        let mut retired = 0;
        for decision_id in open {
            let Ok(decision) = self.0.decisions.get(&decision_id) else {
                continue;
            };
            let Ok(request) = GrantChildRequest::of(&decision) else {
                continue;
            };
            let ended = match decision.state.phase {
                // Its execution may be in flight.
                DecisionPhase::Open | DecisionPhase::Approved => false,
                DecisionPhase::Executed => match self.issued_approval(&decision_id) {
                    Ok(Some(approval)) => match self.0.grants.retained_grant(&approval) {
                        Ok(retained) => window_end(&retained.restrictions)
                            .is_none_or(|end| self.now_ms().is_some_and(|now| now >= end)),
                        Err(_) => false,
                    },
                    Ok(None) => true,
                    Err(_) => false,
                },
                DecisionPhase::Rejected
                | DecisionPhase::Expired
                | DecisionPhase::Withdrawn
                | DecisionPhase::ExecutionRefused => true,
            };
            if ended {
                self.retire_branch(&decision_id, &request.approval_branch_of)
                    .await;
                retired += 1;
            }
        }
        retired
    }
}

async fn backoff(attempt: u32) {
    let millis = 50u64.saturating_mul(1u64 << attempt.min(6));
    crate::tokio::time::sleep(Duration::from_millis(millis)).await;
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
            Ok(_) => {}
            Err(ApprovalBranchError::Unavailable(_)) => return DecisionExecution::Released,
            Err(_) => {
                self.grants.retire_branch(decision_id, grantee).await;
                return refused("approval_branch_unavailable");
            }
        }
        // The branch's controller grant: a child of the grantee's own,
        // recovered on retry.
        let Some(intent) = branch_controller_intent(decision_id, grantee, &basis, &branch) else {
            return DecisionExecution::Released;
        };
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
                self.grants.retire_branch(decision_id, grantee).await;
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
                self.grants.retire_branch(decision_id, grantee).await;
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
}
