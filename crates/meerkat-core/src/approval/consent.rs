//! Human consent for one retained R3 candidate (ADR-001).
//!
//! With a host human route composed
//! ([`super::review::BoundOperationReview::with_human_consent`]), R3
//! admission (or an R2 escalation or expired deadline the route takes over)
//! issues ONE consent approval for the exact retained candidate. The
//! generated approval owner creates it linked to its process-local human
//! attempt under one reservation; the dispatch then settles locally: nothing
//! parks and permitted siblings continue. An authenticated decision whose
//! eligibility evidence is still current at commitment only records the
//! decision and delivers a notice; it never executes anything. A later fresh
//! explicit attempt of the SAME retained candidate, inside its original
//! admitted work and run, consumes the decision once at the physical entry:
//! the generated owner's linked spend, conditional on the stored row
//! ([`super::ApprovalStore::put_if_current`]).
//!
//! Spendable consent is the linked attempt's generated `Allowed` status. It
//! is memory-only, never persisted or restored: a stored consent row is
//! history, never an executable capability, so a restart requires fresh
//! consent. Custody is single-process by construction: only the owner holding
//! the retained candidate can decide or spend it. A conflicting or uncertain
//! store commit retires that custody and the row is reconciled from the
//! store before reuse. When the admitting run closes, its unspent consent
//! whose notices have an exact native destination is parked for a resumed
//! run of its origin work (custody kept, nothing spends it); the rest is
//! withdrawn. Parked consent moves into the resumed run its approved
//! notice's admitted input is staged with, whose boundaries attempt it
//! once; when that input ends without a run taking the consent over, the
//! delivery composition reads its native terminal and the consent is
//! withdrawn, its cause naming the input.
//!
//! An owner publication change makes the retained decision stale without
//! saying whether it concerns this candidate, and the decision's own owner
//! may bound it in time (its validity window for the decision). Neither
//! withdraws the consent by itself (ADR-001 D9): the candidate's own owners
//! decide the same binding afresh outside every owner lock, and their answer
//! is taken as is. The consent stands while that decision keeps the binding,
//! the review tier and the material the approval presented; their refusal,
//! or a change of any of these, withdraws it. Approval reads, a decision and
//! its publication apply one shared rule under the owner's reservation, an
//! attempt is handed a decision decided afresh, and its spend never enters
//! on a decision that is not current. The approver's eligibility evidence
//! is kept with the published allowance and checked by the spend: when it
//! went stale, the eligibility owner decides afresh, and only its refusal
//! withdraws the consent.
//!
//! Not covered here: owner-attributed presentation of the native review
//! context, a native approver-eligibility owner, remote authenticated
//! decision ingress, delivery and wake of the owning session (the notice is a
//! host projection seam), closed batches, standing consent and policy edits.

use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};

#[cfg(target_arch = "wasm32")]
use crate::tokio;

use crate::ToolCallView;
use crate::authorization::{
    OperationAuthorizationError, OperationRefusalKind, PreparedOperationCheck,
    ReviewOperationContextResolution, WorkAuthorizationContext,
};
use crate::lifecycle::identifiers::RunId;
use crate::{PrincipalRef, SurfaceMetadata, ToolCallId};

use super::review::{
    EntryRefusal, FinalCheckFailure, OperationReviewObserver, ReviewAttemptHandle,
    ReviewAttemptStatus, ReviewObservation, ReviewRetirementReason, ReviewUnavailableKind,
    ReviewUnsatisfiedKind, retained_current_failure,
};
use crate::generated::approval_lifecycle::{
    ApprovalLifecycleOutcome, ApprovalLifecycleRejectionReason, ApprovalLifecycleStatus,
};

use super::{
    ApprovalActionKind, ApprovalAssurance, ApprovalConsentApprover, ApprovalConsentCustody,
    ApprovalConsentDeliveryState, ApprovalConsentOrigin, ApprovalConsentRecord,
    ApprovalConsentTransition, ApprovalConsentWithdrawal, ApprovalDecision, ApprovalDecisionRecord,
    ApprovalError, ApprovalId, ApprovalOwnerRef, ApprovalPrincipalId, ApprovalProposedAction,
    ApprovalRecord, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind, ApprovalResourceRef,
    ApprovalRisk, ApprovalService, ApprovalServiceState, ApprovalStatus, QueuedDisposalReport,
    QueuedReviewDisposal, QueuedReviewInput, SettledReports, allowed_decision_flags,
    approval_lifecycle_decision, lifecycle_error, lifecycle_status_from_outcome,
    review_status_from_outcome,
};

/// A human approver the host's ingress owner authenticated on the actual
/// decision event, for example a person-authenticated console session or a
/// verified channel update.
///
/// Constructing one is that trusted host adapter's statement. It is not
/// `Deserialize`, and no wire surface builds it from caller text: an actor
/// string, display name, quoted approval, forwarded message or agent-relayed
/// claim is not authentication. Eligibility to decide a particular approval
/// is a separate, current host policy check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedApprover {
    principal: PrincipalRef,
    assurance: ApprovalAssurance,
    authentication_scope: Option<HostAuthenticationScope>,
}

/// Opaque reference to the host authentication scope of one decision event:
/// the exact login or session the actual event was authenticated in. Two
/// logins of one person are different scopes, so the host's eligibility
/// owner can refuse an event from a revoked login while another login of
/// the same person stays live. It creates no permission, and it is not
/// `Deserialize`: only the trusted host adapter that authenticated the event
/// names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HostAuthenticationScope(Arc<str>);

impl HostAuthenticationScope {
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AuthenticatedApprover {
    /// The principal must be a human naming an explicit trust domain: R3 is
    /// a qualified human decision. A delegated service may carry the event
    /// (`ApprovalAssurance::ServiceCredential`), never decide it.
    pub fn new(
        principal: PrincipalRef,
        assurance: ApprovalAssurance,
    ) -> Result<Self, ApprovalError> {
        if principal.kind != crate::PrincipalKind::Human {
            return Err(ApprovalError::InvalidPrincipal);
        }
        principal
            .validate_qualified()
            .map_err(|_| ApprovalError::InvalidPrincipal)?;
        Ok(Self {
            principal,
            assurance,
            authentication_scope: None,
        })
    }

    /// The exact host authentication scope (login or session) of the event
    /// this approver was authenticated on. The host route's eligibility owner
    /// receives it with the approver and resolves currentness for it.
    #[must_use]
    pub fn with_authentication_scope(mut self, scope: HostAuthenticationScope) -> Self {
        self.authentication_scope = Some(scope);
        self
    }

    #[must_use]
    pub fn principal(&self) -> &PrincipalRef {
        &self.principal
    }

    #[must_use]
    pub fn assurance(&self) -> ApprovalAssurance {
        self.assurance
    }

    /// The host authentication scope of the decision event, when the host
    /// adapter named one.
    #[must_use]
    pub fn authentication_scope(&self) -> Option<&HostAuthenticationScope> {
        self.authentication_scope.as_ref()
    }
}

/// One authenticated human decision on a consent approval.
#[derive(Debug, Clone)]
pub struct AuthenticatedConsentDecision {
    pub approver: AuthenticatedApprover,
    pub decision: ApprovalDecision,
    pub reason: Option<String>,
}

/// One authenticated human decision event, bound by the trusted host adapter
/// to the exact approval and decision the human made on it. Not
/// `Deserialize`: a transport builds it only after authenticating the actual
/// event, never from a request body, and a host-only dispatch entry is the
/// only way it reaches the approval owner. Plain request dispatch cannot
/// decide native consent.
#[derive(Debug, Clone)]
pub struct AuthenticatedConsentEvent {
    approval_id: ApprovalId,
    decision: AuthenticatedConsentDecision,
}

impl AuthenticatedConsentEvent {
    pub fn new(approval_id: ApprovalId, decision: AuthenticatedConsentDecision) -> Self {
        Self {
            approval_id,
            decision,
        }
    }

    #[must_use]
    pub fn approval_id(&self) -> &ApprovalId {
        &self.approval_id
    }

    #[must_use]
    pub fn decision(&self) -> &AuthenticatedConsentDecision {
        &self.decision
    }
}

/// The exact native candidate a human approval is requested for. Unlike a
/// model review candidate it carries no review attempt attribution or work
/// owner: presentation is host-local and can neither read review context nor
/// start a governed operation on the candidate's behalf.
pub struct ConsentPresentationCandidate<'a> {
    pub(super) call: ToolCallView<'a>,
    pub(super) check: &'a PreparedOperationCheck,
    pub(super) operation_context: Option<&'a crate::authorization::ReviewOperationContext>,
}

impl<'a> ConsentPresentationCandidate<'a> {
    pub fn tool_name(&self) -> &'a str {
        self.call.name
    }

    pub fn tool_call_id(&self) -> &'a str {
        self.call.id
    }

    pub fn arguments(&self) -> &'a serde_json::value::RawValue {
        self.call.args
    }

    pub fn facts(&self) -> &'a crate::authorization::OperationAuthorizationFacts {
        self.check.binding().facts()
    }

    /// The owners' passive material for this operation (selected action
    /// account, current mandate), read once before presentation. `None` when
    /// the owners stated it is not applicable. It grants nothing.
    pub fn operation_context(&self) -> Option<&'a crate::authorization::ReviewOperationContext> {
        self.operation_context
    }

    /// Historical attribution of the preparation awaiting consent. Never a
    /// currentness proof; staleness is decided only by current owners.
    pub fn policy_observation(&self) -> Option<crate::authorization::PolicyPublicationObservation> {
        self.check.policy_observation()
    }
}

impl fmt::Debug for ConsentPresentationCandidate<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConsentPresentationCandidate([REDACTED])")
    }
}

/// Trusted host route for R3 human consent. Host code: the owner never calls
/// it while holding its own locks, and nothing it returns authorizes entry.
pub trait HumanConsentRoute: Send + Sync {
    /// Presentation facts for the approval of one exact candidate. The owner
    /// itself supplies the exact action, the binding-owned arguments, the
    /// owner reference, the allowed decisions and the expiry.
    fn present(
        &self,
        candidate: &ConsentPresentationCandidate<'_>,
    ) -> Result<ConsentPresentation, ConsentRouteUnavailable>;

    /// Resolve, from the host's approver-eligibility owner, whether this
    /// authenticated approver may decide this approval now. Visibility of
    /// the approval, or membership in the channel that shows it, is not
    /// eligibility. Called outside every approval owner lock; the returned
    /// evidence is checked again when the decision commits, when its
    /// allowance is published and when the consent is spent, so it must be
    /// that owner's coherent publication of the exact principal and
    /// approval, never a boolean computed now.
    ///
    /// Evidence that went stale after the decision (an owner change, related
    /// or not) is resolved again for the same approver and the approval as
    /// decided (its record carries this approver's decision), on the
    /// publishing or spending caller's thread (ADR-001 D9). The question is
    /// then only whether this approver was revoked, or whether policy came
    /// to forbid them, since deciding: a natural expiry of the login they
    /// decided in is no revocation (the consent keeps its own expiry), and
    /// only a refusal withdraws the approved consent. See
    /// [`Self::eligibility_answer`] for an owner that cannot say now.
    fn eligibility(
        &self,
        approver: &AuthenticatedApprover,
        approval: &ApprovalRecord,
    ) -> Result<Arc<dyn ConsentEligibility>, ConsentIneligible>;

    /// [`Self::eligibility`] that tells a refusal apart from an owner that
    /// cannot say now: the approval owner asks this one, at a decision, at
    /// its publication and before a spend. An answer of
    /// [`ConsentEligibilityAnswer::Unsettled`] decides nothing: a decision
    /// records nothing and answers a retryable `StoreUnavailable`, a
    /// publication asks again within its bound and then publishes as it is,
    /// and a spend enters nothing and keeps the consent (`owner_unavailable`).
    /// The default maps [`Self::eligibility`]: evidence is `Eligible`, its
    /// error `Refused`. A route whose owner can be mid-change (an owner
    /// change in progress, or one landing during the read) overrides it.
    fn eligibility_answer(
        &self,
        approver: &AuthenticatedApprover,
        approval: &ApprovalRecord,
    ) -> ConsentEligibilityAnswer {
        match self.eligibility(approver, approval) {
            Ok(evidence) => ConsentEligibilityAnswer::Eligible(evidence),
            Err(ConsentIneligible) => ConsentEligibilityAnswer::Refused,
        }
    }

    /// Projection of one recorded consent transition, delivered after the
    /// owner released its locks. It never executes the retained action.
    fn notice(&self, notice: ConsentNotice);
}

/// An eligibility owner's evidence that one approver may decide one
/// approval, as of a coherent publication of its facts.
pub trait ConsentEligibility: Send + Sync {
    /// Whether that publication is still current. Runs under the approval
    /// owner's commit reservation, so it has the same contract as
    /// [`crate::authorization::PreparedOperationAuthorization::check_current`]:
    /// no I/O, allocation, observation, callback or lock.
    fn check_current(&self) -> Result<(), ConsentIneligible>;
}

/// An eligibility owner's answer for one approver and approval
/// ([`HumanConsentRoute::eligibility_answer`]).
#[non_exhaustive]
pub enum ConsentEligibilityAnswer {
    /// Evidence that the approver may decide the approval (for an approval
    /// this approver already decided: that they were not revoked and policy
    /// does not forbid them since), current as of a coherent publication of
    /// the owner's facts.
    Eligible(Arc<dyn ConsentEligibility>),
    /// The approver may not: not eligible, revoked, or forbidden by policy.
    Refused,
    /// The owner cannot say now (an owner change in progress, or one that
    /// landed during the read): nothing is decided, and it is asked again.
    Unsettled,
}

impl fmt::Debug for ConsentEligibilityAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Eligible(_) => "Eligible",
            Self::Refused => "Refused",
            Self::Unsettled => "Unsettled",
        })
    }
}

/// The approver is not, or is no longer, eligible to decide the approval (or
/// the eligibility owner cannot currently say, which
/// [`HumanConsentRoute::eligibility_answer`] tells apart).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("approver is not currently eligible")]
pub struct ConsentIneligible;

/// Host presentation facts for one consent approval.
#[derive(Debug, Clone)]
pub struct ConsentPresentation {
    pub requester: ApprovalPrincipalId,
    pub risk: ApprovalRisk,
    pub metadata: SurfaceMetadata,
}

/// The host route cannot present the candidate readably. No approval is
/// issued: an approver cannot consent to what they cannot inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("human consent route unavailable")]
pub struct ConsentRouteUnavailable;

/// Audience-safe projection of one consent transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentNotice {
    pub approval_id: ApprovalId,
    pub tool_call_id: Arc<str>,
    pub kind: ConsentNoticeKind,
}

/// Recorded consent transition kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentNoticeKind {
    /// A consent approval was issued for the retained candidate.
    Requested,
    /// An authenticated decision approved it. Nothing executed: a fresh
    /// explicit attempt is still required.
    Approved,
    /// An authenticated decision declined it.
    Denied,
    /// A fresh attempt consumed it for its physical entry; delivered after
    /// the effect returned, or with the refusal when a final check after the
    /// committed spend refused the entry (zero effect, no refund). Not proof
    /// that the effect succeeded.
    Used,
    /// The owner withdrew the unspent consent (its run closed, its context
    /// or authority changed, or it expired), after recording that durably.
    /// Not a denial; nothing executed.
    Withdrawn,
}

/// Host human-consent composition of a review owner: its route and the
/// finite validity of each consent request.
pub(crate) struct HumanConsent {
    pub(crate) route: Arc<dyn HumanConsentRoute>,
    pub(crate) validity: std::time::Duration,
}

/// Process-local custody of one retained R3 candidate: its human attempt
/// (bound to the exact operation and work by identity), the exact decision
/// it holds, the owner material its decision is compared against, the
/// approver of its published allowance, and the validity deadline. Never
/// persisted.
pub(crate) struct ConsentCandidate {
    attempt: ReviewAttemptHandle,
    /// The exact decision this custody holds: the admitted one, or a
    /// decision of the same binding and work that its owners decided afresh
    /// (after an owner publication change, or once the held decision's own
    /// check refused it) and found unchanged (ADR-001 D9; see
    /// [`Self::readjudicate`]). Replaced only under the owner's reservation
    /// and only by such a decision, current then, so every decision one
    /// custody ever holds keeps its binding, review tier and material.
    retained: parking_lot::Mutex<PreparedOperationCheck>,
    /// The owners' review material for this custody's decision: what the
    /// approval presented, read once for the request, or, once a resumed
    /// run took the custody over, what that run's own owners read for it
    /// then (the same action account; only the mandate may differ). A
    /// re-adjudication keeps the consent only while the owners read exactly
    /// this.
    material: ReviewOperationContextResolution,
    /// The approver of the published allowance and the eligibility owner's
    /// evidence for them, which the spend checks (ADR-001 D9). Set when the
    /// allowance is published, and replaced only under the owner's
    /// reservation, by fresh evidence for the same approver.
    approval: parking_lot::Mutex<Option<ConsentApproval>>,
    deadline: tokio::time::Instant,
    tool_call_id: Arc<str>,
    route: Arc<dyn HumanConsentRoute>,
    observer: Option<Arc<dyn OperationReviewObserver>>,
    /// The runtime owner's hosting claim lineage for the admitting run's
    /// session, held exactly as long as this custody: the session's claim is
    /// never released while the candidate can still be decided or spent, so
    /// a later grant that excluded every earlier holder proves this custody
    /// is gone. Absent when the admitting work carried none.
    hosting: Option<crate::session_hosting::HostingClaim>,
}

/// The authenticated approver of a published allowance, the approval as they
/// decided it (the record the eligibility owner is asked about after the
/// decision), and the eligibility owner's evidence for them.
#[derive(Clone)]
struct ConsentApproval {
    approver: AuthenticatedApprover,
    record: ApprovalRecord,
    eligibility: Arc<dyn ConsentEligibility>,
}

/// What the eligibility owner answered when the evidence a published
/// allowance holds went stale and the spend asked it afresh, outside every
/// lock (ADR-001 D9).
pub(crate) enum EligibilityRecheck {
    /// Fresh evidence that the approver is still eligible.
    Fresh(Arc<dyn ConsentEligibility>),
    /// The eligibility owner refuses the approver now: revoked, or
    /// forbidden by policy since the decision.
    Refused,
    /// The eligibility owner cannot say now: nothing is decided.
    Unsettled,
}

/// What a spend's attempt decided afresh before its reservation, outside
/// every lock (ADR-001 D9).
#[derive(Default)]
pub(crate) struct SpendRechecks {
    /// The re-adjudication of its entering decision, when that is its
    /// staging's re-preparation of the held one.
    pub(crate) readjudication: Option<Readjudication>,
    /// The eligibility owner's fresh answer, when the evidence the
    /// published allowance holds went stale.
    pub(crate) eligibility: Option<EligibilityRecheck>,
}

impl fmt::Debug for ConsentCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConsentCandidate([REDACTED])")
    }
}

/// Re-adjudications of one retained decision that one owner step makes
/// while the owner publication keeps moving under them (ADR-001 D9): as many
/// as the free rebuilds of one operation after an owner change. Past them
/// nothing is decided, and nothing is withdrawn for the bound alone.
pub(crate) const CONSENT_READJUDICATION_BOUND: u8 =
    crate::authorization::OWNER_CHANGE_FREE_REBUILDS;

/// What re-adjudicating a retained decision that is no longer current found,
/// outside every owner lock (ADR-001 D9): one stale because the owner
/// publication moved, or one its own check refuses (its owner's validity
/// window for it ended, for example).
#[derive(Debug)]
pub(crate) enum Readjudication {
    /// A decision of the same binding and work, at the same review tier,
    /// whose owners read the material this custody's decision is compared
    /// against: the consent stands for it once it is current under the
    /// owner's reservation. Its own validity window is its owner's answer,
    /// checked when it is held and at the spend; the consent's own expiry
    /// stays separate.
    Unchanged(PreparedOperationCheck),
    /// A related change: the owners decided the operation at another review
    /// tier, for another operation or work, no longer read that material, or
    /// state facts they cannot read. The consent is withdrawn for this
    /// reason.
    Changed(ReviewRetirementReason),
    /// The owners, asked afresh, refuse the operation now (a revoked
    /// permission, or an owner deadline that passed), or refuse to present
    /// its review material: the consent is withdrawn as `AuthorityChanged`,
    /// with this refusal.
    Refused(OperationAuthorizationError),
    /// Another completed owner change met the re-adjudication: it is made
    /// again, within [`CONSENT_READJUDICATION_BOUND`].
    Moved,
    /// Nothing can be decided now: an owner change is still in progress, or
    /// the owners are unavailable. Nothing changes, and nothing enters on
    /// the decision that is not current.
    Unsettled,
}

/// What the shared currentness rule does with a held decision that its own
/// check refuses, rather than finds stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeldRefusal {
    /// Have its owners decide it afresh, like a stale one: approval reads, a
    /// decision and its publication (its validity window may have ended
    /// while the human took their time).
    Readjudicate,
    /// Withdraw it now as `AuthorityChanged`: the spend, whose entering
    /// decision its staging found current moments before.
    Withdraw,
}

/// Which owner step meets a held decision that is no longer current, and so
/// which of its failures have its owners decide it afresh (ADR-001 D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadjudicationStep {
    /// An approval read: only a decision the owner publication made stale is
    /// decided afresh. One its own check refuses (its owner's validity
    /// window for it ended, for example) is left for a decision or an
    /// attempt to decide afresh, so a polling read never calls its owners
    /// for it.
    Read,
    /// An approval, its publication or an attempt: a decision the owner
    /// publication made stale, or one its own check refuses, is decided
    /// afresh.
    Decision,
}

/// What `step` does with a held decision before anything calls its owners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeldCheck {
    /// It is current: nothing to decide.
    Current,
    /// It is not current, and its owners decide it afresh.
    Readjudicate,
    /// It is not current, and nothing calls its owners now: its own check
    /// answers that they are unavailable (asking them afresh would only add
    /// their audit rows), or a read meets a decision its own check refuses.
    Leave,
}

/// The held decision `retained`'s own callback-free check, for `step`.
fn held_check(retained: &PreparedOperationCheck, step: ReadjudicationStep) -> HeldCheck {
    match retained_current_failure(retained) {
        None => HeldCheck::Current,
        Some(FinalCheckFailure::ContextChanged) => HeldCheck::Readjudicate,
        Some(FinalCheckFailure::AuthorityChanged(
            OperationAuthorizationError::Unavailable
            | OperationAuthorizationError::ObservationUnavailable(_),
        )) => HeldCheck::Leave,
        // Its own check refuses it.
        Some(_) => match step {
            ReadjudicationStep::Decision => HeldCheck::Readjudicate,
            ReadjudicationStep::Read => HeldCheck::Leave,
        },
    }
}

impl ConsentCandidate {
    /// The exact decision this custody holds now.
    pub(crate) fn admitted(&self) -> PreparedOperationCheck {
        self.retained.lock().clone()
    }

    /// Hold `fresh`, a decision a re-adjudication found unchanged and that
    /// is current now. Only under the owner's reservation.
    fn hold(&self, fresh: &PreparedOperationCheck) {
        *self.retained.lock() = fresh.clone();
    }

    /// The approver of the published allowance, with the eligibility
    /// evidence it holds now.
    fn approval(&self) -> Option<ConsentApproval> {
        self.approval.lock().clone()
    }

    /// Keep `approval` with the custody. Only under the owner's reservation.
    fn approve(&self, approval: ConsentApproval) {
        *self.approval.lock() = Some(approval);
    }

    /// When the eligibility evidence the published allowance holds is no
    /// longer current (an owner change, related or not, under a stamp of
    /// the whole publication), ask the eligibility owner afresh, outside
    /// every lock, whether this approver was revoked or is forbidden since
    /// the decision (ADR-001 D9). `None` when the evidence is current, or no
    /// allowance was published.
    pub(crate) fn recheck_eligibility(&self) -> Option<EligibilityRecheck> {
        let approval = self.approval()?;
        if approval.eligibility.check_current().is_ok() {
            return None;
        }
        Some(
            match self
                .route
                .eligibility_answer(&approval.approver, &approval.record)
            {
                ConsentEligibilityAnswer::Eligible(evidence) => EligibilityRecheck::Fresh(evidence),
                ConsentEligibilityAnswer::Refused => EligibilityRecheck::Refused,
                ConsentEligibilityAnswer::Unsettled => EligibilityRecheck::Unsettled,
            },
        )
    }

    /// Re-adjudicate `retained`, a decision this custody held that is no
    /// longer current, outside every owner lock (ADR-001 D9): against
    /// `fresh`, a decision of its binding the canonical current/reprepare
    /// path already returned (a spend's entering decision), or else against
    /// the decision its owners return when asked afresh now
    /// ([`PreparedOperationCheck::reprepared`]): their answer as is, never a
    /// window this custody synthesizes or carries forward. The consent
    /// stands only for a decision of the same binding and work, at the same
    /// review tier, whose owners read the material this custody's decision
    /// is compared against.
    fn readjudicate(
        &self,
        retained: &PreparedOperationCheck,
        fresh: Option<PreparedOperationCheck>,
    ) -> Readjudication {
        let fresh = match fresh {
            Some(fresh) => fresh,
            None => match retained.reprepared() {
                Ok(fresh) => fresh,
                Err(error) => return readjudication_refused(retained, error),
            },
        };
        if !fresh.same_operation_and_work(retained) || fresh.review_tier() != retained.review_tier()
        {
            return Readjudication::Changed(ReviewRetirementReason::ContextChanged);
        }
        let read = fresh
            .work_authorization()
            .authorization()
            .review_operation_context(fresh.binding());
        match read {
            Ok(material) if material == self.material => Readjudication::Unchanged(fresh),
            Ok(_) => Readjudication::Changed(ReviewRetirementReason::ContextChanged),
            // A denial of the material is the owners' refusal of the
            // operation, as a denial of its preparation is.
            Err(error) => readjudication_refused(retained, error),
        }
    }

    /// The spend's re-adjudication of `entering` (ADR-001 D9): when it is
    /// not the decision this custody holds but one of the same binding and
    /// work (its staging prepared it afresh after an owner publication
    /// change), it is re-adjudicated against the held one, outside every
    /// lock. `None` otherwise: the spend settles the held decision itself,
    /// or refuses another operation's.
    pub(crate) fn readjudicate_entering(
        &self,
        entering: &PreparedOperationCheck,
    ) -> Option<Readjudication> {
        let retained = self.admitted();
        (!entering.same_check(&retained) && entering.same_operation_and_work(&retained))
            .then(|| self.readjudicate(&retained, Some(entering.clone())))
    }

    /// Custody of this candidate's attempt once its consent was spent at an
    /// entry: the spend removed the candidate from the owner's live custody
    /// and left the attempt registered as `Used` for the entered effect,
    /// until the admitting dispatch ends and releases it.
    pub(crate) fn entered_attempt(&self) -> ReviewAttemptHandle {
        self.attempt.twin()
    }

    /// Whether this custody keeps the very claim lineage `reservation`
    /// holds: it was admitted under the same live claim of its session.
    pub(super) fn keeps_lineage_of(
        &self,
        reservation: &crate::session_hosting::OwnerLossReservation,
    ) -> bool {
        self.hosting
            .as_ref()
            .is_some_and(|claim| reservation.holds_lineage_of(claim))
    }

    fn expired(&self) -> bool {
        tokio::time::Instant::now() >= self.deadline
    }

    /// This candidate's custody bound to a fresh decision of its exact
    /// original operation under `work`: moved into a resumed run, or rebound
    /// to the plan the session's agent this process rebuilt resolved afresh.
    /// The same attempt (its id and generated status), approval, validity,
    /// route and observer; `material` is what a later re-adjudication
    /// compares the owners' read against (the resumed run's own owners'
    /// read when it moved, for the account the approval presented, since
    /// their mandate may name the run's own requester; this custody's own
    /// otherwise).
    fn rebound(
        &self,
        admitted: PreparedOperationCheck,
        work: WorkAuthorizationContext,
        material: ReviewOperationContextResolution,
    ) -> Self {
        Self {
            attempt: self.attempt.rebound(admitted.binding().clone(), work),
            retained: parking_lot::Mutex::new(admitted),
            material,
            approval: parking_lot::Mutex::new(self.approval()),
            deadline: self.deadline,
            tool_call_id: Arc::clone(&self.tool_call_id),
            route: Arc::clone(&self.route),
            observer: self.observer.clone(),
            // Still custody of the same row and scope: it keeps the
            // session's claim lineage it was admitted under.
            hosting: self.hosting.clone(),
        }
    }

    fn observation(
        &self,
        status: ReviewAttemptStatus,
        retirement: Option<ReviewRetirementReason>,
    ) -> ReviewObservation {
        ReviewObservation {
            attempt: self.attempt.attempt_ref(),
            tool_call_id: Arc::clone(&self.tool_call_id),
            status,
            retirement,
        }
    }

    /// Report for a withdrawal whose attempt is disposed under a later
    /// release: the attempt's retirement, then the `Withdrawn` notice when
    /// the withdrawal was recorded durably.
    fn retirement_report(
        &self,
        reason: ReviewRetirementReason,
        withdrawn: Option<ApprovalId>,
    ) -> QueuedDisposalReport {
        let observer = self.observer.clone();
        let route = Arc::clone(&self.route);
        let attempt = self.attempt.attempt_ref();
        let tool_call_id = Arc::clone(&self.tool_call_id);
        Box::new(
            move |settled: Result<ReviewAttemptStatus, super::review::ReviewOwnerError>| {
                if let (Some(observer), Ok(status)) = (observer, settled) {
                    observer.observe(ReviewObservation {
                        attempt,
                        tool_call_id: Arc::clone(&tool_call_id),
                        status,
                        retirement: Some(reason),
                    });
                }
                if let Some(approval_id) = withdrawn {
                    route.notice(ConsentNotice {
                        approval_id,
                        tool_call_id,
                        kind: ConsentNoticeKind::Withdrawn,
                    });
                }
            },
        )
    }
}

/// A re-adjudication whose owners answered `error` instead of a decision or
/// its review material: a refusal for a moved publication is made again, or
/// decides nothing while that owner change is still in progress; a denial
/// (of the preparation, or of the material) is their refusal of the
/// operation; malformed facts changed the candidate's context; and owners
/// that are unavailable decide nothing.
fn readjudication_refused(
    retained: &PreparedOperationCheck,
    error: OperationAuthorizationError,
) -> Readjudication {
    match error {
        OperationAuthorizationError::Refused(refusal) => match refusal.kind() {
            OperationRefusalKind::ReprepareRequired => {
                if retained
                    .work_authorization()
                    .authorization()
                    .owner_change_in_progress()
                    .is_some()
                {
                    Readjudication::Unsettled
                } else {
                    Readjudication::Moved
                }
            }
            OperationRefusalKind::Denied => Readjudication::Refused(error),
            OperationRefusalKind::MalformedFacts => {
                Readjudication::Changed(ReviewRetirementReason::ContextChanged)
            }
        },
        OperationAuthorizationError::Unavailable
        | OperationAuthorizationError::ObservationUnavailable(_) => Readjudication::Unsettled,
    }
}

/// How the shared currentness rule left a live, unparked candidate
/// ([`ApprovalService::current_consent_locked`]).
#[derive(Debug)]
enum ConsentCurrentness {
    /// Its decision is current: the one it held, or the one a
    /// re-adjudication found unchanged, which it holds from now on.
    Current,
    /// Not current, and its re-adjudication could not decide now: nothing
    /// changed.
    Unsettled,
    /// Not current, with no re-adjudication that settles it (none yet, or
    /// another change met it): the caller re-adjudicates outside the lock
    /// and applies the rule again, within the bound; past it nothing
    /// changes.
    Readjudicate,
    /// Withdrawn under this reservation. An authority change carries the
    /// decision whose owners refused and that refusal, observed only once
    /// every lock is released.
    Withdrawn(Option<(PreparedOperationCheck, OperationAuthorizationError)>),
}

/// One decided consent a run can spend now: its approval and the original
/// tool call its candidate retained (that call already holds its local
/// result).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecidedConsent {
    pub(crate) approval_id: ApprovalId,
    pub(crate) tool_call_id: Arc<str>,
    pub(crate) tool_name: Option<String>,
    /// The staged notice input this consent moved into the run with (its
    /// custody `Transferred { input_id }`); `None` for consent the run
    /// requested itself.
    pub(crate) moved_from: Option<String>,
}

/// What a run's boundary finds of the decided consent it requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecidedConsentScan {
    /// The approval owner is held or unavailable: nothing was examined and
    /// the run's next boundary looks again.
    Deferred,
    /// The requested consent the run can spend now (possibly none).
    Decided(Vec<DecidedConsent>),
}

/// What a resumed run's boundary finds of the consent the approval owner
/// moved into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MovedConsentScan {
    /// The approval owner is held now: nothing was examined, and the
    /// boundary yields and looks again.
    Deferred,
    /// The live consent moved into the run, by approval (possibly none).
    Moved(Vec<ApprovalId>),
}

/// What rebinding one live custody to its re-resolved plan did
/// ([`ApprovalService::rebind_consent_to_reresolved_plan`]).
#[derive(Debug)]
pub(crate) enum ConsentRebind {
    /// The custody now holds the fresh decision of the same durable
    /// execution; a new attempt of this run resolves it.
    Rebound,
    /// Not decided now: the owner is held, the fresh decision's owners are
    /// unavailable, or an owner change is in progress (or published during
    /// the fresh preparation). Nothing changed, and the consent stays
    /// spendable.
    NotReady,
    /// Refused with zero entry: no live custody of this work and run, or a
    /// fresh plan that is not the same durable execution (which withdraws
    /// the consent).
    Refused(ConsentRefusal),
}

/// Everything a retained candidate needs except its attempt, which the owner
/// begins under the same reservation as the approval.
pub(crate) struct ConsentCandidateParts {
    pub(crate) admitted: PreparedOperationCheck,
    /// The owners' review material the approval presents with it.
    pub(crate) material: ReviewOperationContextResolution,
    pub(crate) deadline: tokio::time::Instant,
    pub(crate) tool_call_id: Arc<str>,
    pub(crate) route: Arc<dyn HumanConsentRoute>,
    pub(crate) observer: Option<Arc<dyn OperationReviewObserver>>,
}

enum ConsentProjection {
    Observation(Arc<dyn OperationReviewObserver>, ReviewObservation),
    Notice(Arc<dyn HumanConsentRoute>, ConsentNotice),
}

/// Consent projections collected under the owner's locks and delivered once
/// every lock is released: observations and notices in order, then settled
/// reports of unrelated disposals. Dropping delivers them, so none is lost.
#[derive(Default)]
#[must_use = "deliver once every lock is released"]
pub(crate) struct ConsentProjections {
    items: Vec<ConsentProjection>,
    reports: SettledReports,
}

impl ConsentProjections {
    pub(crate) fn observe(
        &mut self,
        candidate: &ConsentCandidate,
        status: ReviewAttemptStatus,
        retirement: Option<ReviewRetirementReason>,
    ) {
        if let Some(observer) = candidate.observer.as_ref() {
            self.items.push(ConsentProjection::Observation(
                Arc::clone(observer),
                candidate.observation(status, retirement),
            ));
        }
    }

    pub(crate) fn notify(
        &mut self,
        candidate: &ConsentCandidate,
        approval_id: &ApprovalId,
        kind: ConsentNoticeKind,
    ) {
        self.items.push(ConsentProjection::Notice(
            Arc::clone(&candidate.route),
            ConsentNotice {
                approval_id: approval_id.clone(),
                tool_call_id: Arc::clone(&candidate.tool_call_id),
                kind,
            },
        ));
    }

    pub(crate) fn absorb_reports(&mut self, reports: SettledReports) {
        self.reports.absorb(reports);
    }

    pub(crate) fn extend(&mut self, other: Self) {
        let mut other = other;
        self.items.append(&mut other.items);
        self.reports.absorb(std::mem::take(&mut other.reports));
    }

    pub(crate) fn deliver(self) {
        drop(self);
    }
}

impl Drop for ConsentProjections {
    fn drop(&mut self) {
        for item in self.items.drain(..) {
            match item {
                ConsentProjection::Observation(observer, observation) => {
                    observer.observe(observation);
                }
                ConsentProjection::Notice(route, notice) => route.notice(notice),
            }
        }
        std::mem::take(&mut self.reports).run();
    }
}

/// Why one consent attempt did not enter. Every refusal is zero entry.
#[derive(Debug)]
pub(crate) enum ConsentRefusal {
    /// No spendable consent for this exact candidate here: still pending,
    /// expired, unknown, or a reference copied from another work
    /// association or run. Fresh consent is required.
    NoConsent,
    Denied,
    AlreadyConsumed,
    /// The retained decision is no longer the current one, and a related
    /// change (or another operation's decision) withdrew the consent.
    ContextChanged,
    /// The prepared owner's current check refused under the reservation; the
    /// failure is observed only once every lock is released.
    AuthorityChanged {
        check: PreparedOperationCheck,
        error: crate::authorization::OperationAuthorizationError,
    },
    /// The owners, asked afresh for the held decision before an attempt was
    /// handed it, refused the operation (ADR-001 D9): the consent was
    /// withdrawn as `AuthorityChanged`, and this is their answer, already
    /// observed where it was made.
    OwnersRefused(crate::authorization::OperationAuthorizationError),
    /// The owner could not commit now (contention, a store that wrote
    /// nothing). Nothing was spent: the consent stays spendable. Also the
    /// answer of a service that holds no custody for unspent, unwithdrawn
    /// consent it did not issue: that says only that this service cannot
    /// spend it, not that another owner is live or that the issuing process
    /// stopped. And the answer of a spend whose entering decision, or whose
    /// approver's eligibility evidence, is not current under its reservation
    /// and was not settled for it (an owner change after its staging or its
    /// re-resolution, or an eligibility owner that could not say), and of an
    /// attempt whose held decision could not be settled before it was handed
    /// it: nothing is spent, the consent is kept, and a retry decides afresh
    /// (ADR-001 D9). A run's own attempt counts as not made.
    OwnerUnavailable,
    /// Another owner of the store changed the approval first. The custody
    /// was retired; the row is reconciled before reuse.
    Conflict,
    /// The spend's store commit may have happened. Nothing entered, nothing
    /// is refunded, and the row is reconciled before reuse.
    CommitUnknown,
}

impl ConsentRefusal {
    /// The refusal, observing a refused authority now: call only with every
    /// owner lock released.
    pub(crate) fn into_tool_error(self) -> crate::ToolError {
        self.into_entry_refusal().resolve()
    }

    /// The refusal, deferring the observation of a refused authority to
    /// [`EntryRefusal::resolve`].
    pub(crate) fn into_entry_refusal(self) -> EntryRefusal {
        match self {
            Self::AuthorityChanged { check, error } => EntryRefusal::Authority { check, error },
            settled => EntryRefusal::Settled(settled.into_settled_error()),
        }
    }

    fn into_settled_error(self) -> crate::ToolError {
        let unsatisfied = |kind| crate::ToolError::ReviewUnsatisfied {
            kind,
            approval_id: None,
        };
        match self {
            Self::NoConsent => unsatisfied(ReviewUnsatisfiedKind::HumanConsentRequired),
            Self::Denied => unsatisfied(ReviewUnsatisfiedKind::Denied),
            Self::AlreadyConsumed => unsatisfied(ReviewUnsatisfiedKind::AlreadyConsumed),
            Self::ContextChanged => unsatisfied(ReviewUnsatisfiedKind::ContextChanged),
            Self::AuthorityChanged { check, error } => {
                crate::ToolError::from(check.observe_retained_check_failure(error))
            }
            Self::OwnersRefused(error) => crate::ToolError::from(error),
            Self::OwnerUnavailable => crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::OwnerUnavailable,
            },
            Self::Conflict => crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::StoreConflict,
            },
            Self::CommitUnknown => crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::StoreCommitUnknown,
            },
        }
    }

    /// Refusal for an approval this owner holds no live candidate for.
    fn without_custody(record: Option<&ApprovalRecord>) -> Self {
        match record {
            Some(record)
                if record
                    .consent
                    .as_ref()
                    .is_some_and(|consent| consent.consumed_at.is_some()) =>
            {
                Self::AlreadyConsumed
            }
            Some(record) if record.status == ApprovalStatus::Denied => Self::Denied,
            _ => Self::NoConsent,
        }
    }
}

/// The approval request for one exact candidate: the owner's own action,
/// binding-owned arguments, owner reference and finite expiry, with the
/// host's presentation facts. `None` when the validity cannot be represented.
pub(crate) fn consent_request(
    call: ToolCallView<'_>,
    presentation: ConsentPresentation,
    validity: std::time::Duration,
) -> Option<ApprovalRequest> {
    let expires_at = Utc::now().checked_add_signed(chrono::Duration::from_std(validity).ok()?)?;
    let arguments: serde_json::Value = serde_json::from_str(call.args.get()).ok()?;
    Some(ApprovalRequest {
        requester: presentation.requester,
        owner: ApprovalOwnerRef::ToolCall {
            tool_call_id: ToolCallId::new(call.id),
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::ToolCall,
            id: ApprovalResourceId::new(call.name),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::ToolCall,
            summary: call.name.to_owned(),
            body: Some(arguments),
        },
        risk: presentation.risk,
        request_body: None,
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
            .into_iter()
            .collect(),
        expires_at: Some(expires_at),
        metadata: presentation.metadata,
        request_provenance: None,
    })
}

impl ApprovalService {
    /// Issue one consent approval for the exact retained candidate together
    /// with its linked human attempt, under ONE non-blocking reservation (this
    /// runs on the dispatch's async thread): a held owner refuses at once
    /// with nothing created. The stored row is inserted conditionally, before
    /// anything is published. Returns the committed approval's id, the only
    /// reference feedback may carry.
    pub(crate) fn try_request_consent(
        &self,
        request: ApprovalRequest,
        parts: ConsentCandidateParts,
    ) -> Result<(ApprovalId, ConsentProjections), ApprovalError> {
        self.ensure_available()?;
        if request.requester.as_str().trim().is_empty() {
            return Err(ApprovalError::InvalidPrincipal);
        }
        request.metadata.validate_public()?;
        let now = Utc::now();
        let Some(mut state) = self.state.try_write() else {
            return Err(ApprovalError::StoreUnavailable(
                "approval owner is held by another commit".to_string(),
            ));
        };
        self.count_review_lock();
        let mut projections = ConsentProjections::default();
        let result = self.request_consent_locked(&mut state, request, parts, now, &mut projections);
        projections.absorb_reports(self.release_state(state));
        match result {
            Ok(approval_id) => Ok((approval_id, projections)),
            Err(error) => {
                projections.deliver();
                Err(error)
            }
        }
    }

    fn request_consent_locked(
        &self,
        state: &mut ApprovalServiceState,
        request: ApprovalRequest,
        parts: ConsentCandidateParts,
        now: DateTime<Utc>,
        projections: &mut ConsentProjections,
    ) -> Result<ApprovalId, ApprovalError> {
        let approval_id = ApprovalId::new();
        let attempt_id: Arc<str> = Arc::from(crate::time_compat::new_uuid_v7().to_string());
        let (approve_allowed, deny_allowed) = allowed_decision_flags(&request.allowed_decisions);
        let mut authority = state.authority.clone();
        let outcome = authority
            .create_consent_approval(
                approval_id.to_string(),
                attempt_id.to_string(),
                approve_allowed,
                deny_allowed,
                request.expires_at.is_some(),
            )
            .map_err(lifecycle_error)?;
        let status = lifecycle_status_from_outcome(&approval_id, outcome, None)?;
        let record = ApprovalRecord {
            approval_id: approval_id.clone(),
            status,
            requester: request.requester,
            owner: request.owner,
            resource: request.resource,
            proposed_action: request.proposed_action,
            risk: request.risk,
            request_body: request.request_body,
            allowed_decisions: request.allowed_decisions,
            expires_at: request.expires_at,
            created_at: now,
            updated_at: now,
            metadata: request.metadata,
            request_provenance: request.request_provenance,
            decision: None,
            consent: Some(ApprovalConsentRecord {
                scope: parts.admitted.work_authorization().consent_scope().cloned(),
                origin: parts
                    .admitted
                    .work_authorization()
                    .retained_work()
                    .cloned()
                    .map(|retained_work| ApprovalConsentOrigin { retained_work }),
                ..ApprovalConsentRecord::default()
            }),
        };
        self.commit_row_locked(state, None, &record)?;
        state.authority = authority;
        state.records.insert(approval_id.clone(), record);
        let hosting = parts
            .admitted
            .work_authorization()
            .consent_hosting()
            .cloned();
        let candidate = Arc::new(ConsentCandidate {
            attempt: ReviewAttemptHandle::new(
                attempt_id,
                parts.admitted.binding().clone(),
                parts.admitted.work_authorization().clone(),
            ),
            retained: parking_lot::Mutex::new(parts.admitted),
            material: parts.material,
            approval: parking_lot::Mutex::new(None),
            deadline: parts.deadline,
            tool_call_id: parts.tool_call_id,
            route: parts.route,
            observer: parts.observer,
            hosting,
        });
        projections.observe(&candidate, ReviewAttemptStatus::Pending, None);
        projections.notify(&candidate, &approval_id, ConsentNoticeKind::Requested);
        state.consent.insert(approval_id.clone(), candidate);
        state.issued_consent.insert(approval_id.clone());
        Ok(approval_id)
    }

    /// Record the decision of one authenticated decision event, exactly as
    /// [`Self::decide_consent`] records it: the event binds the approval and
    /// the decision the host adapter authenticated, including the approver's
    /// authentication scope for the eligibility owner.
    pub fn decide_consent_event(
        &self,
        event: AuthenticatedConsentEvent,
    ) -> Result<ApprovalRecord, ApprovalError> {
        let AuthenticatedConsentEvent {
            approval_id,
            decision,
        } = event;
        self.decide_consent(&approval_id, decision)
    }

    /// Record one authenticated human decision on a consent approval. It only
    /// records and notifies; it never executes the retained action, which
    /// needs a fresh explicit attempt.
    ///
    /// The approver's eligibility is resolved from the host route's
    /// eligibility owner outside every owner lock
    /// ([`HumanConsentRoute::eligibility_answer`]), and its evidence is
    /// checked again, coherently, under the commit reservation together with
    /// the live candidate, the exact resolved record and expiry. Evidence an
    /// owner change made stale since (related or not: it stamps the owner's
    /// whole publication) is resolved afresh outside the reservation, within
    /// three rounds (the consent re-adjudication bound): an approver the
    /// eligibility owner now refuses (a revocation in between) is not
    /// eligible and leaves the approval pending, with no notice, while an
    /// eligibility owner that cannot say now, or a publication that keeps
    /// changing, records nothing and answers a retryable `StoreUnavailable`.
    /// An approval whose retained candidate this owner no longer holds
    /// (consumed, declined, expired, withdrawn, or lost on restart) needs
    /// fresh consent instead.
    ///
    /// For an approval, a retained decision that is no longer current (an
    /// owner publication change made it stale, or its own check refuses it,
    /// as when its owner's validity window for it ended while the human took
    /// their time) is re-adjudicated before the decision and again before
    /// its allowance is published (ADR-001 D9): an unrelated change leaves
    /// the candidate as it was, while a change of its binding, review tier
    /// or material, or its owners' refusal, withdraws it. While its
    /// re-adjudication cannot settle it (an owner change still in progress,
    /// or owners that are unavailable), the approval records nothing and
    /// answers a retryable `StoreUnavailable`. A decline stands on no
    /// retained decision: it is recorded whatever that decision's
    /// currentness.
    pub fn decide_consent(
        &self,
        approval_id: &ApprovalId,
        decision: AuthenticatedConsentDecision,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.ensure_available()?;
        // Only an approval stands on the retained decision: a decline is
        // recorded whatever its currentness, even while its owners' change
        // is still in progress.
        if decision.decision == ApprovalDecision::Approve
            && !self.readjudicate_consent(approval_id, ReadjudicationStep::Decision)
        {
            return Err(unsettled_decision(approval_id));
        }
        self.reconcile_if_unreconciled(approval_id)?;
        let (record, candidate) = {
            let state = self.state.read();
            (
                state.records.get(approval_id).cloned(),
                state.consent.get(approval_id).cloned(),
            )
        };
        // A reader may have made a disposer's try-lock fail; settle after it.
        self.settle_owner_now().run();
        let Some(record) = record else {
            return Err(ApprovalError::NotFound {
                approval_id: approval_id.clone(),
            });
        };
        let Some(candidate) = candidate else {
            return Err(ApprovalError::ConsentUnavailable {
                approval_id: approval_id.clone(),
            });
        };
        // The host's eligibility owner, consulted outside every owner lock:
        // again while an owner change made its evidence stale before the
        // reservation. An owner that cannot say now decides nothing.
        let eligibility = || match candidate
            .route
            .eligibility_answer(&decision.approver, &record)
        {
            ConsentEligibilityAnswer::Eligible(evidence) => Ok(evidence),
            ConsentEligibilityAnswer::Refused => Err(ApprovalError::NotEligible {
                approval_id: approval_id.clone(),
            }),
            ConsentEligibilityAnswer::Unsettled => Err(unsettled_decision(approval_id)),
        };
        let mut evidence = eligibility()?;
        let now = self.now();
        let mut rounds: u8 = 0;
        let mut state = loop {
            let state = self.state.write();
            if evidence.check_current().is_ok() {
                break state;
            }
            self.release_state(state).run();
            rounds += 1;
            if rounds > CONSENT_READJUDICATION_BOUND {
                return Err(unsettled_decision(approval_id));
            }
            evidence = eligibility()?;
        };
        let approver = decision.approver.clone();
        let mut projections = ConsentProjections::default();
        let decided = self.decide_consent_locked(
            &mut state,
            approval_id,
            &candidate,
            Resolved {
                record: &record,
                eligibility: evidence.as_ref(),
            },
            decision,
            now,
            &mut projections,
        );
        // Every store and owner-exit step of the decision completes here,
        // before its final checks.
        projections.absorb_reports(self.release_state(state));
        let result = match decided {
            // From now on the eligibility owner is asked about the approval
            // as decided: the record that carries this approver's decision.
            Ok(Decided::Approved(decided)) => self.publish_allowance(
                approval_id,
                &candidate,
                ConsentApproval {
                    approver,
                    record: decided.clone(),
                    eligibility: evidence,
                },
                decided,
                &mut projections,
            ),
            Ok(Decided::Declined(decided)) => Ok(decided),
            Err(error) => Err(error),
        };
        projections.deliver();
        result
    }

    /// The decision's final reservation, after its store and owner-exit work:
    /// the callback-free final checks (approver eligibility, the retained
    /// decision, the candidate deadline and the record's expiry), then the
    /// allowance is published, keeping its approver and their evidence for
    /// the spend, or the linked attempt withdrawn. This reservation releases
    /// without store work, so nothing foreign runs between the final checks
    /// and the Approved notice. A committed decision stays history either
    /// way; a withdrawal that could not be recorded leaves the row to
    /// reconciliation (typed Unreconciled).
    ///
    /// What went stale since (the eligibility evidence, or the retained
    /// decision, by an owner change related or not, or a decision its own
    /// check refuses) is decided afresh by its owners outside the
    /// reservation, and every final check runs again, at most
    /// [`CONSENT_READJUDICATION_BOUND`] times (ADR-001 D9): their refusal or a
    /// related change withdraws the consent, and past the bound the
    /// allowance is published as it is, its spend checking both again.
    fn publish_allowance(
        &self,
        approval_id: &ApprovalId,
        candidate: &Arc<ConsentCandidate>,
        mut approval: ConsentApproval,
        decided: ApprovalRecord,
        projections: &mut ConsentProjections,
    ) -> Result<ApprovalRecord, ApprovalError> {
        let mut readjudication = None;
        let mut eligible = true;
        let mut rounds: u8 = 0;
        loop {
            let mut state = self.state.write();
            let published = self.publish_allowance_locked(
                &mut state,
                approval_id,
                candidate,
                PublishChecks {
                    approval: eligible.then_some(&approval),
                    decided: &decided,
                    readjudication: readjudication.as_ref(),
                    exhausted: rounds >= CONSENT_READJUDICATION_BOUND,
                },
                projections,
            );
            projections.absorb_reports(self.release_review_state(state));
            if let Some(result) = published {
                return result;
            }
            rounds += 1;
            // Outside the reservation, the owners decide afresh what is no
            // longer current: the eligibility owner the approver (an owner
            // that cannot say now leaves the stale evidence, asked again in
            // the next round), the decision's owners its decision (never
            // called while its own check answers that they are unavailable:
            // nothing is decided then).
            if approval.eligibility.check_current().is_err() {
                match candidate
                    .route
                    .eligibility_answer(&approval.approver, &approval.record)
                {
                    ConsentEligibilityAnswer::Eligible(evidence) => {
                        approval.eligibility = evidence;
                    }
                    ConsentEligibilityAnswer::Refused => eligible = false,
                    ConsentEligibilityAnswer::Unsettled => {}
                }
            }
            let held = candidate.admitted();
            readjudication = match held_check(&held, ReadjudicationStep::Decision) {
                HeldCheck::Current => None,
                HeldCheck::Readjudicate => Some(candidate.readjudicate(&held, None)),
                HeldCheck::Leave => Some(Readjudication::Unsettled),
            };
        }
    }

    /// One round of the decision's final checks; `None` when the approver's
    /// eligibility or the retained decision is to be decided afresh first,
    /// outside the reservation.
    fn publish_allowance_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        candidate: &Arc<ConsentCandidate>,
        checks: PublishChecks<'_>,
        projections: &mut ConsentProjections,
    ) -> Option<Result<ApprovalRecord, ApprovalError>> {
        let current = |state: &ApprovalServiceState| {
            state.ensure_reconciled(approval_id).map(|()| {
                state
                    .records
                    .get(approval_id)
                    .cloned()
                    .unwrap_or_else(|| checks.decided.clone())
            })
        };
        // Withdrawn meanwhile (its run closed, it was abandoned): nothing to
        // publish; the committed decision and its withdrawal stand.
        if !is_live(state, approval_id, candidate) {
            return Some(current(state));
        }
        let record = match current(state) {
            Ok(record) => record,
            Err(error) => return Some(Err(error)),
        };
        // The approver's eligibility: the eligibility owner's refusal
        // withdraws the consent, and evidence an owner change made stale
        // (related or not) is decided afresh first.
        let Some(approval) = checks.approval else {
            self.withdraw_consent_locked(
                state,
                approval_id,
                ReviewRetirementReason::AuthorityChanged,
            );
            return Some(current(state));
        };
        if approval.eligibility.check_current().is_err() && !checks.exhausted {
            return None;
        }
        // A parked candidate's retained decision belongs to its closed run:
        // only eligibility and validity decide here, and the allowance stays
        // unspendable until a transfer publishes it for a resumed run. Any
        // other candidate's decision follows the shared currentness rule: a
        // decision its re-adjudication cannot settle now, or within the
        // bound, is published as it is, and its spend never enters on it.
        if !state.parked.contains(approval_id) {
            match self.current_consent_locked(
                state,
                approval_id,
                candidate,
                checks.readjudication,
                HeldRefusal::Readjudicate,
            ) {
                ConsentCurrentness::Current | ConsentCurrentness::Unsettled => {}
                ConsentCurrentness::Readjudicate if checks.exhausted => {}
                ConsentCurrentness::Readjudicate => return None,
                ConsentCurrentness::Withdrawn(_) => return Some(current(state)),
            }
        }
        if candidate.expired()
            || record
                .expires_at
                .is_some_and(|expires_at| expires_at <= self.now())
        {
            self.withdraw_consent_locked(
                state,
                approval_id,
                ReviewRetirementReason::DeadlineExpired,
            );
            return Some(current(state));
        }
        state.unpublished.remove(approval_id);
        candidate.approve(approval.clone());
        projections.observe(candidate, ReviewAttemptStatus::Allowed, None);
        projections.notify(candidate, approval_id, ConsentNoticeKind::Approved);
        Some(Ok(record))
    }

    #[allow(clippy::too_many_arguments)]
    fn decide_consent_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        candidate: &Arc<ConsentCandidate>,
        resolved: Resolved<'_>,
        decision: AuthenticatedConsentDecision,
        now: DateTime<Utc>,
        projections: &mut ConsentProjections,
    ) -> Result<Decided, ApprovalError> {
        let not_eligible = || ApprovalError::NotEligible {
            approval_id: approval_id.clone(),
        };
        // Still the live candidate: not consumed, declined, expired or
        // withdrawn since the snapshot.
        if !is_live(state, approval_id, candidate) {
            return Err(ApprovalError::ConsentUnavailable {
                approval_id: approval_id.clone(),
            });
        }
        // The exact record the eligibility was resolved for, unchanged but for
        // its executable custody (its run's close may park it meanwhile), and
        // that evidence still current at commitment.
        if !state
            .records
            .get(approval_id)
            .is_some_and(|current| same_but_custody(current, resolved.record))
            || resolved.eligibility.check_current().is_err()
        {
            return Err(not_eligible());
        }
        // Expiry: the record's expiry or the attempt's deadline. For an
        // approval, the retained decision's currentness was settled by its
        // re-adjudication before this reservation, and is again before the
        // allowance is published (ADR-001 D9; a parked candidate's belongs to
        // its closed run and is not checked); a decline stands on none.
        self.observe_expiry_locked(state, approval_id, now, candidate.expired())?;
        if !is_live(state, approval_id, candidate) {
            return Err(
                match state.records.get(approval_id).map(|record| record.status) {
                    Some(ApprovalStatus::Expired) => ApprovalError::Expired {
                        approval_id: approval_id.clone(),
                    },
                    _ => ApprovalError::ConsentUnavailable {
                        approval_id: approval_id.clone(),
                    },
                },
            );
        }
        // The row as it stands now: a park recorded above changed only its
        // custody, and the decision commits on top of it.
        let Some(record) = state
            .records
            .get(approval_id)
            .filter(|current| same_but_custody(current, resolved.record))
            .cloned()
        else {
            return Err(not_eligible());
        };
        let attempt_id = candidate.attempt.id().to_owned();
        let mut authority = state.authority.clone();
        let outcome = authority
            .decide_consent(
                approval_id.to_string(),
                attempt_id.clone(),
                approval_lifecycle_decision(decision.decision),
                true,
            )
            .map_err(lifecycle_error)?;
        let status = lifecycle_status_from_outcome(approval_id, outcome, Some(decision.decision))?;
        let declined = decision.decision == ApprovalDecision::Deny;
        let attempt_status = if declined {
            // A declined attempt is unlinked and never spendable: dispose it
            // in the same reservation.
            let _ = review_status_from_outcome(authority.release_review(attempt_id));
            ReviewAttemptStatus::Denied
        } else {
            ReviewAttemptStatus::Allowed
        };
        let actor = ApprovalPrincipalId::new(decision.approver.principal().id.as_str())?;
        let mut decided = record.clone();
        decided.status = status;
        decided.updated_at = now;
        decided.decision = Some(ApprovalDecisionRecord {
            decision: decision.decision,
            actor,
            decided_at: now,
            reason: decision.reason,
            provenance: None,
        });
        let mut consent = ApprovalConsentRecord {
            approver: Some(ApprovalConsentApprover {
                principal: decision.approver.principal().clone(),
                assurance: decision.approver.assurance(),
            }),
            consumed_at: None,
            withdrawal: None,
            ..record.consent.clone().unwrap_or_default()
        };
        // The decision owes its owning session a notice, committed with it,
        // whatever later happens to the candidate's executable custody.
        consent.owe_delivery(
            if declined {
                ApprovalConsentTransition::Denied
            } else {
                ApprovalConsentTransition::Approved
            },
            now,
        );
        decided.consent = Some(consent);
        self.commit_row_locked(state, Some(&record), &decided)?;
        self.note_consent_delivery_change();
        state.authority = authority;
        state.records.insert(approval_id.clone(), decided.clone());
        if declined {
            state.consent.remove(approval_id);
            state.parked.remove(approval_id);
            projections.observe(candidate, attempt_status, None);
            projections.notify(candidate, approval_id, ConsentNoticeKind::Denied);
            return Ok(Decided::Declined(decided));
        }
        // Committed, but not spendable until the decision's final checks
        // (after its store and owner-exit work) publish it.
        state.unpublished.insert(approval_id.clone());
        Ok(Decided::Approved(decided))
    }

    /// Resolve a fresh explicit attempt of one retained candidate, without
    /// spending anything. Only the presenting context's SAME admitted work
    /// association and run resolve it: a reference copied into another work
    /// association or run, or equal-looking facts, have no consent here.
    /// Never waits for the owner (callers run on async threads).
    pub(crate) fn resolve_consent_attempt(
        &self,
        approval_id: &ApprovalId,
        work: &WorkAuthorizationContext,
        run_id: Option<&RunId>,
    ) -> Result<Arc<ConsentCandidate>, ConsentRefusal> {
        if self.ensure_available().is_err() {
            return Err(ConsentRefusal::OwnerUnavailable);
        }
        let Some(state) = self.state.try_read() else {
            return Err(ConsentRefusal::OwnerUnavailable);
        };
        // A parked candidate's run closed: nothing resolves it until a
        // transfer publishes it for a resumed run.
        let parked = state.parked.contains(approval_id);
        let candidate = state
            .consent
            .get(approval_id)
            .filter(|_| !state.unpublished.contains(approval_id) && !parked)
            .cloned();
        let record = state.records.get(approval_id).cloned();
        let issued_here = state.issued_consent.contains(approval_id);
        drop(state);
        self.settle_owner_now().run();
        let Some(candidate) = candidate else {
            // Parked for the work this presenting run resumes: its move is
            // still pending (its notice receipt or the transfer itself), a
            // readiness gap, never missing consent.
            if parked
                && record
                    .as_ref()
                    .is_some_and(|record| resumes_origin(record, work))
            {
                return Err(ConsentRefusal::OwnerUnavailable);
            }
            // Unspent, unwithdrawn consent this service did not issue: it
            // holds no custody and cannot tell whether the issuing owner
            // still does, so the consent is unavailable here rather than
            // missing. No claim about that owner is made.
            if !issued_here
                && record.as_ref().is_some_and(|record| {
                    matches!(
                        record.status,
                        ApprovalStatus::Pending | ApprovalStatus::Approved
                    ) && record.consent.as_ref().is_some_and(|consent| {
                        consent.consumed_at.is_none() && consent.withdrawal.is_none()
                    })
                })
            {
                return Err(ConsentRefusal::OwnerUnavailable);
            }
            return Err(ConsentRefusal::without_custody(record.as_ref()));
        };
        let admitted = candidate.admitted();
        let binding = admitted.binding();
        if !candidate.attempt.bound_to(binding, work) || binding.facts().run_id.as_ref() != run_id {
            return Err(ConsentRefusal::NoConsent);
        }
        match record {
            Some(record)
                if record.status == ApprovalStatus::Approved
                    && !candidate.expired()
                    && record
                        .expires_at
                        .is_none_or(|expires_at| expires_at > Utc::now()) =>
            {
                Ok(candidate)
            }
            _ => Err(ConsentRefusal::NoConsent),
        }
    }

    /// The decided consent among `requested` (consent the run `run_id` of
    /// `work` requested itself, or that a receipt moved into it) that the
    /// run can spend now, in approval order: live custody of a candidate
    /// admitted under exactly that work and run, decided Approved with its
    /// allowance published, neither parked nor expired, and unspent. Only
    /// the requested candidates are examined, and the owner is never waited
    /// for: a held or unavailable owner is [`DecidedConsentScan::Deferred`],
    /// never "none decided". It only names candidates; each spend still
    /// resolves, checks and enters through its own fresh attempt.
    pub(crate) fn decided_consent_for_run(
        &self,
        work: &WorkAuthorizationContext,
        run_id: Option<&RunId>,
        requested: &[ApprovalId],
    ) -> DecidedConsentScan {
        if self.ensure_available().is_err() {
            return DecidedConsentScan::Deferred;
        }
        let Some(state) = self.state.try_read() else {
            return DecidedConsentScan::Deferred;
        };
        let now = Utc::now();
        let decided = requested
            .iter()
            .filter_map(|approval_id| {
                let candidate = state.consent.get(approval_id)?;
                let admitted = candidate.admitted();
                let binding = admitted.binding();
                let record = state.records.get(approval_id);
                let spendable = !state.unpublished.contains(approval_id)
                    && !state.parked.contains(approval_id)
                    && !candidate.expired()
                    && candidate.attempt.bound_to(binding, work)
                    && binding.facts().run_id.as_ref() == run_id
                    && record.is_some_and(|record| {
                        record.status == ApprovalStatus::Approved
                            && record.expires_at.is_none_or(|expires_at| expires_at > now)
                            && record
                                .consent
                                .as_ref()
                                .is_some_and(|consent| consent.consumed_at.is_none())
                    });
                spendable.then(|| DecidedConsent {
                    approval_id: approval_id.clone(),
                    tool_call_id: Arc::clone(&candidate.tool_call_id),
                    tool_name: admitted
                        .tool_dispatch_parts()
                        .map(|(call, _)| call.name.to_owned()),
                    moved_from: record.and_then(moved_input),
                })
            })
            .collect();
        drop(state);
        // A reader may have made a disposer's try-lock fail; settle after it.
        self.settle_owner_now().run();
        DecidedConsentScan::Decided(decided)
    }

    /// The live consent this owner moved into the open resumed run `run_id`
    /// of `work`, by the committed receipts of its staged notice inputs:
    /// the approvals whose transfer into exactly that run's entry it
    /// committed, still in live, unexpired custody recorded as `Transferred`
    /// to one of that run's own staged inputs. Withdrawn, spent or expired
    /// consent, consent moved into another run, and parked or pending
    /// consent are never named; nothing is matched by tool or arguments.
    /// The owner is never waited for: a held owner is
    /// [`MovedConsentScan::Deferred`], and the boundary yields and looks
    /// again. An unavailable owner holds no custody, so nothing moved. It
    /// only names candidates for the run's own consent attempts.
    pub(crate) fn consent_moved_into_run(
        &self,
        work: &WorkAuthorizationContext,
        run_id: Option<&RunId>,
    ) -> MovedConsentScan {
        if self.ensure_available().is_err() {
            return MovedConsentScan::Moved(Vec::new());
        }
        let staged: Vec<String> = work
            .resumed_inputs()
            .iter()
            .map(|resumed| resumed.input_id.to_string())
            .collect();
        let Some(state) = self.state.try_read() else {
            return MovedConsentScan::Deferred;
        };
        let moved = state
            .resumed_runs
            .iter()
            .find(|open| open.work.same_context(work) && Some(&open.run_id) == run_id)
            .map(|open| {
                open.moved
                    .iter()
                    .filter(|approval_id| {
                        state
                            .consent
                            .get(*approval_id)
                            .is_some_and(|candidate| !candidate.expired())
                            && state
                                .records
                                .get(*approval_id)
                                .and_then(moved_input)
                                .is_some_and(|input_id| staged.contains(&input_id))
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        drop(state);
        // A reader may have made a disposer's try-lock fail; settle after it.
        self.settle_owner_now().run();
        MovedConsentScan::Moved(moved)
    }

    /// Rebind the live custody of `approval_id`, admitted as `old` under
    /// `work` and run `run_id`, to the same retained call resolved afresh as
    /// `fresh` by the session's agent this process rebuilt after the plan
    /// was resolved. As a transfer moves custody, in three steps: a
    /// non-waiting read of the live candidate, the fresh plan's checks and
    /// decision outside every lock, and a re-checked replacement of the
    /// candidate (its attempt rebound, its generated status untouched). No
    /// row is written: the custody stays as recorded. `fresh` is accepted
    /// only as the same durable execution
    /// ([`crate::ResolvedToolExecutionPlan::same_durable_execution`]: the
    /// same call, declaration digest, execution mode and durable operation
    /// targets) of the same operation, decided afresh by the run's own
    /// owners as R3. A proven mismatch, or a refusal of the fresh decision,
    /// withdraws the consent; a held owner or a fresh decision that cannot
    /// be made now (its owners unavailable, or an owner change in progress
    /// or published during the preparation: ADR-001 D9) leaves it
    /// spendable, and the run's next boundary rebinds it afresh. The owner is
    /// never waited for.
    pub(crate) fn rebind_consent_to_reresolved_plan(
        &self,
        approval_id: &ApprovalId,
        work: &WorkAuthorizationContext,
        run_id: Option<&RunId>,
        old: &PreparedOperationCheck,
        fresh: crate::ResolvedToolExecutionPlan,
    ) -> ConsentRebind {
        if self.ensure_available().is_err() {
            return ConsentRebind::Refused(ConsentRefusal::OwnerUnavailable);
        }
        // 1. The live candidate of exactly the operation this run's attempt
        // admitted, whichever decision of it the custody holds now (a
        // re-adjudication may have replaced it since: ADR-001 D9).
        let Some(state) = self.state.try_read() else {
            return ConsentRebind::NotReady;
        };
        let candidate = state
            .consent
            .get(approval_id)
            .filter(|candidate| {
                !state.parked.contains(approval_id)
                    && !state.unpublished.contains(approval_id)
                    && candidate.admitted().same_operation_and_work(old)
                    && candidate.attempt.bound_to(old.binding(), work)
                    && old.binding().facts().run_id.as_ref() == run_id
            })
            .cloned();
        let record = state.records.get(approval_id).cloned();
        drop(state);
        // A reader may have made a disposer's try-lock fail; settle after it.
        self.settle_owner_now().run();
        let Some(candidate) = candidate else {
            return ConsentRebind::Refused(ConsentRefusal::without_custody(record.as_ref()));
        };
        let now = self.now();
        let Some(record) = record.filter(|record| {
            record.status == ApprovalStatus::Approved
                && !candidate.expired()
                && record.expires_at.is_none_or(|expires_at| expires_at > now)
                && record
                    .consent
                    .as_ref()
                    .is_some_and(|consent| consent.consumed_at.is_none())
        }) else {
            return ConsentRebind::Refused(ConsentRefusal::NoConsent);
        };
        // 2. Outside every lock: the same durable execution, decided afresh
        // by the run's own owners.
        let same = old
            .tool_dispatch_parts()
            .is_some_and(|(_, retained)| retained.same_durable_execution(&fresh));
        let (reason, refusal) = if same {
            match old.with_reresolved_plan(work.clone(), fresh) {
                Ok(admitted)
                    if admitted.review_tier() == crate::authorization::OperationReviewTier::R3 =>
                {
                    return self.commit_rebind(approval_id, &candidate, &record, admitted, work);
                }
                // No longer the reviewed R3 operation the human consented to.
                Ok(_) => (
                    ReviewRetirementReason::ContextChanged,
                    ConsentRefusal::ContextChanged,
                ),
                // An owner change in progress, or one published during the
                // fresh preparation, decides nothing (ADR-001 D9): not ready,
                // and the run's next boundary rebinds afresh.
                Err(OperationAuthorizationError::Refused(refusal))
                    if refusal.kind() == OperationRefusalKind::ReprepareRequired =>
                {
                    return ConsentRebind::NotReady;
                }
                Err(OperationAuthorizationError::Refused(refusal))
                    if refusal.kind() == OperationRefusalKind::Denied =>
                {
                    (
                        ReviewRetirementReason::AuthorityChanged,
                        ConsentRefusal::AuthorityChanged {
                            check: old.clone(),
                            error: OperationAuthorizationError::Refused(refusal),
                        },
                    )
                }
                Err(OperationAuthorizationError::Refused(_)) => (
                    ReviewRetirementReason::ContextChanged,
                    ConsentRefusal::ContextChanged,
                ),
                // Readiness or observation unavailable: still spendable.
                Err(_) => return ConsentRebind::NotReady,
            }
        } else {
            (
                ReviewRetirementReason::ContextChanged,
                ConsentRefusal::ContextChanged,
            )
        };
        self.try_retire_consent(approval_id, &candidate, reason)
            .run();
        ConsentRebind::Refused(refusal)
    }

    /// Step 3 of [`Self::rebind_consent_to_reresolved_plan`]: replace the
    /// same live candidate, on the same row, with its rebound custody.
    fn commit_rebind(
        &self,
        approval_id: &ApprovalId,
        candidate: &Arc<ConsentCandidate>,
        record: &ApprovalRecord,
        admitted: PreparedOperationCheck,
        work: &WorkAuthorizationContext,
    ) -> ConsentRebind {
        let Some(mut state) = self.state.try_write() else {
            return ConsentRebind::NotReady;
        };
        self.count_review_lock();
        let rebound = is_live(&state, approval_id, candidate)
            && !state.parked.contains(approval_id)
            && state.records.get(approval_id) == Some(record);
        let outcome = if rebound {
            state.consent.insert(
                approval_id.clone(),
                Arc::new(candidate.rebound(admitted, work.clone(), candidate.material.clone())),
            );
            ConsentRebind::Rebound
        } else {
            ConsentRebind::Refused(ConsentRefusal::without_custody(
                state.records.get(approval_id),
            ))
        };
        self.release_state(state).run();
        outcome
    }

    /// Spend one authenticated consent once at the candidate's physical
    /// entry, under ONE non-blocking reservation: identity, the retained
    /// decision's coherent currentness and validity are rechecked there; then
    /// the generated owner's linked spend and the stored row's consumption
    /// commit together, conditional on the row the owner checked. The spent
    /// attempt stays registered (`Used`) for the entered effect until the
    /// admitting dispatch ends; a refusal after the commit releases it at
    /// once (the caller). A proven no-commit (contention, a store that wrote
    /// nothing) leaves the consent spendable. A conflicting or uncertain
    /// commit retires the custody (no entry, no refund) and marks the row for
    /// reconciliation. A related change, another operation's decision or
    /// expiry withdraws the candidate (fresh consent).
    ///
    /// What the attempt decided afresh before the reservation, outside every
    /// lock (ADR-001 D9), comes in `rechecks`. An entering decision that is
    /// not the retained one but its staging's re-preparation of the same
    /// binding after an owner publication change comes with its
    /// re-adjudication: found unchanged and current under the reservation, it
    /// is held and spent; found changed, the candidate is withdrawn. An
    /// entering decision that is not current under the reservation and was
    /// not settled so spends nothing, and the consent stays for its next
    /// attempt (`OwnerUnavailable`); one its own check refuses withdraws the
    /// candidate (its staging found it current moments before). The
    /// approver's eligibility is checked by evidence alone: the evidence the
    /// allowance holds, or the eligibility owner's fresh answer when that
    /// went stale. Its refusal withdraws the candidate with zero entry
    /// (`AuthorityChanged`, answered as fresh consent required), and evidence
    /// stale again since spends nothing and keeps the consent.
    ///
    /// Every store and owner-exit step (this spend's write, queued
    /// withdrawals the release records) completes before the final check:
    /// the entered decision's callback-free current check and the deadline
    /// run again after the release, immediately before the effect handoff.
    /// A refusal there has zero effect; the committed spend stays spent, with
    /// no refund and no resend (`spent` is then true).
    ///
    /// The returned projections are settled reports only; the caller adds
    /// the `Used` projection and delivers it after the effect.
    pub(crate) fn consume_consent_for_entry(
        &self,
        approval_id: &ApprovalId,
        candidate: &Arc<ConsentCandidate>,
        entering: &PreparedOperationCheck,
        rechecks: &SpendRechecks,
    ) -> (Result<(), ConsentRefusal>, bool, ConsentProjections) {
        let mut projections = ConsentProjections::default();
        if self.ensure_available().is_err() {
            return (Err(ConsentRefusal::OwnerUnavailable), false, projections);
        }
        projections.absorb_reports(self.settle_queued_disposals_now());
        let Some(mut state) = self.state.try_write() else {
            return (Err(ConsentRefusal::OwnerUnavailable), false, projections);
        };
        self.count_review_lock();
        let committed = self.consume_consent_locked(
            &mut state,
            approval_id,
            candidate,
            entering,
            rechecks,
            self.now(),
        );
        projections.absorb_reports(self.release_state(state));
        let spent = committed.is_ok();
        // A committed spend held exactly the entering decision.
        let result = committed.and_then(|expires_at| match retained_current_failure(entering) {
            Some(FinalCheckFailure::AuthorityChanged(error)) => {
                Err(ConsentRefusal::AuthorityChanged {
                    check: entering.clone(),
                    error,
                })
            }
            Some(_) => Err(ConsentRefusal::ContextChanged),
            None if candidate.expired()
                || expires_at.is_some_and(|expires_at| expires_at <= self.now()) =>
            {
                Err(ConsentRefusal::NoConsent)
            }
            None => Ok(()),
        });
        (result, spent, projections)
    }

    fn consume_consent_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        candidate: &Arc<ConsentCandidate>,
        entering: &PreparedOperationCheck,
        rechecks: &SpendRechecks,
        now: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, ConsentRefusal> {
        if !is_live(state, approval_id, candidate) {
            return Err(ConsentRefusal::without_custody(
                state.records.get(approval_id),
            ));
        }
        // A decided allowance is spendable only once its decision published
        // it, and a parked one only once a transfer publishes it for a
        // resumed run.
        if state.unpublished.contains(approval_id) || state.parked.contains(approval_id) {
            return Err(ConsentRefusal::NoConsent);
        }
        // The entering decision is the retained one, or one of its binding
        // and work that its staging prepared afresh after an owner
        // publication change. Any other decision is no longer the reviewed
        // operation: the candidate is withdrawn under this reservation.
        if !entering.same_operation_and_work(&candidate.admitted()) {
            self.withdraw_consent_locked(
                state,
                approval_id,
                ReviewRetirementReason::ContextChanged,
            );
            return Err(ConsentRefusal::ContextChanged);
        }
        // The shared currentness rule, by the prepared owners' coherent
        // checks alone: nothing here re-prepares or observes (the entering
        // decision's full current check ran at its staging, and its
        // re-adjudication before this reservation, outside every lock). Its
        // own check refusing it here is the owners' refusal.
        match self.current_consent_locked(
            state,
            approval_id,
            candidate,
            rechecks.readjudication.as_ref(),
            HeldRefusal::Withdraw,
        ) {
            ConsentCurrentness::Current => {}
            ConsentCurrentness::Withdrawn(Some((check, error))) => {
                return Err(ConsentRefusal::AuthorityChanged { check, error });
            }
            ConsentCurrentness::Withdrawn(None) => return Err(ConsentRefusal::ContextChanged),
            // Stale, and not settled for this entry: nothing is spent, and
            // the consent stays for its next attempt.
            ConsentCurrentness::Readjudicate | ConsentCurrentness::Unsettled => {
                return Err(ConsentRefusal::OwnerUnavailable);
            }
        }
        // Spent only for the decision it enters with, which the rule above
        // holds when it re-adjudicated it unchanged.
        if !entering.same_check(&candidate.admitted()) {
            return Err(ConsentRefusal::OwnerUnavailable);
        }
        let Some(record) = state.records.get(approval_id).cloned() else {
            return Err(ConsentRefusal::NoConsent);
        };
        if record.status != ApprovalStatus::Approved {
            return Err(ConsentRefusal::without_custody(Some(&record)));
        }
        // Validity at use: an approved consent can still expire before entry.
        if candidate.expired()
            || record
                .expires_at
                .is_some_and(|expires_at| expires_at <= now)
        {
            self.withdraw_consent_locked(
                state,
                approval_id,
                ReviewRetirementReason::DeadlineExpired,
            );
            return Err(ConsentRefusal::NoConsent);
        }
        // The approver's eligibility, by evidence alone (callback-free): the
        // evidence the allowance holds, or the eligibility owner's fresh
        // answer before this reservation when that went stale (ADR-001 D9).
        // Its refusal (the approver revoked, or forbidden by policy, since
        // the decision) withdraws the consent with zero entry; an owner that
        // could not say then, or evidence stale again since, spends nothing,
        // and the consent stays for its next attempt.
        if let Some(approval) = candidate.approval() {
            let evidence = match &rechecks.eligibility {
                Some(EligibilityRecheck::Refused) => {
                    self.withdraw_consent_locked(
                        state,
                        approval_id,
                        ReviewRetirementReason::AuthorityChanged,
                    );
                    return Err(ConsentRefusal::NoConsent);
                }
                // The eligibility owner cannot say now: nothing is spent.
                Some(EligibilityRecheck::Unsettled) => {
                    return Err(ConsentRefusal::OwnerUnavailable);
                }
                Some(EligibilityRecheck::Fresh(evidence)) => Arc::clone(evidence),
                None => Arc::clone(&approval.eligibility),
            };
            if evidence.check_current().is_err() {
                return Err(ConsentRefusal::OwnerUnavailable);
            }
            candidate.approve(ConsentApproval {
                eligibility: evidence,
                ..approval
            });
        }
        let attempt_id = candidate.attempt.id().to_owned();
        let mut authority = state.authority.clone();
        // The spent attempt stays registered (`Used`), the entered state of
        // the effect, until the admitting dispatch ends and releases it.
        if review_status_from_outcome(
            authority.consume_consent_for_entry(approval_id.to_string(), attempt_id),
        ) != Ok(ReviewAttemptStatus::Used)
        {
            return Err(ConsentRefusal::NoConsent);
        }
        let mut consumed = record.clone();
        consumed.updated_at = now;
        consumed.consent = Some(ApprovalConsentRecord {
            consumed_at: Some(now),
            ..record.consent.clone().unwrap_or_default()
        });
        match self.commit_row_locked(state, Some(&record), &consumed) {
            Ok(()) => {}
            // The store row changed or may have changed: the custody is
            // already retired and the row awaits reconciliation.
            Err(ApprovalError::StoreConflict { .. }) => return Err(ConsentRefusal::Conflict),
            Err(ApprovalError::StoreCommitUnknown { .. }) => {
                return Err(ConsentRefusal::CommitUnknown);
            }
            // Nothing committed: the consent stays spendable.
            Err(_) => return Err(ConsentRefusal::OwnerUnavailable),
        }
        state.authority = authority;
        let expires_at = consumed.expires_at;
        state.records.insert(approval_id.clone(), consumed);
        state.consent.remove(approval_id);
        Ok(expires_at)
    }

    /// Withdraw a retained candidate without waiting for the owner: the
    /// entering decision is no longer the reviewed one. When the owner is
    /// held, nothing changes; every later spend rechecks the same identity
    /// and currentness under the reservation and withdraws it then.
    pub(crate) fn try_retire_consent(
        &self,
        approval_id: &ApprovalId,
        candidate: &Arc<ConsentCandidate>,
        reason: ReviewRetirementReason,
    ) -> SettledReports {
        if self.ensure_available().is_err() {
            return SettledReports::default();
        }
        let Some(mut state) = self.state.try_write() else {
            return SettledReports::default();
        };
        self.count_review_lock();
        if is_live(&state, approval_id, candidate) {
            self.withdraw_consent_locked(&mut state, approval_id, reason);
        }
        self.release_state(state)
    }

    /// Withdraw the unspent consent of `approval_id` under a reservation
    /// already held: the generated owner cancels a pending approval (never a
    /// denial) or keeps approved or expired history, and retires the linked
    /// attempt with `reason`. The custody is gone at once, so nothing can
    /// spend it even if persistence or notification fails. The withdrawal
    /// is then recorded durably; only a confirmed record publishes the
    /// `Withdrawn` notice. Otherwise the row is marked for reconciliation
    /// (callers check that mark before using the row further) and the
    /// withdrawal stays owed until it settles against the reconciled row.
    pub(super) fn withdraw_consent_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        reason: ReviewRetirementReason,
    ) {
        self.withdraw_consent_with_cause_locked(state, approval_id, reason, reason.into());
    }

    /// [`Self::withdraw_consent_locked`], recording `cause` as the durable
    /// withdrawal cause: the attempt retires with `reason`, and a more
    /// specific cause, such as
    /// [`super::ApprovalConsentWithdrawalReason::NoticeInputEnded`], is
    /// committed with the owed `Withdrawn` notice in one conditional write.
    /// Every earlier notice and its receipt stay as they were.
    pub(super) fn withdraw_consent_with_cause_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        reason: ReviewRetirementReason,
        cause: super::ApprovalConsentWithdrawalReason,
    ) {
        if let Some(mut pending) = withdraw_custody_locked(state, approval_id, reason) {
            pending.cause = cause;
            self.record_withdrawal_locked(state, pending);
        }
    }

    /// The run that admitted candidates under `work` closed: park every
    /// unspent consent it admitted that a resumed run of that work can take
    /// over, and withdraw the rest (`RunEnded`). Never waits for the owner:
    /// when it is held the close is queued, and the holder's release handoff
    /// drains it before unlocking (see [`ApprovalService::release_with`]).
    pub(crate) fn close_consent_for_work(&self, work: &WorkAuthorizationContext) {
        if self.ensure_available().is_err() {
            return;
        }
        self.queued_withdrawals.lock().push(work.clone());
        if let Some(state) = self.state.try_write() {
            self.count_review_lock();
            self.release_state(state).run();
        }
    }

    /// First stage of a run close, in memory only (any owner path). Every
    /// live candidate admitted under `work` is either parked (custody kept,
    /// nothing can spend it) or has its custody retired. The durable record
    /// of either waits in the owner state for a store-capable holder.
    pub(super) fn retire_work_custody_locked(
        &self,
        state: &mut ApprovalServiceState,
        work: &WorkAuthorizationContext,
    ) {
        // A resumed run closed: parked consent its staged inputs carried by
        // committed receipts but never took over (its owners unavailable, or
        // an owner change racing every move of its boundaries, or a move that
        // committed nothing) has no other run to reach it, and is withdrawn.
        if let Some(position) = state
            .resumed_runs
            .iter()
            .position(|open| open.work.same_context(work))
        {
            state.resumed_runs.remove(position);
            // A receipt naming one of its inputs may still come for parked
            // consent of the work that input resumed, for this run's
            // destination: it then arrives after the input's only run.
            if let Some(scope) = work.consent_scope() {
                for resumed in work.resumed_inputs() {
                    let awaited = state.parked.iter().any(|approval_id| {
                        state
                            .records
                            .get(approval_id)
                            .is_some_and(|record| awaits_receipt(record, &resumed.resumes, scope))
                    });
                    if awaited {
                        state.closed_inputs.push(ClosedInput {
                            input_id: resumed.input_id.to_string(),
                            resumes: resumed.resumes.clone(),
                            scope: scope.clone(),
                        });
                    }
                }
            }
            let stranded: Vec<ApprovalId> = state
                .parked
                .iter()
                .filter(|approval_id| {
                    state
                        .records
                        .get(*approval_id)
                        .is_some_and(|record| transfer_input(record, work).is_some())
                })
                .cloned()
                .collect();
            for approval_id in stranded {
                if let Some(pending) =
                    withdraw_custody_locked(state, &approval_id, ReviewRetirementReason::RunEnded)
                {
                    state.pending_withdrawals.push(pending);
                    self.withdrawals_pending
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }
        let admitted: Vec<ApprovalId> = state
            .consent
            .iter()
            .filter(|(approval_id, candidate)| {
                candidate.attempt.admitted_under(work) && !state.parked.contains(*approval_id)
            })
            .map(|(approval_id, _)| approval_id.clone())
            .collect();
        for approval_id in admitted {
            if self.parkable_locked(state, &approval_id, work) {
                state.parked.insert(approval_id.clone());
                state.pending_parks.push(approval_id);
                self.withdrawals_pending
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                continue;
            }
            if let Some(pending) =
                withdraw_custody_locked(state, &approval_id, ReviewRetirementReason::RunEnded)
            {
                state.pending_withdrawals.push(pending);
                self.withdrawals_pending
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    /// Whether the live candidate of `approval_id`, admitted under `work`, can
    /// be parked as its run closes: a resumed run of that exact work could
    /// take it over. Its row must be routable and record that origin, be
    /// pending or approved within its validity, and be unspent, unwithdrawn,
    /// published and reconciled.
    fn parkable_locked(
        &self,
        state: &ApprovalServiceState,
        approval_id: &ApprovalId,
        work: &WorkAuthorizationContext,
    ) -> bool {
        let Some(origin) = work.retained_work() else {
            return false;
        };
        let Some(candidate) = state.consent.get(approval_id) else {
            return false;
        };
        let now = self.now();
        !candidate.expired()
            && !state.unpublished.contains(approval_id)
            && !state.unreconciled.contains_key(approval_id)
            && state.records.get(approval_id).is_some_and(|record| {
                matches!(
                    record.status,
                    ApprovalStatus::Pending | ApprovalStatus::Approved
                ) && record.expires_at.is_none_or(|expires_at| expires_at > now)
                    && record.consent.as_ref().is_some_and(|consent| {
                        consent.routable()
                            && consent.consumed_at.is_none()
                            && consent.withdrawal.is_none()
                            && consent
                                .origin
                                .as_ref()
                                .is_some_and(|recorded| &recorded.retained_work == origin)
                    })
            })
    }

    /// Second stage, from a store-capable path: record every pending park
    /// and every pending withdrawal durably, and queue the notices.
    pub(super) fn record_pending_withdrawals_locked(&self, state: &mut ApprovalServiceState) {
        for approval_id in std::mem::take(&mut state.pending_parks) {
            self.record_park_locked(state, &approval_id);
        }
        for pending in std::mem::take(&mut state.pending_withdrawals) {
            self.record_withdrawal_locked(state, pending);
        }
        self.withdrawals_pending
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Record one park durably: the row's custody becomes `Parked` in one
    /// conditional write. The candidate stays unspendable whatever happens
    /// here. A proven no-commit means nothing was parked, so the candidate
    /// is withdrawn as its run closed, with the withdrawal's own record
    /// semantics. A conflicting or unknown outcome has already retired the
    /// custody and marked the row for reconciliation, and nothing more is
    /// written or promised.
    fn record_park_locked(&self, state: &mut ApprovalServiceState, approval_id: &ApprovalId) {
        // Withdrawn, abandoned or transferred meanwhile: nothing to record.
        if !state.parked.contains(approval_id) {
            return;
        }
        let Some(record) = state.records.get(approval_id).cloned() else {
            state.parked.remove(approval_id);
            self.withdraw_consent_locked(state, approval_id, ReviewRetirementReason::RunEnded);
            return;
        };
        let mut parked = record.clone();
        parked.updated_at = self.now();
        parked.consent = Some(ApprovalConsentRecord {
            custody: Some(ApprovalConsentCustody::Parked),
            ..record.consent.clone().unwrap_or_default()
        });
        match self.commit_row_locked(state, Some(&record), &parked) {
            Ok(()) => {
                state.records.insert(approval_id.clone(), parked);
                self.note_consent_delivery_change();
            }
            // The store row changed or may have changed: the custody is
            // already abandoned and the row awaits reconciliation.
            Err(ApprovalError::StoreConflict { .. } | ApprovalError::StoreCommitUnknown { .. }) => {
                state.parked.remove(approval_id);
            }
            // Nothing committed: nothing was parked.
            Err(_) => {
                state.parked.remove(approval_id);
                self.withdraw_consent_locked(state, approval_id, ReviewRetirementReason::RunEnded);
            }
        }
    }

    /// A run opened under `work` as `run_id`. When it resumes retained work
    /// through staged inputs, it is registered until its close, and every
    /// parked candidate one of those inputs carries by its committed notice
    /// receipt moves into it now; a receipt committed while it runs moves
    /// its candidate then ([`Self::transfer_parked_consent`]). The run's
    /// boundaries read what moved ([`Self::consent_moved_into_run`]). A
    /// context whose identity names another run moves nothing.
    pub(crate) fn open_consent_for_run(&self, work: &WorkAuthorizationContext, run_id: &RunId) {
        if work.resumed_inputs().is_empty()
            || work
                .retained_work()
                .is_none_or(|identity| identity.run_id() != run_id)
            || self.ensure_available().is_err()
        {
            return;
        }
        let mut state = self.state.write();
        self.count_review_lock();
        if !state
            .resumed_runs
            .iter()
            .any(|open| open.work.same_context(work))
        {
            state.resumed_runs.push(ResumedRun {
                work: work.clone(),
                run_id: run_id.clone(),
                moved: std::collections::BTreeSet::new(),
            });
        }
        self.release_state(state).run();
        self.transfer_parked_consent();
    }

    /// Move every parked candidate whose committed notice receipt names a
    /// staged input of an open resumed run into that run. Each move stays
    /// unspendable through its preparation and its store commit: the resumed
    /// run's own owners prepare the exact original operation and read its
    /// review material outside every owner lock, one conditional row write
    /// records the transfer, and only a confirmed write publishes the moved
    /// custody. Their refusal of either withdraws the candidate by the
    /// refusal's category, an action account other than the one the
    /// approval presented withdraws it as `ContextChanged`, and owners that
    /// cannot answer now (unavailable, or an owner change in progress or
    /// published during their reads) leave it parked: the resumed run's
    /// boundaries move it later ([`Self::retry_parked_consent_for_run`]),
    /// and its close settles what never moved. A proven no-commit keeps the
    /// candidate parked; a conflicting or unknown outcome retires its
    /// custody for reconciliation, and nothing executes.
    pub(crate) fn transfer_parked_consent(&self) {
        if self.ensure_available().is_err() {
            return;
        }
        let ready = pending_transfers(&self.state.read(), None);
        // A reader may have made a disposer's try-lock fail; settle after it.
        self.settle_owner_now().run();
        for transfer in ready {
            let prepared = prepare_move(&transfer);
            let mut state = self.state.write();
            self.count_review_lock();
            self.commit_transfer_locked(&mut state, transfer, prepared);
            self.release_state(state).run();
        }
    }

    /// At a boundary of the resumed run `run_id` of `work`: move into it the
    /// parked candidates its staged inputs' committed receipts name that an
    /// earlier move left parked (its owners could not answer then, or an
    /// owner change was in progress: ADR-001 D9), exactly as
    /// [`Self::transfer_parked_consent`] moves them. It never waits for the
    /// owner (boundaries run on async threads): a held owner leaves them for
    /// the run's next boundary, and the run's close settles what never
    /// moved. Nothing is read or prepared when none waits.
    pub(crate) fn retry_parked_consent_for_run(
        &self,
        work: &WorkAuthorizationContext,
        run_id: &RunId,
    ) {
        if work.resumed_inputs().is_empty() || self.ensure_available().is_err() {
            return;
        }
        let Some(state) = self.state.try_read() else {
            return;
        };
        let ready = pending_transfers(&state, Some((work, run_id)));
        drop(state);
        // A reader may have made a disposer's try-lock fail; settle after it.
        self.settle_owner_now().run();
        for transfer in ready {
            let prepared = prepare_move(&transfer);
            let Some(mut state) = self.state.try_write() else {
                return;
            };
            self.count_review_lock();
            self.commit_transfer_locked(&mut state, transfer, prepared);
            self.release_state(state).run();
        }
    }

    fn commit_transfer_locked(
        &self,
        state: &mut ApprovalServiceState,
        transfer: PendingTransfer,
        prepared: MovePreparation,
    ) {
        let PendingTransfer {
            approval_id,
            candidate,
            record,
            run,
            input_id,
        } = transfer;
        // Still the same parked candidate on the same row, and its resumed
        // run still open.
        if !is_live(state, &approval_id, &candidate)
            || !state.parked.contains(&approval_id)
            || state.records.get(&approval_id) != Some(&record)
            || !state
                .resumed_runs
                .iter()
                .any(|open| open.work.same_context(&run.work))
        {
            return;
        }
        // The resumed run's owners refuse the exact operation, or its review
        // material: settled here, with the refusal's own category. An owner
        // change in progress or published during their reads, or readiness
        // or observation unavailable, decides nothing (ADR-001 D9): still
        // parked, and a later move, at the resumed run's next boundary, reads
        // both afresh (the run's close settles it unless one succeeds); the
        // move never carries the original material forward.
        let (admitted, material) = match prepared {
            Ok((admitted, Ok(material))) => (admitted, material),
            Ok((_, Err(OperationAuthorizationError::Refused(refusal))))
            | Err(OperationAuthorizationError::Refused(refusal))
                if refusal.kind() != OperationRefusalKind::ReprepareRequired =>
            {
                self.withdraw_consent_locked(
                    state,
                    &approval_id,
                    move_refusal_reason(refusal.kind()),
                );
                return;
            }
            Ok((_, Err(_))) | Err(_) => return,
        };
        // The account the approval presented binds its consent: one the
        // resumed run's owners read otherwise (rebound while parked)
        // withdraws it, with zero entry. Only the mandate, which may name
        // the resumed run's own requester, is taken from their read.
        if action_account(&material) != action_account(&candidate.material) {
            self.withdraw_consent_locked(
                state,
                &approval_id,
                ReviewRetirementReason::ContextChanged,
            );
            return;
        }
        // Validity at the move: an expired candidate never moves, and is
        // withdrawn.
        let now = self.now();
        if candidate.expired()
            || record
                .expires_at
                .is_some_and(|expires_at| expires_at <= now)
        {
            self.withdraw_consent_locked(
                state,
                &approval_id,
                ReviewRetirementReason::DeadlineExpired,
            );
            return;
        }
        let mut transferred = record.clone();
        transferred.updated_at = now;
        transferred.consent = Some(ApprovalConsentRecord {
            custody: Some(ApprovalConsentCustody::Transferred { input_id }),
            ..record.consent.clone().unwrap_or_default()
        });
        // A conflicting or unknown outcome has already retired the custody
        // for reconciliation, and a proven no-commit keeps it parked: only a
        // confirmed write publishes the moved custody.
        if self
            .commit_row_locked(state, Some(&record), &transferred)
            .is_ok()
        {
            state.records.insert(approval_id.clone(), transferred);
            state.parked.remove(&approval_id);
            prune_closed_inputs(state);
            // The owner's own binding of the moved consent to this run's
            // work, run and receipt input, which the run's boundaries read.
            if let Some(open) = state
                .resumed_runs
                .iter_mut()
                .find(|open| open.work.same_context(&run.work) && open.run_id == run.run_id)
            {
                open.moved.insert(approval_id.clone());
            }
            state.consent.insert(
                approval_id,
                Arc::new(candidate.rebound(admitted, run.work, material)),
            );
        }
    }

    /// Record one withdrawal whose custody is already retired. Its full
    /// pending value (the exact cause, the attempt's retirement and the owed
    /// `Withdrawn` notice) is owed before the first conditional write and
    /// settles only through [`Self::settle_reconciled_withdrawal_locked`]:
    /// at once against a known row, or, when that write fails before or
    /// after it committed or the row's stored form is unknown, against the
    /// row its reconciliation confirms. Only a confirmed record announces
    /// the notice.
    fn record_withdrawal_locked(
        &self,
        state: &mut ApprovalServiceState,
        pending: PendingWithdrawal,
    ) {
        let approval_id = pending.approval_id.clone();
        state
            .unreconciled_withdrawals
            .insert(approval_id.clone(), pending);
        if !state.unreconciled.contains_key(&approval_id) {
            // A failed write marks the row and keeps the withdrawal owed;
            // callers check that mark before using the row further.
            let _ = self.settle_reconciled_withdrawal_locked(state, &approval_id);
        }
    }

    /// The reservation's own settlement of the candidate of `approval_id` on
    /// an approval read: an approved parked candidate past its deadline is
    /// withdrawn. An unparked candidate's decision that is no longer current
    /// is settled by its re-adjudication, which approval reads and decisions
    /// run before their reservation ([`Self::readjudicate_consent`]):
    /// callback-free, nothing here withdraws it for that alone (ADR-001 D9).
    pub(super) fn withdraw_stale_consent_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
    ) {
        let Some(candidate) = state.consent.get(approval_id).cloned() else {
            return;
        };
        // A parked candidate's retained decision belongs to its closed run by
        // design; its transfer prepares the resumed run's own decision. Its
        // validity still applies: an approved parked candidate past its
        // deadline is withdrawn here (a pending one expires with its
        // approval).
        if state.parked.contains(approval_id) {
            let now = self.now();
            let lapsed = state.records.get(approval_id).is_some_and(|record| {
                record.status == ApprovalStatus::Approved
                    && (candidate.expired()
                        || record
                            .expires_at
                            .is_some_and(|expires_at| expires_at <= now))
            });
            if lapsed {
                self.withdraw_consent_locked(
                    state,
                    approval_id,
                    ReviewRetirementReason::DeadlineExpired,
                );
            }
        }
    }

    /// The one currentness rule of a live, unparked candidate, under the
    /// owner's reservation (ADR-001 D9), shared by approval reads and
    /// decisions ([`Self::readjudicate_consent`]), a decision's publication,
    /// an attempt's admission ([`Self::settle_consent_for_attempt`]) and the
    /// spend. Callback-free: the prepared owners' coherent checks only.
    ///
    /// A current decision stands. One that is no longer current (stale
    /// because the owner publication moved, or refused by its own check, as
    /// when its owner's validity window for it ended) is never withdrawn for
    /// that alone: its `readjudication`, made outside the lock, decides it.
    /// A decision found unchanged that is current now is held from now on, a
    /// related change or the owners' refusal withdraws the consent, and an
    /// unsettled one changes nothing. Without one, or when another change
    /// met it, the caller re-adjudicates and applies the rule again
    /// ([`ConsentCurrentness::Readjudicate`]), within the bound, past which
    /// nothing changes. Only the spend (`held_refusal` is
    /// [`HeldRefusal::Withdraw`]) withdraws a decision its own check refuses
    /// as `AuthorityChanged` at once: its staging found it current moments
    /// before.
    fn current_consent_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        candidate: &ConsentCandidate,
        readjudication: Option<&Readjudication>,
        held_refusal: HeldRefusal,
    ) -> ConsentCurrentness {
        // A decision found unchanged that is current now is held. Every
        // decision this custody ever held shares its binding, review tier and
        // material, so it stands even if another re-adjudication replaced the
        // held one meanwhile. One its owners refuse at once is their answer.
        if let Some(Readjudication::Unchanged(fresh)) = readjudication {
            match retained_current_failure(fresh) {
                None => {
                    candidate.hold(fresh);
                    return ConsentCurrentness::Current;
                }
                Some(FinalCheckFailure::AuthorityChanged(error)) => {
                    self.withdraw_consent_locked(
                        state,
                        approval_id,
                        ReviewRetirementReason::AuthorityChanged,
                    );
                    return ConsentCurrentness::Withdrawn(Some((fresh.clone(), error)));
                }
                // Another change since: the held decision decides below.
                Some(_) => {}
            }
        }
        let retained = candidate.admitted();
        match retained_current_failure(&retained) {
            None => return ConsentCurrentness::Current,
            Some(FinalCheckFailure::AuthorityChanged(error))
                if held_refusal == HeldRefusal::Withdraw =>
            {
                self.withdraw_consent_locked(
                    state,
                    approval_id,
                    ReviewRetirementReason::AuthorityChanged,
                );
                return ConsentCurrentness::Withdrawn(Some((retained, error)));
            }
            Some(_) => {}
        }
        match readjudication {
            Some(Readjudication::Changed(reason)) => {
                self.withdraw_consent_locked(state, approval_id, *reason);
                ConsentCurrentness::Withdrawn(None)
            }
            Some(Readjudication::Refused(error)) => {
                self.withdraw_consent_locked(
                    state,
                    approval_id,
                    ReviewRetirementReason::AuthorityChanged,
                );
                ConsentCurrentness::Withdrawn(Some((retained, *error)))
            }
            Some(Readjudication::Unsettled) => ConsentCurrentness::Unsettled,
            Some(Readjudication::Unchanged(_) | Readjudication::Moved) | None => {
                ConsentCurrentness::Readjudicate
            }
        }
    }

    /// Re-adjudicate the live, unparked candidate of `approval_id` before an
    /// approval read (`step` is [`ReadjudicationStep::Read`]) or an approval,
    /// when its decision is no longer current (ADR-001 D9): stale because
    /// the owner publication moved, or, for an approval only, refused by its
    /// own check, as when its owner's validity window for it ended. Its
    /// owners decide the same binding afresh and read its material outside
    /// every owner lock, on the caller's thread, and the shared rule
    /// ([`Self::current_consent_locked`]) settles it under the reservation.
    /// Nothing calls its owners while its own check answers that they are
    /// unavailable, so a polling read adds none of their audit rows. It
    /// never waits for an owner change, and never withdraws for the bound
    /// alone: a change that keeps meeting the re-adjudication for
    /// [`CONSENT_READJUDICATION_BOUND`] rounds leaves the candidate as it is.
    /// Whether no decision that is not current is left: `false` when the
    /// candidate stays live with one this did not settle (an approval then
    /// records nothing).
    pub(super) fn readjudicate_consent(
        &self,
        approval_id: &ApprovalId,
        step: ReadjudicationStep,
    ) -> bool {
        let candidate = {
            let state = self.state.read();
            state
                .consent
                .get(approval_id)
                .filter(|_| !state.parked.contains(approval_id))
                .cloned()
        };
        // A reader may have made a disposer's try-lock fail; settle after it.
        self.settle_owner_now().run();
        let Some(candidate) = candidate else {
            return true;
        };
        for _ in 0..CONSENT_READJUDICATION_BOUND {
            let retained = candidate.admitted();
            match held_check(&retained, step) {
                HeldCheck::Current => return true,
                HeldCheck::Leave => return false,
                HeldCheck::Readjudicate => {}
            }
            let readjudication = candidate.readjudicate(&retained, None);
            let mut state = self.state.write();
            let settled = (is_live(&state, approval_id, &candidate)
                && !state.parked.contains(approval_id))
            .then(|| {
                self.current_consent_locked(
                    &mut state,
                    approval_id,
                    &candidate,
                    Some(&readjudication),
                    HeldRefusal::Readjudicate,
                )
            });
            self.release_state(state).run();
            match settled {
                Some(ConsentCurrentness::Readjudicate) => {}
                Some(ConsentCurrentness::Unsettled) => return false,
                None | Some(ConsentCurrentness::Current | ConsentCurrentness::Withdrawn(_)) => {
                    return true;
                }
            }
        }
        false
    }

    /// Settle the held decision of `candidate` before an attempt is handed it
    /// (ADR-001 D9): a decision that is not current (stale because the owner
    /// publication moved, or refused by its own check, as when its owner's
    /// validity window for it ended while the human decided) is decided
    /// afresh by its owners outside every lock, and the shared currentness
    /// rule ([`Self::current_consent_locked`]) settles it under the
    /// reservation, taken without waiting (attempts run on async threads). A
    /// decision found unchanged and current is held, and the attempt goes
    /// on with it. A related change withdraws the consent and refuses the
    /// attempt as `ContextChanged`; the owners' refusal withdraws it as
    /// `AuthorityChanged` and refuses the attempt with that refusal. While
    /// nothing can be settled now (the reservation is held, an owner change
    /// is in progress, the owners are unavailable, or changes met every
    /// round within [`CONSENT_READJUDICATION_BOUND`]) nothing changes: the
    /// attempt refuses as `OwnerUnavailable` (a run's own attempt counts as
    /// not made) and the consent is kept. An attempt is never handed a
    /// decision that is not current.
    pub(crate) fn settle_consent_for_attempt(
        &self,
        approval_id: &ApprovalId,
        candidate: &Arc<ConsentCandidate>,
    ) -> Result<(), ConsentRefusal> {
        for _ in 0..CONSENT_READJUDICATION_BOUND {
            let retained = candidate.admitted();
            let readjudication = match held_check(&retained, ReadjudicationStep::Decision) {
                HeldCheck::Current => return Ok(()),
                HeldCheck::Leave => return Err(ConsentRefusal::OwnerUnavailable),
                HeldCheck::Readjudicate => candidate.readjudicate(&retained, None),
            };
            let Some(mut state) = self.state.try_write() else {
                return Err(ConsentRefusal::OwnerUnavailable);
            };
            self.count_review_lock();
            let settled =
                if is_live(&state, approval_id, candidate) && !state.parked.contains(approval_id) {
                    Some(self.current_consent_locked(
                        &mut state,
                        approval_id,
                        candidate,
                        Some(&readjudication),
                        HeldRefusal::Readjudicate,
                    ))
                } else {
                    None
                };
            let record = state.records.get(approval_id).cloned();
            self.release_state(state).run();
            match settled {
                Some(ConsentCurrentness::Current) => return Ok(()),
                Some(ConsentCurrentness::Readjudicate) => {}
                Some(ConsentCurrentness::Unsettled) => {
                    return Err(ConsentRefusal::OwnerUnavailable);
                }
                // The owners refused the fresh preparation: their answer,
                // observed where it was made.
                Some(ConsentCurrentness::Withdrawn(Some((_, error))))
                    if matches!(readjudication, Readjudication::Refused(_)) =>
                {
                    return Err(ConsentRefusal::OwnersRefused(error));
                }
                // A fresh decision their own check refused under the
                // reservation: observed once every lock is released.
                Some(ConsentCurrentness::Withdrawn(Some((check, error)))) => {
                    return Err(ConsentRefusal::AuthorityChanged { check, error });
                }
                Some(ConsentCurrentness::Withdrawn(None)) => {
                    return Err(ConsentRefusal::ContextChanged);
                }
                // Spent, declined, withdrawn or parked meanwhile.
                None => return Err(ConsentRefusal::without_custody(record.as_ref())),
            }
        }
        Err(ConsentRefusal::OwnerUnavailable)
    }

    /// Retire the custody of `approval_id` after a conflicting or uncertain
    /// store commit: the generated owner withdraws it, nothing is written and
    /// nothing is announced. The row is reconciled before reuse.
    pub(super) fn abandon_consent_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
    ) {
        if let Some(pending) =
            withdraw_custody_locked(state, approval_id, ReviewRetirementReason::Abandoned)
        {
            self.queue_attempt_release(&pending.candidate, ReviewRetirementReason::Abandoned, None);
        }
    }

    /// Dispose a withdrawn attempt as this reservation is released, then
    /// report its retirement (and a confirmed withdrawal notice) outside
    /// every lock.
    fn queue_attempt_release(
        &self,
        candidate: &ConsentCandidate,
        reason: ReviewRetirementReason,
        withdrawn: Option<ApprovalId>,
    ) {
        self.queued_disposals.lock().push(QueuedReviewDisposal {
            id: candidate.attempt.id().to_owned(),
            input: QueuedReviewInput::Dispose(None),
            report: candidate.retirement_report(reason, withdrawn),
        });
    }
}

/// A committed consent decision.
/// The `Host` continuation namespace in which the issuing approval owner
/// answers for its consent notices.
pub const CONSENT_DELIVERY_NAMESPACE: &str = "meerkat.consent";

/// The continuation job id of one transition's notice:
/// `{approval_id}/{transition}`.
pub fn consent_delivery_job_id(
    approval_id: &ApprovalId,
    transition: ApprovalConsentTransition,
) -> String {
    format!("{approval_id}/{}", transition.as_str())
}

/// The approval and transition a consent notice job id names, if it names
/// one exactly.
pub fn parse_consent_delivery_job_id(
    job_id: &str,
) -> Option<(ApprovalId, ApprovalConsentTransition)> {
    let (approval_id, transition) = job_id.rsplit_once('/')?;
    let approval_id = approval_id.parse::<ApprovalId>().ok()?;
    let transition = transition.parse::<ApprovalConsentTransition>().ok()?;
    (consent_delivery_job_id(&approval_id, transition) == job_id)
        .then_some((approval_id, transition))
}

/// The exact receipt of one consent notice's delivery, from the delivery
/// composition that owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentDeliveryReceipt {
    /// The notice was admitted as `input_id` into the destination session.
    /// Admitted is not staged, transferred or observed.
    Admitted {
        session_id: crate::SessionId,
        input_id: crate::lifecycle::InputId,
    },
    /// Proven terminal non-delivery, such as a settled policy refusal or a
    /// retired owner; never inferred from a later refusal after an unknown
    /// earlier attempt.
    Failed { reason: String },
}

/// Why a consent notice receipt was not recorded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConsentReceiptError {
    /// This owner committed no such notice (an unknown or unrouted approval,
    /// or a transition that never committed).
    #[error("no consent notice is owed for approval {approval_id} ({})", .transition.as_str())]
    NotOwed {
        approval_id: ApprovalId,
        transition: ApprovalConsentTransition,
    },
    /// The receipt names another session than the notice's destination.
    #[error("the receipt names another destination for approval {approval_id}")]
    WrongDestination { approval_id: ApprovalId },
    /// Another receipt already settled the notice; it stands.
    #[error("the consent notice of approval {approval_id} is already settled")]
    Conflicting {
        approval_id: ApprovalId,
        settled: ApprovalConsentDeliveryState,
    },
    /// The owner or its store: nothing committed, or the outcome is unknown
    /// and the row is reconciled before reuse.
    #[error(transparent)]
    Approval(#[from] ApprovalError),
}

/// What the issuing approval owner committed about one consent notice: the
/// scope (hosting domain and session) and retained work it is owed to, the
/// digest of the committed transition, and where its delivery stands. Facts
/// only: nothing here grants authority, and a governed admission confirms
/// them again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentDeliveryFacts {
    pub scope: super::ApprovalConsentScope,
    pub retained_work: crate::retained_work::RetainedWorkIdentity,
    /// `sha256:` digest of the committed transition: the approval, the
    /// transition and its commit time.
    pub result_digest: String,
    pub state: ApprovalConsentDeliveryState,
}

impl ApprovalService {
    /// What this owner committed about `approval_id`'s `transition` notice.
    /// `Ok(None)` when it committed no such notice: an unknown or unscoped
    /// approval, or a transition that never committed. A row whose stored
    /// form is unknown is unavailable until it is reconciled.
    pub fn consent_delivery(
        &self,
        approval_id: &ApprovalId,
        transition: ApprovalConsentTransition,
    ) -> Result<Option<ConsentDeliveryFacts>, ApprovalError> {
        self.ensure_available()?;
        let state = self.state.read();
        let facts = state.ensure_reconciled(approval_id).map(|()| {
            state.records.get(approval_id).and_then(|record| {
                let consent = record.consent.as_ref()?;
                let delivery = consent.delivery(transition)?;
                Some(ConsentDeliveryFacts {
                    scope: consent.scope.clone()?,
                    retained_work: consent.origin.as_ref()?.retained_work.clone(),
                    result_digest: consent_delivery_digest(
                        approval_id,
                        transition,
                        delivery.committed_at,
                    ),
                    state: delivery.state.clone(),
                })
            })
        });
        drop(state);
        self.settle_owner_now().run();
        facts
    }

    /// Record the exact receipt of `approval_id`'s `transition` notice, in one
    /// conditional row write. Only an owed notice settles, and an admission
    /// only for its own destination; the same receipt again (a retry after a
    /// lost acknowledgement) changes nothing, and any other receipt is
    /// refused while the first stands. A row whose earlier write is unknown
    /// is reconciled first. A receipt is a delivery fact only: it never
    /// revives withdrawn, spent or expired consent.
    ///
    /// For parked consent, an admitted approved notice moves the consent
    /// into the resumed run staged with that input when one is open (or
    /// when it opens); when that input's only run already closed, or the
    /// notice can never be delivered, the parked consent is withdrawn.
    ///
    /// The delivery owner records the receipt. When a governed admission of
    /// the notice commits, the notice's committed job source (the `meerkat`
    /// crate's `ConsentNoticeJobSource`) records `Admitted` for that session
    /// and input before the delivery row is acknowledged, so a retry reports
    /// the same receipt. The projector's rounds record the receipt a
    /// committed row's own status proves when no admission recorded it:
    /// `Admitted` for an applied admission, and `Failed` for a settled
    /// refusal.
    pub fn settle_consent_delivery(
        &self,
        approval_id: &ApprovalId,
        transition: ApprovalConsentTransition,
        receipt: ConsentDeliveryReceipt,
    ) -> Result<ApprovalConsentDeliveryState, ConsentReceiptError> {
        self.ensure_available()?;
        self.reconcile_if_unreconciled(approval_id)?;
        let not_owed = || ConsentReceiptError::NotOwed {
            approval_id: approval_id.clone(),
            transition,
        };
        let mut state = self.state.write();
        self.count_review_lock();
        let settled = self.settle_consent_delivery_locked(
            &mut state,
            approval_id,
            transition,
            receipt,
            not_owed,
        );
        self.release_state(state).run();
        let settled = settled?;
        if transition == ApprovalConsentTransition::Approved
            && matches!(settled, ApprovalConsentDeliveryState::Admitted { .. })
        {
            self.transfer_parked_consent();
        }
        Ok(settled)
    }

    fn settle_consent_delivery_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        transition: ApprovalConsentTransition,
        receipt: ConsentDeliveryReceipt,
        not_owed: impl Fn() -> ConsentReceiptError,
    ) -> Result<ApprovalConsentDeliveryState, ConsentReceiptError> {
        state.ensure_reconciled(approval_id)?;
        let record = state
            .records
            .get(approval_id)
            .cloned()
            .ok_or_else(&not_owed)?;
        let consent = record
            .consent
            .as_ref()
            .filter(|consent| consent.routable())
            .ok_or_else(&not_owed)?;
        let current = consent
            .delivery(transition)
            .map(|delivery| delivery.state.clone())
            .ok_or_else(&not_owed)?;
        let settled = match receipt {
            ConsentDeliveryReceipt::Admitted {
                session_id,
                input_id,
            } => {
                if consent
                    .scope
                    .as_ref()
                    .is_none_or(|scope| scope.session_id != session_id)
                {
                    return Err(ConsentReceiptError::WrongDestination {
                        approval_id: approval_id.clone(),
                    });
                }
                ApprovalConsentDeliveryState::Admitted {
                    session_id,
                    input_id: input_id.to_string(),
                }
            }
            ConsentDeliveryReceipt::Failed { reason } => {
                ApprovalConsentDeliveryState::Failed { reason }
            }
        };
        if current == settled {
            return Ok(settled);
        }
        if current != ApprovalConsentDeliveryState::Owed {
            return Err(ConsentReceiptError::Conflicting {
                approval_id: approval_id.clone(),
                settled: current,
            });
        }
        let mut receipted = record.clone();
        receipted.updated_at = self.now();
        let mut consent = consent.clone();
        if let Some(delivery) = consent
            .deliveries
            .iter_mut()
            .find(|delivery| delivery.transition == transition)
        {
            delivery.state = settled.clone();
        }
        receipted.consent = Some(consent);
        if let Err(error) = self.store.put_if_current(Some(&record), &receipted) {
            if error.leaves_row_unknown() {
                // An unknown receipt write is no proof the admission failed:
                // the consent stops being spendable now, and its withdrawal
                // stays owed until the row is reconciled, then settles
                // against the confirmed row.
                state.mark_unreconciled(approval_id);
                if let Some(pending) =
                    withdraw_custody_locked(state, approval_id, ReviewRetirementReason::Abandoned)
                {
                    state
                        .unreconciled_withdrawals
                        .insert(approval_id.clone(), pending);
                }
            }
            return Err(error.into_commit_error(approval_id).into());
        }
        state.records.insert(approval_id.clone(), receipted.clone());
        // A new receipt can leave its consent parked on an admitted input
        // whose end only the delivery composition can read: announced, so
        // its next round looks.
        self.note_consent_delivery_change();
        // Parked consent this approved notice can no longer reach a run for:
        // the notice is undeliverable, or no open run carries its input and
        // that input's only run for this destination and origin closed. A
        // missing run alone proves nothing.
        if transition == ApprovalConsentTransition::Approved && state.parked.contains(approval_id) {
            let unreachable = match &settled {
                ApprovalConsentDeliveryState::Failed { .. } => true,
                ApprovalConsentDeliveryState::Admitted { input_id, .. } => {
                    receipted.consent.as_ref().is_some_and(|consent| {
                        let carries = |scope: Option<&super::ApprovalConsentScope>,
                                       input: &str,
                                       resumes: &crate::retained_work::RetainedWorkIdentity| {
                            input == input_id.as_str()
                                && scope.is_some_and(|scope| consent.scope.as_ref() == Some(scope))
                                && consent
                                    .origin
                                    .as_ref()
                                    .is_some_and(|origin| origin.retained_work == *resumes)
                        };
                        let open = state.resumed_runs.iter().any(|run| {
                            run.work.resumed_inputs().iter().any(|resumed| {
                                carries(
                                    run.work.consent_scope(),
                                    &resumed.input_id.to_string(),
                                    &resumed.resumes,
                                )
                            })
                        });
                        !open
                            && state.closed_inputs.iter().any(|closed| {
                                carries(Some(&closed.scope), &closed.input_id, &closed.resumes)
                            })
                    })
                }
                ApprovalConsentDeliveryState::Owed => false,
            };
            if unreachable {
                self.withdraw_consent_locked(state, approval_id, ReviewRetirementReason::RunEnded);
            }
        }
        Ok(settled)
    }

    /// Settle the owed withdrawal of `approval_id` against its known row: at
    /// once for an ordinary withdrawal, or, after a failed or uncertain write
    /// (of the withdrawal itself or of a receipt), once the row is
    /// reconciled. Its custody was retired already, so the row records the
    /// withdrawal with its exact cause and owes its own notice, in one
    /// conditional write. The row's status is the generated owner's: the
    /// withdrawal cancelled a pending approval there, and a pending status
    /// that reconciliation restored from the store is cancelled again
    /// through the owner's unlinked consent settlement, committed with this
    /// write; approved and expired history keep their status. A row that
    /// already records this withdrawal (an earlier write whose outcome was
    /// unknown committed) is announced now. A spent row, one that records
    /// another withdrawal, and a denied or cancelled row keep that end and
    /// announce nothing. A write that fails leaves the row unknown again
    /// (`Unreconciled`), the withdrawal still owed.
    pub(super) fn settle_reconciled_withdrawal_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
    ) -> Result<(), ApprovalError> {
        let Some(pending) = state.unreconciled_withdrawals.remove(approval_id) else {
            return Ok(());
        };
        let Some(record) = state.records.get(approval_id).cloned() else {
            self.queue_attempt_release(&pending.candidate, pending.reason, None);
            return Ok(());
        };
        let open = record
            .consent
            .as_ref()
            .is_some_and(|consent| consent.withdrawal.is_none() && consent.consumed_at.is_none());
        if !open {
            let landed = record.consent.as_ref().is_some_and(|consent| {
                consent.consumed_at.is_none()
                    && consent
                        .withdrawal
                        .as_ref()
                        .is_some_and(|withdrawal| withdrawal.reason == pending.cause)
            });
            if landed {
                self.note_consent_delivery_change();
            }
            self.queue_attempt_release(
                &pending.candidate,
                pending.reason,
                landed.then(|| approval_id.clone()),
            );
            return Ok(());
        }
        if matches!(
            record.status,
            ApprovalStatus::Denied | ApprovalStatus::Cancelled
        ) {
            self.queue_attempt_release(&pending.candidate, pending.reason, None);
            return Ok(());
        }
        let id = approval_id.to_string();
        let mut authority = state.authority.clone();
        if authority.status_for(&id) == Some(ApprovalLifecycleStatus::Pending) {
            // Refused only while an attempt is still linked (the withdrawal's
            // own generated transition was refused): the row then keeps the
            // owner's status.
            if let Err(error) = authority.settle_unlinked_consent(id.clone()) {
                state.mark_unreconciled(approval_id);
                state
                    .unreconciled_withdrawals
                    .insert(approval_id.clone(), pending);
                return Err(lifecycle_error(error));
            }
        }
        let status = authority
            .status_for(&id)
            .map_or(record.status, super::approval_status_from_lifecycle);
        let withdrawn = withdrawn_record(&record, status, pending.cause.clone(), self.now());
        match self.store.put_if_current(Some(&record), &withdrawn) {
            Ok(()) => {
                state.authority = authority;
                state.records.insert(approval_id.clone(), withdrawn);
                self.note_consent_delivery_change();
                self.queue_attempt_release(
                    &pending.candidate,
                    pending.reason,
                    Some(approval_id.clone()),
                );
                Ok(())
            }
            // Still owed: the row is reconciled again, and settles then.
            Err(_) => {
                state.mark_unreconciled(approval_id);
                state
                    .unreconciled_withdrawals
                    .insert(approval_id.clone(), pending);
                Err(ApprovalError::Unreconciled {
                    approval_id: approval_id.clone(),
                })
            }
        }
    }

    /// Observe changes of this owner's consent notice obligations: the value
    /// advances after every committed decision, every recorded withdrawal,
    /// every recorded park (which can make a withheld notice deliverable)
    /// and every newly recorded receipt (which can leave parked consent
    /// waiting on its admitted input; the same receipt again announces
    /// nothing). It carries no state: a delivery composition reads the
    /// committed facts again on every advance.
    pub fn subscribe_consent_deliveries(&self) -> tokio::sync::watch::Receiver<u64> {
        self.consent_delivery_changes.subscribe()
    }

    fn note_consent_delivery_change(&self) {
        self.consent_delivery_changes
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    /// Discover every notice this owner's committed rows still owe. A row
    /// whose stored form is unknown (a commit whose acknowledgement was
    /// lost, a withdrawal written only to the store) is reconciled first
    /// through the owner's existing reconciliation, its store read outside
    /// the owner's reservation, so what it committed is found without an
    /// unrelated read. A row that cannot be reconciled now is reported,
    /// never dropped. An unavailable owner is an error, never "nothing owed".
    ///
    /// The same enumeration reports this owner's parked consent whose
    /// approved notice's committed receipt names an admitted input of its
    /// destination ([`ParkedNoticeInput`]): only that input's native
    /// terminal can still end it
    /// ([`Self::settle_consent_notice_input_ended`]).
    pub fn owed_consent_deliveries(&self) -> Result<OwedConsentDeliveries, ApprovalError> {
        self.ensure_available()?;
        let marked: Vec<ApprovalId> = self.state.read().unreconciled.keys().cloned().collect();
        self.settle_owner_now().run();
        for approval_id in &marked {
            // A row still unknown afterwards is reported below.
            let _ = self.reconcile(approval_id);
        }
        let state = self.state.read();
        let unreconciled = state.unreconciled.keys().cloned().collect();
        let owed = state
            .records
            .iter()
            .filter(|(approval_id, _)| !state.unreconciled.contains_key(*approval_id))
            .filter_map(|(approval_id, record)| Some((approval_id, record.consent.as_ref()?)))
            .flat_map(|(approval_id, consent)| {
                consent
                    .deliveries
                    .iter()
                    .filter(|delivery| delivery.state == ApprovalConsentDeliveryState::Owed)
                    .map(move |delivery| (approval_id.clone(), delivery.transition))
            })
            .collect();
        let parked_notice_inputs = state
            .parked
            .iter()
            .filter(|approval_id| {
                state.consent.contains_key(*approval_id)
                    && !state.unpublished.contains(*approval_id)
                    && !state.unreconciled.contains_key(*approval_id)
            })
            .filter_map(|approval_id| {
                let (scope, input_id) = admitted_notice_input(state.records.get(approval_id)?)?;
                // The receipt records a runtime input id; one that is not
                // names no input the runtime could report on.
                let input_id = uuid::Uuid::parse_str(input_id)
                    .ok()
                    .map(crate::lifecycle::InputId::from_uuid)?;
                Some(ParkedNoticeInput {
                    approval_id: approval_id.clone(),
                    scope: scope.clone(),
                    input_id,
                })
            })
            .collect();
        drop(state);
        self.settle_owner_now().run();
        Ok(OwedConsentDeliveries {
            owed,
            unreconciled,
            parked_notice_inputs,
        })
    }

    /// The admitted notice input of `approval_id`'s parked consent ended
    /// without a resumed run taking the consent over: the delivery
    /// composition read, outside this owner, that the runtime abandoned,
    /// superseded or coalesced it (a consumed input proves nothing here: the
    /// close of the run that consumed it decides). Under the owner's
    /// reservation, the exact current row, its parked custody and its
    /// approved notice's receipt naming `input_id` for the full `destination`
    /// (hosting domain and session) are checked again, and only that consent
    /// is withdrawn: its cause names the input (`NoticeInputEnded`), its
    /// `Withdrawn` notice is owed in the same conditional write, and the
    /// receipt stays as it was. A resumed run's move and this settlement have
    /// one winner: moved consent is never withdrawn here, and withdrawn
    /// consent never moves.
    ///
    /// The custody is retired before the write, and the withdrawal stays
    /// owed until its row confirms it. When the write fails or its outcome is
    /// unknown, the row is unknown (`Unreconciled`) and its reconciliation,
    /// also the next discovery's, records the one withdrawal. Nothing is
    /// re-admitted, relabelled or resent.
    ///
    /// A trusted native delivery-composition entry: its caller read the
    /// input's terminal from the runtime itself. Never expose it to request
    /// coordinates (RPC or REST): coordinates prove no terminal.
    pub fn settle_consent_notice_input_ended(
        &self,
        approval_id: &ApprovalId,
        destination: &super::ApprovalConsentScope,
        input_id: &crate::lifecycle::InputId,
    ) -> Result<NoticeInputEndedSettlement, ApprovalError> {
        self.ensure_available()?;
        self.reconcile_if_unreconciled(approval_id)?;
        let mut state = self.state.write();
        self.count_review_lock();
        let settled =
            self.settle_notice_input_ended_locked(&mut state, approval_id, destination, input_id);
        self.release_state(state).run();
        settled
    }

    fn settle_notice_input_ended_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        destination: &super::ApprovalConsentScope,
        input_id: &crate::lifecycle::InputId,
    ) -> Result<NoticeInputEndedSettlement, ApprovalError> {
        state.ensure_reconciled(approval_id)?;
        let input = input_id.to_string();
        let waiting = state.parked.contains(approval_id)
            && state.consent.contains_key(approval_id)
            && !state.unpublished.contains(approval_id)
            && state
                .records
                .get(approval_id)
                .and_then(admitted_notice_input)
                .is_some_and(|(scope, admitted)| scope == destination && admitted == input);
        if !waiting {
            return Ok(NoticeInputEndedSettlement::NotWaiting);
        }
        let Some(mut pending) =
            withdraw_custody_locked(state, approval_id, ReviewRetirementReason::Abandoned)
        else {
            return Ok(NoticeInputEndedSettlement::NotWaiting);
        };
        pending.cause =
            super::ApprovalConsentWithdrawalReason::NoticeInputEnded { input_id: input };
        // Owed before the write, through the same settlement a
        // reconciliation runs: a failed or unknown write leaves it owed with
        // its cause, never lost and never written twice.
        state
            .unreconciled_withdrawals
            .insert(approval_id.clone(), pending);
        self.settle_reconciled_withdrawal_locked(state, approval_id)?;
        Ok(NoticeInputEndedSettlement::Withdrawn)
    }
}

/// What one discovery of an owner's consent notice obligations found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwedConsentDeliveries {
    /// Every notice still owed, by approval and transition.
    pub owed: Vec<(ApprovalId, ApprovalConsentTransition)>,
    /// Rows whose stored form is unknown and could not be reconciled now.
    /// They may owe notices; a later discovery reconciles them again.
    pub unreconciled: Vec<ApprovalId>,
    /// Parked consent whose approved notice was admitted as one input of its
    /// destination, not taken over by a resumed run yet. Candidates only:
    /// [`ApprovalService::settle_consent_notice_input_ended`] checks each
    /// again once its input's native terminal is read.
    pub parked_notice_inputs: Vec<ParkedNoticeInput>,
}

/// One parked consent of this owner whose approved notice's committed
/// receipt admitted it as one input of its destination session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParkedNoticeInput {
    pub approval_id: ApprovalId,
    /// The committed destination of the consent's notices: hosting domain
    /// and session.
    pub scope: super::ApprovalConsentScope,
    /// The admitted input, as the receipt names it.
    pub input_id: crate::lifecycle::InputId,
}

/// What settling one parked consent's ended notice input did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeInputEndedSettlement {
    /// The parked consent was withdrawn, its cause naming the input, and its
    /// `Withdrawn` notice is owed.
    Withdrawn,
    /// Nothing of this owner waits on that input: no parked consent's
    /// approved notice receipt names it for that destination (a resumed run
    /// took the consent over, or it was withdrawn, spent or expired). No
    /// waiting consent was withdrawn now; the row's reconciliation on entry
    /// may first have finished an earlier uncertain withdrawal.
    NotWaiting,
}

/// Why an owner-loss settlement refused one consent row. Nothing was
/// written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerLossRefusal {
    /// No such approval.
    NotFound,
    /// Not a consent approval.
    NotConsent,
    /// A consent row outside the scope the owner-loss proof covers (or one
    /// without a recorded scope, which no proof covers).
    OutOfScope,
    /// This owner holds the consent's live custody: it is not lost.
    HeldHere,
    /// The consent was spent.
    Consumed,
    /// Expired history is never rewritten as an owner loss.
    Expired,
    /// Already settled history: denied, cancelled, or approved with its
    /// withdrawal recorded.
    Settled,
}

/// The outcome of settling one consent row of a lost owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerLossSettlement {
    /// The row records the owner's loss in one conditional write: a
    /// pending approval is cancelled (never denied, whatever its deadline),
    /// approved history is kept, the withdrawal is recorded as `Abandoned`
    /// and its notice owed. Deadline, decision and custody history are
    /// unchanged.
    Settled(ApprovalRecord),
    /// Refused; nothing was written.
    Refused(OwnerLossRefusal),
}

impl ApprovalService {
    /// Settle one consent row whose issuing owner is proven lost, row only:
    /// no candidate is constructed and nothing becomes executable. `scope`
    /// is the hosting scope the caller's owner-loss proof covers (a
    /// cross-process fresh acquisition or takeover); a row of another scope
    /// is refused. Parked or transferred custody recorded on the row
    /// belonged to the lost owner's process and is gone with it: such a row
    /// settles like any unspent one and keeps that history. A row this
    /// owner still holds custody of, a spent, expired or already settled
    /// row, and a row whose stored form is unknown (`Unreconciled`) are
    /// never written.
    pub fn settle_lost_owner_consent(
        &self,
        approval_id: &ApprovalId,
        scope: &super::ApprovalConsentScope,
    ) -> Result<OwnerLossSettlement, ApprovalError> {
        self.ensure_available()?;
        let mut state = self.state.write();
        self.count_review_lock();
        let settled = self.settle_lost_owner_consent_locked(&mut state, approval_id, scope);
        self.release_state(state).run();
        settled
    }

    /// [`Self::settle_lost_owner_consent`] under a reservation already held.
    pub(super) fn settle_lost_owner_consent_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        scope: &super::ApprovalConsentScope,
    ) -> Result<OwnerLossSettlement, ApprovalError> {
        state.ensure_reconciled(approval_id)?;
        let refused = |refusal| Ok(OwnerLossSettlement::Refused(refusal));
        let Some(record) = state.records.get(approval_id).cloned() else {
            return refused(OwnerLossRefusal::NotFound);
        };
        if record
            .consent
            .as_ref()
            .is_some_and(|consent| consent.scope.as_ref() != Some(scope))
        {
            return refused(OwnerLossRefusal::OutOfScope);
        }
        let mut authority = state.authority.clone();
        let outcome = authority
            .settle_unlinked_consent(approval_id.to_string())
            .map_err(lifecycle_error)?;
        let status = match outcome {
            ApprovalLifecycleOutcome::Status(status) => {
                super::approval_status_from_lifecycle(status)
            }
            ApprovalLifecycleOutcome::Rejected(reason) => {
                return refused(match reason {
                    ApprovalLifecycleRejectionReason::NotFound => OwnerLossRefusal::NotFound,
                    ApprovalLifecycleRejectionReason::NotConsent => OwnerLossRefusal::NotConsent,
                    ApprovalLifecycleRejectionReason::ConsentLinked => OwnerLossRefusal::HeldHere,
                    ApprovalLifecycleRejectionReason::AlreadyConsumed => OwnerLossRefusal::Consumed,
                    ApprovalLifecycleRejectionReason::Expired => OwnerLossRefusal::Expired,
                    ApprovalLifecycleRejectionReason::AlreadyDecided => OwnerLossRefusal::Settled,
                    other => {
                        return Err(ApprovalError::LifecycleInvariant(format!(
                            "generated approval lifecycle refused the owner loss of {approval_id} as {other:?}"
                        )));
                    }
                });
            }
            other => {
                return Err(ApprovalError::LifecycleInvariant(format!(
                    "generated approval lifecycle settled the owner loss of {approval_id} as {other:?}"
                )));
            }
        };
        // Approved history whose withdrawal its owner recorded before it
        // was lost is already settled.
        if record
            .consent
            .as_ref()
            .is_some_and(|consent| consent.withdrawal.is_some())
        {
            return refused(OwnerLossRefusal::Settled);
        }
        let now = self.now();
        let mut settled = record.clone();
        settled.status = status;
        settled.updated_at = now;
        let mut consent = record.consent.clone().unwrap_or_default();
        consent.withdrawal = Some(ApprovalConsentWithdrawal {
            reason: super::ApprovalConsentWithdrawalReason::Abandoned,
            withdrawn_at: now,
        });
        // Owed with the withdrawal's own durable record, in the same write.
        consent.owe_delivery(ApprovalConsentTransition::Withdrawn, now);
        settled.consent = Some(consent);
        self.commit_row_locked(state, Some(&record), &settled)?;
        state.authority = authority;
        state.records.insert(approval_id.clone(), settled.clone());
        Ok(OwnerLossSettlement::Settled(settled))
    }
}

/// `sha256:` digest of one committed consent transition.
fn consent_delivery_digest(
    approval_id: &ApprovalId,
    transition: ApprovalConsentTransition,
    committed_at: DateTime<Utc>,
) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(approval_id.to_string().as_bytes());
    hasher.update([0]);
    hasher.update(transition.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(committed_at.to_rfc3339().as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

enum Decided {
    /// Approved and committed; its allowance awaits publication.
    Approved(ApprovalRecord),
    Declined(ApprovalRecord),
}

/// The record and eligibility evidence a decision was resolved against.
struct Resolved<'a> {
    record: &'a ApprovalRecord,
    eligibility: &'a dyn ConsentEligibility,
}

/// What one round of a decision's final checks checks against.
struct PublishChecks<'a> {
    /// The approver, the record their eligibility was resolved for and its
    /// evidence (resolved afresh when an owner change made it stale);
    /// `None` once the eligibility owner refused them.
    approval: Option<&'a ConsentApproval>,
    /// The committed decision: the answer when the row is gone meanwhile.
    decided: &'a ApprovalRecord,
    /// The retained decision's re-adjudication of the previous round.
    readjudication: Option<&'a Readjudication>,
    /// The re-adjudication bound is spent: what is still not current is
    /// published as it is, for its spend to check again; nothing is
    /// withdrawn for the bound alone.
    exhausted: bool,
}

/// The retryable answer of a decision that records nothing because the
/// owners' authorization of its candidate, or the approver's eligibility,
/// kept changing or could not be decided now (ADR-001 D9).
fn unsettled_decision(approval_id: &ApprovalId) -> ApprovalError {
    ApprovalError::StoreUnavailable(format!(
        "the authorization of approval {approval_id} is changing; nothing was recorded, and a retry is valid"
    ))
}

/// What a move's preparation found, outside every owner lock: the resumed
/// run's own decision of the exact original operation with its owners'
/// read of the review material, or their answer instead of a decision.
type MovePreparation = Result<
    (
        PreparedOperationCheck,
        Result<ReviewOperationContextResolution, OperationAuthorizationError>,
    ),
    OperationAuthorizationError,
>;

/// The parked candidates ready to move, each into the open resumed run whose
/// staged input its committed receipt names: into any such run, or `only`
/// into the run of that work and id.
fn pending_transfers(
    state: &ApprovalServiceState,
    only: Option<(&WorkAuthorizationContext, &RunId)>,
) -> Vec<PendingTransfer> {
    state
        .parked
        .iter()
        .filter(|approval_id| {
            !state.unpublished.contains(*approval_id)
                && !state.unreconciled.contains_key(*approval_id)
        })
        .filter_map(|approval_id| {
            let candidate = state.consent.get(approval_id)?;
            let record = state.records.get(approval_id)?;
            state
                .resumed_runs
                .iter()
                .filter(|run| {
                    only.is_none_or(|(work, run_id)| {
                        run.work.same_context(work) && &run.run_id == run_id
                    })
                })
                .find_map(|run| {
                    transfer_input(record, &run.work).map(|input_id| PendingTransfer {
                        approval_id: approval_id.clone(),
                        candidate: Arc::clone(candidate),
                        record: record.clone(),
                        run: run.clone(),
                        input_id,
                    })
                })
        })
        .collect()
}

/// The resumed run's own owners decide the exact original operation of
/// `transfer` afresh, outside every owner lock, and read its review material
/// (ADR-001 D9): the move keeps the consent only for the account the
/// approval presented, and an owner change in that run is compared against
/// this read from then on.
fn prepare_move(transfer: &PendingTransfer) -> MovePreparation {
    transfer
        .candidate
        .admitted()
        .prepare_for_resumed_run(transfer.run.work.clone(), &transfer.run.run_id)
        .map(|admitted| {
            let material = admitted
                .work_authorization()
                .authorization()
                .review_operation_context(admitted.binding());
            (admitted, material)
        })
}

/// The withdrawal of a parked consent whose resumed run's owners refused its
/// operation, or its review material, at the move: by the refusal's category
/// (a denial is an authority answer; facts they cannot read changed the
/// candidate's context). A change in progress never reaches here.
fn move_refusal_reason(kind: OperationRefusalKind) -> ReviewRetirementReason {
    if kind == OperationRefusalKind::Denied {
        ReviewRetirementReason::AuthorityChanged
    } else {
        ReviewRetirementReason::ContextChanged
    }
}

/// The action account review material names, if any.
fn action_account(material: &ReviewOperationContextResolution) -> Option<&str> {
    match material {
        ReviewOperationContextResolution::NotApplicable => None,
        ReviewOperationContextResolution::Material(context) => context.action_account(),
    }
}

/// A run open now that resumes retained work through staged inputs.
#[derive(Debug, Clone)]
pub(super) struct ResumedRun {
    work: WorkAuthorizationContext,
    run_id: RunId,
    /// The consent whose transfer into this run this owner committed, by the
    /// receipt of one of the run's staged inputs: the owner's own binding of
    /// that consent to this work, run and input, read by the run's
    /// boundaries ([`ApprovalService::consent_moved_into_run`]). It ends with
    /// the run's entry.
    moved: std::collections::BTreeSet<ApprovalId>,
}

/// One parked candidate ready to move into an open resumed run, as read.
struct PendingTransfer {
    approval_id: ApprovalId,
    candidate: Arc<ConsentCandidate>,
    record: ApprovalRecord,
    run: ResumedRun,
    input_id: String,
}

/// The staged input of `work` that carries `record`'s approved notice by its
/// committed receipt, when the parked consent can move into that resumed
/// run: approved, unspent and unwithdrawn, the same destination, a receipt
/// naming one of the run's inputs for that destination, and that input
/// resuming the consent's exact origin work.
fn transfer_input(record: &ApprovalRecord, work: &WorkAuthorizationContext) -> Option<String> {
    let consent = record.consent.as_ref()?;
    if record.status != ApprovalStatus::Approved
        || consent.consumed_at.is_some()
        || consent.withdrawal.is_some()
        || consent.custody != Some(ApprovalConsentCustody::Parked)
    {
        return None;
    }
    let scope = consent.scope.as_ref()?;
    let origin = consent.origin.as_ref()?;
    if work.consent_scope() != Some(scope) {
        return None;
    }
    let ApprovalConsentDeliveryState::Admitted {
        session_id,
        input_id,
    } = &consent.delivery(ApprovalConsentTransition::Approved)?.state
    else {
        return None;
    };
    (session_id == &scope.session_id
        && work.resumed_inputs().iter().any(|resumed| {
            resumed.input_id.to_string() == *input_id && resumed.resumes == origin.retained_work
        }))
    .then(|| input_id.clone())
}

/// The staged notice input `record`'s consent moved into a resumed run with:
/// its custody `Transferred { input_id }`, when the row records one.
fn moved_input(record: &ApprovalRecord) -> Option<String> {
    match record.consent.as_ref()?.custody.as_ref()? {
        ApprovalConsentCustody::Transferred { input_id } => Some(input_id.clone()),
        ApprovalConsentCustody::Parked => None,
    }
}

/// The destination and admitted input that `record`'s approved notice
/// receipt names, while its consent is approved, unspent, unwithdrawn and
/// recorded as parked, and the receipt admitted it into the consent's own
/// destination session.
fn admitted_notice_input(record: &ApprovalRecord) -> Option<(&super::ApprovalConsentScope, &str)> {
    let consent = record.consent.as_ref()?;
    if record.status != ApprovalStatus::Approved
        || consent.consumed_at.is_some()
        || consent.withdrawal.is_some()
        || consent.custody != Some(ApprovalConsentCustody::Parked)
        || consent.origin.is_none()
    {
        return None;
    }
    let scope = consent.scope.as_ref()?;
    let ApprovalConsentDeliveryState::Admitted {
        session_id,
        input_id,
    } = &consent.delivery(ApprovalConsentTransition::Approved)?.state
    else {
        return None;
    };
    (session_id == &scope.session_id).then_some((scope, input_id.as_str()))
}

/// `record` with its consent withdrawn for `cause` at `now` and the
/// generated owner's `status`. The withdrawal owes its own notice; it never
/// settles or retracts an earlier transition's.
fn withdrawn_record(
    record: &ApprovalRecord,
    status: ApprovalStatus,
    cause: super::ApprovalConsentWithdrawalReason,
    now: DateTime<Utc>,
) -> ApprovalRecord {
    let mut withdrawn = record.clone();
    withdrawn.status = status;
    withdrawn.updated_at = now;
    let mut consent = ApprovalConsentRecord {
        withdrawal: Some(ApprovalConsentWithdrawal {
            reason: cause,
            withdrawn_at: now,
        }),
        ..record.consent.clone().unwrap_or_default()
    };
    consent.owe_delivery(ApprovalConsentTransition::Withdrawn, now);
    withdrawn.consent = Some(consent);
    withdrawn
}

/// A staged input whose resumed run, for `scope`, closed while parked
/// consent of the work it resumed, for that destination, still awaited its
/// approved notice's receipt.
#[derive(Debug, Clone)]
pub(super) struct ClosedInput {
    input_id: String,
    resumes: crate::retained_work::RetainedWorkIdentity,
    scope: super::ApprovalConsentScope,
}

/// Whether `record`'s parked consent from `origin`, for the destination
/// `scope`, still awaits the receipt of its approved notice (approved,
/// unspent and unwithdrawn, owed).
fn awaits_receipt(
    record: &ApprovalRecord,
    origin: &crate::retained_work::RetainedWorkIdentity,
    scope: &super::ApprovalConsentScope,
) -> bool {
    record.status == ApprovalStatus::Approved
        && record.consent.as_ref().is_some_and(|consent| {
            consent.consumed_at.is_none()
                && consent.withdrawal.is_none()
                && consent.scope.as_ref() == Some(scope)
                && consent
                    .origin
                    .as_ref()
                    .is_some_and(|recorded| &recorded.retained_work == origin)
                && consent
                    .delivery(ApprovalConsentTransition::Approved)
                    .is_some_and(|delivery| delivery.state == ApprovalConsentDeliveryState::Owed)
        })
}

/// Drop the closed inputs no parked consent of their work awaits any more.
fn prune_closed_inputs(state: &mut ApprovalServiceState) {
    let ApprovalServiceState {
        closed_inputs,
        parked,
        records,
        ..
    } = state;
    closed_inputs.retain(|closed| {
        parked.iter().any(|approval_id| {
            records
                .get(approval_id)
                .is_some_and(|record| awaits_receipt(record, &closed.resumes, &closed.scope))
        })
    });
}

/// Whether `work` resumes the origin work of `record`'s consent.
fn resumes_origin(record: &ApprovalRecord, work: &WorkAuthorizationContext) -> bool {
    record
        .consent
        .as_ref()
        .and_then(|consent| consent.origin.as_ref())
        .is_some_and(|origin| {
            work.resumed_inputs()
                .iter()
                .any(|resumed| resumed.resumes == origin.retained_work)
        })
}

/// Whether `current` is the row a decision resolved its eligibility for,
/// changed at most in its executable custody (a park) and that write's time:
/// eligibility never reads custody.
fn same_but_custody(current: &ApprovalRecord, resolved: &ApprovalRecord) -> bool {
    let custody = |record: &ApprovalRecord| {
        record
            .consent
            .as_ref()
            .and_then(|consent| consent.custody.clone())
    };
    if current == resolved || custody(current) == custody(resolved) {
        return current == resolved;
    }
    let mut restored = current.clone();
    restored.updated_at = resolved.updated_at;
    if let Some(consent) = restored.consent.as_mut() {
        consent.custody = custody(resolved);
    }
    restored == *resolved
}

fn is_live(
    state: &ApprovalServiceState,
    approval_id: &ApprovalId,
    candidate: &Arc<ConsentCandidate>,
) -> bool {
    state
        .consent
        .get(approval_id)
        .is_some_and(|live| Arc::ptr_eq(live, candidate))
}

/// A withdrawal whose custody is already retired and whose durable record
/// (and notice) is still to be written.
#[derive(Debug, Clone)]
pub(super) struct PendingWithdrawal {
    approval_id: ApprovalId,
    candidate: Arc<ConsentCandidate>,
    reason: ReviewRetirementReason,
    /// The durable cause the withdrawal records: `reason`'s own, unless the
    /// withdrawing owner names a more specific one (such as
    /// [`super::ApprovalConsentWithdrawalReason::NoticeInputEnded`]) before
    /// the record. Cause and owed `Withdrawn` notice commit in one write.
    cause: super::ApprovalConsentWithdrawalReason,
}

/// Drop the custody of a live candidate and apply the generated withdrawal.
fn withdraw_custody_locked(
    state: &mut ApprovalServiceState,
    approval_id: &ApprovalId,
    reason: ReviewRetirementReason,
) -> Option<PendingWithdrawal> {
    let candidate = state.consent.remove(approval_id)?;
    state.unpublished.remove(approval_id);
    if state.parked.remove(approval_id) {
        prune_closed_inputs(state);
    }
    let outcome = state.authority.withdraw_consent(
        approval_id.to_string(),
        candidate.attempt.id().to_owned(),
        reason,
    );
    // The generated owner cancels a pending approval or keeps settled
    // history; the row takes the owner's status as the withdrawal settles.
    if !matches!(
        outcome,
        Ok(
            ApprovalLifecycleOutcome::Status(ApprovalLifecycleStatus::Cancelled)
                | ApprovalLifecycleOutcome::ReviewStatus(ReviewAttemptStatus::Retired)
        )
    ) {
        // The custody is gone regardless: nothing can spend it.
        tracing::warn!(
            ?outcome,
            "generated approval owner refused a consent withdrawal"
        );
    }
    Some(PendingWithdrawal {
        approval_id: approval_id.clone(),
        candidate,
        reason,
        cause: reason.into(),
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    //! Human consent at the approval owner and the review seam: an
    //! authenticated decision only records and notifies; a fresh exact
    //! attempt spends it once; wrong, copied, expired, declined, changed and
    //! restored attempts never spend; contention and a failure before entry
    //! keep it spendable.

    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::approval::review::{
        BoundOperationReview, OperationReviewTier, OperationReviewer, ReviewCandidate,
        ReviewDispatchGuard, ReviewEntry, ReviewEntrySupport, ReviewVerdict, ReviewerFailure,
        admit_consent_attempt, admit_operation_review,
    };
    use crate::approval::{
        ApprovalConsentCustody, ApprovalConsentDelivery, ApprovalConsentScope,
        ApprovalConsentWithdrawalReason, ApprovalListFilter, ApprovalStore, ApprovalStoreError,
        InMemoryApprovalStore,
    };
    use crate::authorization::{
        AuthorizationOperation, OperationAuthorizationError, OperationAuthorizationFacts,
        OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
        PreparedOperationAuthorization, SourceAuthorizationFacts, SourceAuthorizationTarget,
        SourceAuthorizationUse, WorkAuthorization,
    };
    use crate::exact_operation::OperationExecutionScope;
    use crate::lifecycle::InputId;
    use crate::memory::MemorySearchScope;
    use crate::ops::OperationId;
    use crate::retained_work::ResumedInput;
    use crate::types::SessionId;
    use crate::{PrincipalKind, TrustDomainId};

    mod owner_loss;

    #[test]
    fn presentation_candidate_keeps_the_historical_policy_observation() {
        const OBSERVED: crate::authorization::PolicyPublicationObservation =
            crate::authorization::PolicyPublicationObservation::LocalPublication {
                instance: uuid::Uuid::nil(),
                sequence: 7,
            };
        struct Observed;
        impl PreparedOperationAuthorization for Observed {
            fn policy_observation(
                &self,
            ) -> Option<crate::authorization::PolicyPublicationObservation> {
                Some(OBSERVED)
            }
            fn review_tier(&self) -> OperationReviewTier {
                OperationReviewTier::R3
            }
            fn check_current(
                &self,
                _binding: &PreparedAuthorizationBinding,
            ) -> Result<(), OperationAuthorizationError> {
                Ok(())
            }
            fn observe(
                &self,
                _binding: &PreparedAuthorizationBinding,
                _observation: crate::authorization::OperationObservation,
            ) -> Result<(), crate::authorization::OperationObservationError> {
                Ok(())
            }
        }
        struct Publisher;
        impl WorkAuthorization for Publisher {
            fn prepare(
                &self,
                _binding: &PreparedAuthorizationBinding,
            ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError>
            {
                Ok(Arc::new(Observed))
            }
        }

        let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: OperationId::new(),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(
                    SessionId::new(),
                )),
                usage: SourceAuthorizationUse::Read,
            }),
        });
        let check = PreparedOperationCheck::prepare(
            WorkAuthorizationContext::new(Arc::new(Publisher), OperationExecutionScope::Domain),
            binding,
        )
        .expect("prepared");
        let args = serde_json::value::RawValue::from_string("{}".into()).expect("args");
        let candidate = ConsentPresentationCandidate {
            call: ToolCallView {
                id: "call-1",
                name: "send_message",
                args: &args,
            },
            check: &check,
            operation_context: None,
        };
        assert!(candidate.policy_observation() == Some(OBSERVED));
    }

    /// Host-chosen validity of one consent request.
    const VALIDITY: Duration = Duration::from_secs(4 * 60 * 60);
    /// Real-time hang guard for the contention holder.
    const BOUND: Duration = Duration::from_secs(10);

    /// Republishable R3 owner: a publication change makes a retained
    /// decision stale, and re-preparation yields a different decision.
    #[derive(Default)]
    struct Owner {
        sequence: AtomicU64,
        /// The current permission is withdrawn: every check refuses.
        denied: AtomicBool,
        /// Arbitrary owner code run while a refusal is observed.
        on_observe: parking_lot::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
        /// Decisions prepared from now on require R2 instead of R3.
        r2: AtomicBool,
        /// Current checks so far, and the first one that refuses (0: none).
        checks: AtomicUsize,
        deny_from: AtomicUsize,
        /// Preparation refuses the operation (an actual denial).
        prepare_refused: AtomicBool,
        /// Preparation finds the owner unavailable.
        prepare_unavailable: AtomicBool,
        /// Arbitrary owner code run once inside the next preparation.
        on_prepare: parking_lot::Mutex<Option<Box<dyn FnOnce() + Send>>>,
        /// Preparations so far.
        prepares: AtomicUsize,
        /// The next this many preparations each publish one completed owner
        /// change right after preparing, so their decision is stale at once.
        republish_after_prepare: AtomicUsize,
        /// The same, right after each of the next this many material reads.
        republish_after_material_read: AtomicUsize,
        /// An owner change is in progress: preparations and material reads
        /// refuse as stale, and the work names the change in progress.
        in_progress: AtomicBool,
        /// The owner's review material, its selected action account and its
        /// mandate; both `None`: explicitly not applicable.
        material: parking_lot::Mutex<Option<String>>,
        mandate: parking_lot::Mutex<Option<String>>,
        /// Material reads refuse (`Denied`), find the owner unavailable, or
        /// answer that an owner change is in progress (`ReprepareRequired`).
        material_refused: AtomicBool,
        material_unavailable: AtomicBool,
        material_in_change: AtomicBool,
        /// The wall-clock end (ms) of the validity window decisions are
        /// prepared with; 0: decisions carry no window.
        expires_at_ms: AtomicU64,
        /// The owner's validity windows that ended: a decision prepared
        /// before the latest end refuses itself (`Denied`), as an owner's
        /// sliding window for a decision does once its time passed, with no
        /// owner change. A fresh preparation gets a window of its own.
        windows_ended: AtomicU64,
        /// The owner is unavailable: every current check answers so.
        check_unavailable: AtomicBool,
    }

    impl Owner {
        /// One completed owner change that keeps every permission, tier,
        /// validity window and material: unrelated to every candidate.
        fn republish(&self) {
            self.sequence.fetch_add(1, Ordering::AcqRel);
        }

        /// Change the owner's review material, published as an owner change.
        fn set_material(&self, material: Option<&str>) {
            *self.material.lock() = material.map(str::to_owned);
            self.republish();
        }

        /// Change the mandate of the owner's review material, published as
        /// an owner change.
        fn set_mandate(&self, mandate: Option<&str>) {
            *self.mandate.lock() = mandate.map(str::to_owned);
            self.republish();
        }

        /// Change the validity window decisions are prepared with, published
        /// as an owner change.
        fn set_expiry(&self, expires_at_ms: u64) {
            self.expires_at_ms.store(expires_at_ms, Ordering::Release);
            self.republish();
        }

        /// Time passes: the validity window of every decision prepared so
        /// far ends, and nothing is published.
        fn end_windows(&self) {
            self.windows_ended.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Take one of `budget`'s remaining uses, when one is left.
    fn take_one(budget: &AtomicUsize) -> bool {
        budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                left.checked_sub(1)
            })
            .is_ok()
    }

    struct Decision {
        owner: Arc<Owner>,
        sequence: u64,
        /// The owner's ended windows when it was prepared.
        window: u64,
        tier: OperationReviewTier,
        validity: Option<crate::authorization::OperationValidity>,
    }

    impl PreparedOperationAuthorization for Decision {
        fn review_tier(&self) -> OperationReviewTier {
            self.tier
        }

        fn validity(&self) -> Option<crate::authorization::OperationValidity> {
            self.validity
        }

        fn check_current(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            let check = self.owner.checks.fetch_add(1, Ordering::SeqCst) + 1;
            let deny_from = self.owner.deny_from.load(Ordering::SeqCst);
            if self.owner.check_unavailable.load(Ordering::Acquire) {
                return Err(OperationAuthorizationError::Unavailable);
            }
            if self.owner.denied.load(Ordering::Acquire)
                || (deny_from != 0 && check >= deny_from)
                || self.window < self.owner.windows_ended.load(Ordering::Acquire)
            {
                Err(OperationRefused::new(OperationRefusalKind::Denied).into())
            } else if self.sequence == self.owner.sequence.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into())
            }
        }

        fn observe(
            &self,
            _binding: &PreparedAuthorizationBinding,
            _observation: crate::authorization::OperationObservation,
        ) -> Result<(), crate::authorization::OperationObservationError> {
            let hook = self.owner.on_observe.lock().clone();
            if let Some(hook) = hook {
                hook();
            }
            Ok(())
        }
    }

    struct OwnerWork(Arc<Owner>);

    impl WorkAuthorization for OwnerWork {
        /// The owner's review material: its selected action account and its
        /// mandate, or explicitly not applicable (the default).
        fn review_operation_context(
            &self,
            _binding: &crate::authorization::PreparedAuthorizationBinding,
        ) -> Result<
            crate::authorization::ReviewOperationContextResolution,
            crate::authorization::OperationAuthorizationError,
        > {
            if self.0.in_progress.load(Ordering::Acquire) {
                return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
            }
            if self.0.material_refused.load(Ordering::Acquire) {
                return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
            }
            if self.0.material_unavailable.load(Ordering::Acquire) {
                return Err(OperationAuthorizationError::Unavailable);
            }
            if self.0.material_in_change.load(Ordering::Acquire) {
                return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
            }
            let account = self.0.material.lock().clone();
            let mandate = self.0.mandate.lock().clone();
            if take_one(&self.0.republish_after_material_read) {
                self.0.republish();
            }
            if account.is_none() && mandate.is_none() {
                return Ok(crate::authorization::ReviewOperationContextResolution::NotApplicable);
            }
            crate::authorization::ReviewOperationContext::new(account, mandate)
                .map(crate::authorization::ReviewOperationContextResolution::Material)
                .map_err(|_| OperationAuthorizationError::Unavailable)
        }

        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            let hook = self.0.on_prepare.lock().take();
            if let Some(hook) = hook {
                hook();
            }
            self.0.prepares.fetch_add(1, Ordering::SeqCst);
            if self.0.in_progress.load(Ordering::Acquire) {
                return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
            }
            if self.0.prepare_unavailable.load(Ordering::Acquire) {
                return Err(OperationAuthorizationError::Unavailable);
            }
            if self.0.prepare_refused.load(Ordering::Acquire) {
                return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
            }
            let expires_at_ms = self.0.expires_at_ms.load(Ordering::Acquire);
            let decision = Arc::new(Decision {
                owner: Arc::clone(&self.0),
                sequence: self.0.sequence.load(Ordering::Acquire),
                window: self.0.windows_ended.load(Ordering::Acquire),
                tier: if self.0.r2.load(Ordering::Acquire) {
                    OperationReviewTier::R2
                } else {
                    OperationReviewTier::R3
                },
                // Each preparation reads the window on its own monotonic
                // clock: equal windows, different deadlines.
                validity: (expires_at_ms != 0).then(|| crate::authorization::OperationValidity {
                    not_before_ms: 0,
                    expires_at_ms,
                    deadline: crate::time_compat::Instant::now() + Duration::from_secs(3600),
                }),
            });
            if take_one(&self.0.republish_after_prepare) {
                self.0.republish();
            }
            Ok(decision)
        }

        fn owner_change_in_progress(&self) -> Option<crate::authorization::OwnerChangeInProgress> {
            self.0.in_progress.load(Ordering::Acquire).then_some(
                crate::authorization::OwnerChangeInProgress::new(uuid::Uuid::nil(), 1),
            )
        }
    }

    /// R3 is never model-reviewed; any call is counted. An R2 review gets
    /// the scripted behavior: a verdict, a hang past every deadline, or a
    /// verdict that only becomes ready after the deadline; otherwise it
    /// fails.
    #[derive(Default)]
    struct NoReviewer(AtomicUsize, parking_lot::Mutex<Option<Scripted>>);

    #[derive(Clone, Copy)]
    enum Scripted {
        Verdict(ReviewVerdict),
        Hang,
        LateVerdict(ReviewVerdict),
    }

    #[async_trait::async_trait]
    impl OperationReviewer for NoReviewer {
        async fn review(
            &self,
            _candidate: &ReviewCandidate<'_>,
        ) -> Result<ReviewVerdict, ReviewerFailure> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let scripted = *self.1.lock();
            match scripted {
                None => Err(ReviewerFailure::Unavailable),
                Some(Scripted::Verdict(verdict)) => Ok(verdict),
                Some(Scripted::Hang) => std::future::pending().await,
                Some(Scripted::LateVerdict(verdict)) => {
                    tokio::time::advance(R2_DEADLINE + Duration::from_secs(1)).await;
                    Ok(verdict)
                }
            }
        }
    }

    /// The model review deadline of the test review owner.
    const R2_DEADLINE: Duration = Duration::from_secs(30);

    /// Host route: presents readably unless told otherwise, lets only the
    /// listed principals decide, and records notices.
    #[derive(Default)]
    struct Route {
        eligible: parking_lot::Mutex<Vec<String>>,
        /// Publication of the eligibility facts: every change bumps it.
        revision: Arc<AtomicU64>,
        notices: parking_lot::Mutex<Vec<(ApprovalId, ConsentNoticeKind)>>,
        unreadable: AtomicBool,
        /// Revokes the approver right after its eligibility was resolved,
        /// before the decision commits.
        revoke_after_resolve: AtomicBool,
        /// Host authentication scopes (logins) that are revoked.
        revoked_logins: parking_lot::Mutex<Vec<String>>,
        /// Revokes this login right after an event from it was resolved.
        revoke_login_after_resolve: parking_lot::Mutex<Option<String>>,
        /// Runs once right after an eligibility was resolved, before the
        /// decision takes its reservation.
        after_resolve: parking_lot::Mutex<Option<Box<dyn FnOnce() + Send>>>,
        /// The host's self-approval policy: the requester may decide their
        /// own approval while set. Changed only through a publication.
        self_approval: AtomicBool,
        /// An eligibility owner change is in progress: the owner cannot say
        /// now (`Unsettled`).
        in_change: AtomicBool,
        /// Every approval the eligibility owner was asked about: its status
        /// and the principal that decided it.
        asked: parking_lot::Mutex<Vec<(ApprovalStatus, Option<String>)>>,
    }

    impl Route {
        fn publish(&self, change: impl FnOnce(&mut Vec<String>)) {
            change(&mut self.eligible.lock());
            self.revision.fetch_add(1, Ordering::AcqRel);
        }

        /// Set the self-approval policy, published as an eligibility change.
        fn permit_self_approval(&self, permitted: bool) {
            self.self_approval.store(permitted, Ordering::Release);
            self.revision.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Eligibility evidence as of one publication of the route's facts.
    struct Evidence {
        revision: Arc<AtomicU64>,
        seen: u64,
    }

    impl ConsentEligibility for Evidence {
        fn check_current(&self) -> Result<(), ConsentIneligible> {
            if self.revision.load(Ordering::Acquire) == self.seen {
                Ok(())
            } else {
                Err(ConsentIneligible)
            }
        }
    }

    impl HumanConsentRoute for Route {
        fn present(
            &self,
            _candidate: &crate::approval::consent::ConsentPresentationCandidate<'_>,
        ) -> Result<ConsentPresentation, ConsentRouteUnavailable> {
            if self.unreadable.load(Ordering::SeqCst) {
                return Err(ConsentRouteUnavailable);
            }
            Ok(ConsentPresentation {
                requester: ApprovalPrincipalId::new("human:child").expect("principal"),
                risk: ApprovalRisk::High,
                metadata: SurfaceMetadata::default(),
            })
        }

        fn eligibility(
            &self,
            approver: &AuthenticatedApprover,
            approval: &ApprovalRecord,
        ) -> Result<Arc<dyn ConsentEligibility>, ConsentIneligible> {
            let seen = self.revision.load(Ordering::Acquire);
            let id = approver.principal().id.as_str();
            let eligible = if id == approval.requester.as_str() {
                self.self_approval.load(Ordering::Acquire)
            } else {
                self.eligible.lock().iter().any(|eligible| eligible == id)
            };
            let login = approver
                .authentication_scope()
                .map(|scope| scope.as_str().to_owned());
            let revoked = login
                .as_ref()
                .is_some_and(|login| self.revoked_logins.lock().contains(login));
            if !eligible || revoked {
                return Err(ConsentIneligible);
            }
            let evidence = Arc::new(Evidence {
                revision: Arc::clone(&self.revision),
                seen,
            });
            let mut revoke_login = self.revoke_login_after_resolve.lock();
            if login.is_some() && *revoke_login == login {
                let login = revoke_login.take();
                drop(revoke_login);
                self.revoked_logins.lock().extend(login);
                self.revision.fetch_add(1, Ordering::AcqRel);
            } else {
                drop(revoke_login);
            }
            if self.revoke_after_resolve.swap(false, Ordering::AcqRel) {
                let revoked = approver.principal().id.as_str().to_owned();
                self.publish(|eligible| eligible.retain(|id| *id != revoked));
            }
            let after_resolve = self.after_resolve.lock().take();
            if let Some(after_resolve) = after_resolve {
                after_resolve();
            }
            Ok(evidence)
        }

        fn eligibility_answer(
            &self,
            approver: &AuthenticatedApprover,
            approval: &ApprovalRecord,
        ) -> ConsentEligibilityAnswer {
            self.asked.lock().push((
                approval.status,
                approval
                    .decision
                    .as_ref()
                    .map(|decided| decided.actor.as_str().to_owned()),
            ));
            if self.in_change.load(Ordering::Acquire) {
                return ConsentEligibilityAnswer::Unsettled;
            }
            match self.eligibility(approver, approval) {
                Ok(evidence) => ConsentEligibilityAnswer::Eligible(evidence),
                Err(ConsentIneligible) => ConsentEligibilityAnswer::Refused,
            }
        }

        fn notice(&self, notice: ConsentNotice) {
            self.notices.lock().push((notice.approval_id, notice.kind));
        }
    }

    type Statuses = Vec<(ReviewAttemptStatus, Option<ReviewRetirementReason>)>;

    #[derive(Default)]
    struct Recording(parking_lot::Mutex<Statuses>);

    impl OperationReviewObserver for Recording {
        fn observe(&self, observation: ReviewObservation) {
            self.0
                .lock()
                .push((observation.status, observation.retirement));
        }
    }

    struct World {
        approvals: ApprovalService,
        owner: Arc<Owner>,
        work: WorkAuthorizationContext,
        reviewer: Arc<NoReviewer>,
        route: Arc<Route>,
        observer: Arc<Recording>,
        review: Arc<BoundOperationReview>,
    }

    fn world() -> World {
        world_with(ApprovalService::new())
    }

    fn world_with(approvals: ApprovalService) -> World {
        let owner = Arc::new(Owner::default());
        let work = WorkAuthorizationContext::new(
            Arc::new(OwnerWork(Arc::clone(&owner))),
            OperationExecutionScope::Domain,
        );
        let reviewer = Arc::new(NoReviewer::default());
        let route = Arc::new(Route::default());
        route.eligible.lock().push("lead-1".to_string());
        let observer = Arc::new(Recording::default());
        let review = Arc::new(
            BoundOperationReview::new(reviewer.clone(), approvals.clone(), R2_DEADLINE)
                .with_observer(observer.clone())
                .with_human_consent(route.clone(), VALIDITY),
        );
        World {
            approvals,
            owner,
            work,
            reviewer,
            route,
            observer,
            review,
        }
    }

    fn approver(id: &str) -> AuthenticatedApprover {
        AuthenticatedApprover::new(
            PrincipalRef::in_domain(
                PrincipalKind::Human,
                id,
                TrustDomainId::new("team").expect("domain"),
            )
            .expect("principal"),
            ApprovalAssurance::PersonAuthenticated,
        )
        .expect("qualified approver")
    }

    fn decision(decision: ApprovalDecision, id: &str) -> AuthenticatedConsentDecision {
        AuthenticatedConsentDecision {
            approver: approver(id),
            decision,
            reason: None,
        }
    }

    fn human_consent_required() -> crate::ToolError {
        crate::ToolError::ReviewUnsatisfied {
            kind: ReviewUnsatisfiedKind::HumanConsentRequired,
            approval_id: None,
        }
    }

    impl World {
        /// A fresh prepared decision for a new candidate in this work.
        fn check(&self) -> PreparedOperationCheck {
            let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
                operation_id: OperationId::new(),
                execution_scope: OperationExecutionScope::Domain,
                run_id: None,
                context_revision: None,
                operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                    target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(
                        SessionId::new(),
                    )),
                    usage: SourceAuthorizationUse::Read,
                }),
            });
            PreparedOperationCheck::prepare(self.work.clone(), binding).expect("prepared")
        }

        async fn admit(
            &self,
            check: &PreparedOperationCheck,
            support: ReviewEntrySupport,
        ) -> Result<ReviewEntry, crate::ToolError> {
            let args = serde_json::value::RawValue::from_string(r#"{"to":"lead-1"}"#.into())
                .expect("args");
            let call = ToolCallView {
                id: "call-1",
                name: "send_message",
                args: &args,
            };
            admit_operation_review(Some(&self.review), support, call, check, &self.work).await
        }

        /// R3 admission of one candidate: it settles locally and issues one
        /// consent approval, whose id the host learns from the notice.
        async fn request(&self, check: &PreparedOperationCheck) -> ApprovalId {
            let settled = self.admit(check, ReviewEntrySupport::ConsumesAtEntry).await;
            let notices = self.route.notices.lock();
            let (id, kind) = notices.last().cloned().expect("requested notice");
            assert_eq!(kind, ConsentNoticeKind::Requested);
            // The local feedback carries the exact committed request.
            assert_eq!(
                settled.err(),
                Some(crate::ToolError::ReviewUnsatisfied {
                    kind: ReviewUnsatisfiedKind::HumanConsentRequired,
                    approval_id: Some(id.clone()),
                })
            );
            id
        }

        fn attempt(
            &self,
            id: &ApprovalId,
        ) -> Result<(ReviewEntry, PreparedOperationCheck), crate::ToolError> {
            admit_consent_attempt(Some(&self.review), id, &self.work, None)
        }

        /// A fresh attempt that reaches its leaf and spends there.
        fn spend(&self, id: &ApprovalId) -> Result<(), crate::ToolError> {
            let (entry, admitted) = self.attempt(id)?;
            let guard = ReviewDispatchGuard::new(entry.clone());
            let spent = entry.consume(&admitted);
            guard.complete();
            spent
        }

        /// A fresh attempt that enters with the decision its staging
        /// returns, as the fenced dispatch stages it: the canonical
        /// current/reprepare path of the admitted decision, through the
        /// check point, then the spend at its leaf.
        fn spend_staged(&self, id: &ApprovalId) -> Result<(), crate::ToolError> {
            let (entry, admitted) = self.attempt(id)?;
            let entering = admitted.current().map_err(crate::ToolError::from)?;
            let guard = ReviewDispatchGuard::new(entry.clone());
            let spent = entry
                .check(&entering)
                .and_then(|()| entry.consume(&entering));
            guard.complete();
            spent
        }

        /// The decision `id`'s live custody holds now.
        fn held(&self, id: &ApprovalId) -> PreparedOperationCheck {
            self.approvals
                .state
                .read()
                .consent
                .get(id)
                .expect("live custody")
                .admitted()
        }

        fn notices(&self, id: &ApprovalId) -> Vec<ConsentNoticeKind> {
            self.route
                .notices
                .lock()
                .iter()
                .filter(|(notice_id, _)| notice_id == id)
                .map(|(_, kind)| *kind)
                .collect()
        }

        fn statuses(&self) -> Statuses {
            self.observer.0.lock().clone()
        }

        fn record(&self, id: &ApprovalId) -> ApprovalRecord {
            self.approvals.get(id).expect("approval record")
        }
    }

    #[tokio::test]
    async fn r3_without_a_human_route_retains_nothing() {
        let approvals = ApprovalService::new();
        let review = Arc::new(BoundOperationReview::new(
            Arc::new(NoReviewer::default()),
            approvals.clone(),
            Duration::from_secs(30),
        ));
        let w = world();
        let check = w.check();
        let args = serde_json::value::RawValue::from_string("{}".into()).expect("args");
        let call = ToolCallView {
            id: "call-1",
            name: "send_message",
            args: &args,
        };
        let settled = admit_operation_review(
            Some(&review),
            ReviewEntrySupport::ConsumesAtEntry,
            call,
            &check,
            &w.work,
        )
        .await;
        assert_eq!(settled.err(), Some(human_consent_required()));
        assert!(
            approvals
                .list(ApprovalListFilter::default())
                .expect("list")
                .is_empty(),
            "no route: no approval and no retained attempt"
        );
    }

    #[tokio::test]
    async fn r3_with_a_route_issues_one_approval_and_settles_locally() {
        let w = world();
        let check = w.check();
        let id = w.request(&check).await;

        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Pending);
        assert_eq!(record.consent, Some(ApprovalConsentRecord::default()));
        assert_eq!(
            record.proposed_action.body,
            Some(serde_json::json!({"to": "lead-1"})),
            "the approver sees the exact binding-owned arguments"
        );
        assert_eq!(
            record.owner,
            ApprovalOwnerRef::ToolCall {
                tool_call_id: ToolCallId::new("call-1")
            }
        );
        assert!(record.expires_at.is_some(), "finite host-chosen validity");
        assert_eq!(w.notices(&id), vec![ConsentNoticeKind::Requested]);
        assert_eq!(w.statuses(), vec![(ReviewAttemptStatus::Pending, None)]);
        assert_eq!(
            w.reviewer.0.load(Ordering::SeqCst),
            0,
            "R3 is never model-reviewed"
        );
    }

    #[tokio::test]
    async fn an_unreadable_presentation_or_unsupported_leaf_issues_nothing() {
        let w = world();
        assert_eq!(
            w.admit(&w.check(), ReviewEntrySupport::Unsupported)
                .await
                .err(),
            Some(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::UnsupportedEntry,
            })
        );
        w.route.unreadable.store(true, Ordering::SeqCst);
        assert_eq!(
            w.admit(&w.check(), ReviewEntrySupport::ConsumesAtEntry)
                .await
                .err(),
            Some(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::OwnerUnavailable,
            })
        );
        assert!(
            w.approvals
                .list(ApprovalListFilter::default())
                .expect("list")
                .is_empty()
        );
        assert!(w.statuses().is_empty());
    }

    #[tokio::test]
    async fn caller_text_and_channel_membership_are_not_authentication() {
        let w = world();
        let id = w.request(&w.check()).await;

        // Caller-supplied actor text, the generic approval surface.
        assert_eq!(
            w.approvals.decide(
                &id,
                ApprovalDecision::Approve,
                ApprovalPrincipalId::new("human:lead-1").expect("principal"),
                None,
                None,
            ),
            Err(ApprovalError::AuthenticationRequired {
                approval_id: id.clone()
            })
        );
        // An authenticated member of the channel that shows the approval,
        // who is not an eligible approver for it.
        assert_eq!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "sibling")),
            Err(ApprovalError::NotEligible {
                approval_id: id.clone()
            })
        );
        // An unqualified identity, or one that is not a person, cannot even
        // be stated as an authenticated human approver.
        assert_eq!(
            AuthenticatedApprover::new(
                PrincipalRef::new(PrincipalKind::Human, "lead-1").expect("principal"),
                ApprovalAssurance::PersonAuthenticated,
            ),
            Err(ApprovalError::InvalidPrincipal)
        );
        assert_eq!(
            AuthenticatedApprover::new(
                PrincipalRef::in_domain(
                    PrincipalKind::ServiceAccount,
                    "channel-bot",
                    TrustDomainId::new("team").expect("domain"),
                )
                .expect("principal"),
                ApprovalAssurance::ServiceCredential,
            ),
            Err(ApprovalError::InvalidPrincipal)
        );

        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Pending);
        assert!(record.decision.is_none());
        assert_eq!(w.notices(&id), vec![ConsentNoticeKind::Requested]);
        assert_eq!(
            w.attempt(&id).err(),
            Some(human_consent_required()),
            "nothing became spendable"
        );
    }

    /// Eligibility is current at the commitment: a revocation published
    /// between its resolution and the commit leaves the approval pending,
    /// with no Approved notice and no spendable consent. The unchanged
    /// control approves.
    #[tokio::test]
    async fn an_eligibility_revoked_before_commit_records_nothing() {
        let w = world();
        let id = w.request(&w.check()).await;

        w.route.revoke_after_resolve.store(true, Ordering::Release);
        assert_eq!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::NotEligible {
                approval_id: id.clone()
            })
        );
        assert_eq!(w.record(&id).status, ApprovalStatus::Pending);
        assert_eq!(w.notices(&id), vec![ConsentNoticeKind::Requested]);
        assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));

        // Re-granted and unchanged since resolution: the decision commits.
        w.route
            .publish(|eligible| eligible.push("lead-1".to_string()));
        assert_eq!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
                .expect("approved")
                .status,
            ApprovalStatus::Approved
        );
        assert_eq!(w.spend(&id), Ok(()));
    }

    #[tokio::test]
    async fn an_authenticated_decision_only_records_and_notifies() {
        let w = world();
        let id = w.request(&w.check()).await;

        let decided = w
            .approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("eligible authenticated approval");

        assert_eq!(decided.status, ApprovalStatus::Approved);
        assert_eq!(
            decided.decision.as_ref().map(|d| d.actor.as_str()),
            Some("lead-1")
        );
        let consent = decided.consent.expect("consent facts");
        assert_eq!(
            consent.approver,
            Some(ApprovalConsentApprover {
                principal: approver("lead-1").principal().clone(),
                assurance: ApprovalAssurance::PersonAuthenticated,
            })
        );
        assert_eq!(consent.consumed_at, None, "recording is not spending");
        assert_eq!(
            w.notices(&id),
            vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Approved]
        );
        assert_eq!(
            w.statuses(),
            vec![
                (ReviewAttemptStatus::Pending, None),
                (ReviewAttemptStatus::Allowed, None),
            ],
            "the decision never used the attempt"
        );
        assert_eq!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Deny, "lead-1")),
            Err(ApprovalError::AlreadyDecided {
                approval_id: id.clone()
            }),
            "a duplicate decision adds nothing"
        );
    }

    #[tokio::test]
    async fn a_fresh_exact_attempt_spends_the_decision_once() {
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");

        let (entry, admitted) = w.attempt(&id).expect("fresh exact attempt");
        let guard = ReviewDispatchGuard::new(entry.clone());
        entry.consume(&admitted).expect("one spend at the leaf");
        assert!(
            !w.notices(&id).contains(&ConsentNoticeKind::Used),
            "Used is not delivered between the spend and the effect"
        );
        assert_eq!(
            entry.consume(&admitted),
            Err(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                approval_id: None,
            }),
            "a nested entry does not spend again"
        );
        guard.complete();
        assert_eq!(w.notices(&id).last(), Some(&ConsentNoticeKind::Used));
        assert_eq!(
            w.statuses().last(),
            Some(&(ReviewAttemptStatus::Used, None))
        );
        assert!(
            w.record(&id)
                .consent
                .and_then(|consent| consent.consumed_at)
                .is_some(),
            "the spend is a durable fact"
        );
        assert_eq!(
            w.attempt(&id).err(),
            Some(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                approval_id: None,
            }),
            "a second fresh attempt never enters"
        );
    }

    #[tokio::test]
    async fn copied_references_and_equal_coordinates_have_no_consent() {
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");

        let lookalike = WorkAuthorizationContext::new(
            Arc::clone(w.work.authorization()),
            w.work.execution_scope().clone(),
        );
        for refused in [
            admit_consent_attempt(Some(&w.review), &id, &lookalike, None),
            admit_consent_attempt(Some(&w.review), &id, &w.work, Some(&RunId::new())),
            admit_consent_attempt(Some(&w.review), &ApprovalId::new(), &w.work, None),
        ] {
            assert_eq!(refused.err(), Some(human_consent_required()));
        }
        // Another owner's review composition holds no custody of it.
        let other = world();
        assert_eq!(
            admit_consent_attempt(Some(&other.review), &id, &w.work, None).err(),
            Some(human_consent_required())
        );

        assert_eq!(w.spend(&id), Ok(()), "the exact attempt still spends once");
    }

    #[tokio::test]
    async fn a_new_call_with_equal_arguments_requests_fresh_consent() {
        let w = world();
        let first = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&first, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");

        // Same tool, same arguments, same work: a new binding is a new
        // candidate with its own approval, never the approved one.
        let second = w.request(&w.check()).await;
        assert_ne!(first, second);
        assert_eq!(w.record(&second).status, ApprovalStatus::Pending);
        assert_eq!(w.spend(&first), Ok(()));
        assert_eq!(w.spend(&second).err(), Some(human_consent_required()));
    }

    #[tokio::test(start_paused = true)]
    async fn expiry_prevents_the_decision_and_the_use() {
        // Expired while pending: the decision is refused.
        let w = world();
        let pending = w.request(&w.check()).await;
        tokio::time::advance(VALIDITY + Duration::from_secs(1)).await;
        assert_eq!(
            w.approvals
                .decide_consent(&pending, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::Expired {
                approval_id: pending.clone()
            })
        );
        assert_eq!(w.record(&pending).status, ApprovalStatus::Expired);
        assert_eq!(
            w.statuses().last(),
            Some(&(
                ReviewAttemptStatus::Retired,
                Some(ReviewRetirementReason::DeadlineExpired)
            ))
        );

        // Approved, then expired before the attempt: never resolves.
        let w = world();
        let approved = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&approved, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved in time");
        tokio::time::advance(VALIDITY + Duration::from_secs(1)).await;
        assert_eq!(w.attempt(&approved).err(), Some(human_consent_required()));

        // Approved and resolved in time, expired before the leaf: no spend.
        let w = world();
        let late = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&late, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved in time");
        let (entry, admitted) = w.attempt(&late).expect("resolved in time");
        tokio::time::advance(VALIDITY + Duration::from_secs(1)).await;
        let guard = ReviewDispatchGuard::new(entry.clone());
        assert_eq!(entry.consume(&admitted), Err(human_consent_required()));
        guard.complete();
        assert_eq!(
            w.statuses().last(),
            Some(&(
                ReviewAttemptStatus::Retired,
                Some(ReviewRetirementReason::DeadlineExpired)
            ))
        );
        assert!(
            w.record(&late)
                .consent
                .and_then(|consent| consent.consumed_at)
                .is_none()
        );
    }

    /// ADR-001 D9 at the attempt and its spend. A decision that an unrelated
    /// owner change made stale between the approval and the attempt is
    /// decided afresh before the attempt is handed it, and the attempt enters
    /// once. One that goes stale after the attempt's admission, before its
    /// staging, is re-adjudicated by the spend against the decision the
    /// staging prepared afresh, which it holds and spends. A related change
    /// (here the review tier the owners now require) refuses the spend with
    /// zero entry and withdraws the consent as `ContextChanged`.
    #[tokio::test]
    async fn a_spend_re_adjudicates_a_decision_an_owner_change_made_stale() {
        // Unrelated, before the attempt: the same tier and material.
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let consented = w.held(&id);
        w.owner.republish();
        assert!(
            consented.check_retained_current().is_err(),
            "the consented decision is stale"
        );
        assert_eq!(
            w.spend_staged(&id),
            Ok(()),
            "spent against the decision decided afresh"
        );
        let consent = w.record(&id).consent.expect("consent history");
        assert!(consent.consumed_at.is_some(), "spent once");
        assert!(consent.withdrawal.is_none(), "never withdrawn");
        assert_eq!(w.notices(&id).last(), Some(&ConsentNoticeKind::Used));
        assert_eq!(
            w.spend_staged(&id),
            Err(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                approval_id: None,
            })
        );

        // Unrelated, after the attempt's admission: the spend re-adjudicates
        // the staging's fresh decision, holds it and spends it.
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let (entry, admitted) = w.attempt(&id).expect("resolved");
        w.owner.republish();
        let entering = admitted.current().expect("staged afresh");
        assert!(!entering.same_check(&admitted), "a fresh decision");
        let guard = ReviewDispatchGuard::new(entry.clone());
        assert_eq!(entry.check(&entering), Ok(()));
        assert_eq!(entry.consume(&entering), Ok(()), "spent against it");
        guard.complete();
        assert!(w.record(&id).consent.is_some_and(|consent| {
            consent.consumed_at.is_some() && consent.withdrawal.is_none()
        }));

        // Related: the owners now require R2 for the operation.
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        w.owner.r2.store(true, Ordering::Release);
        w.owner.republish();
        assert_eq!(
            w.spend_staged(&id),
            Err(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::ContextChanged,
                approval_id: None,
            })
        );
        assert_eq!(
            w.statuses().last(),
            Some(&(
                ReviewAttemptStatus::Retired,
                Some(ReviewRetirementReason::ContextChanged)
            ))
        );
        let record = w.record(&id);
        assert!(
            record
                .consent
                .as_ref()
                .is_some_and(|consent| consent.consumed_at.is_none()),
            "nothing spent"
        );
        assert_eq!(
            withdrawal_reason(&record),
            Some(ApprovalConsentWithdrawalReason::ContextChanged)
        );
        assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));
        assert!(!w.notices(&id).contains(&ConsentNoticeKind::Used));
    }

    /// ADR-001 D9: a spend never enters on a decision that is not current.
    /// An owner change during the spend's own re-adjudication (right after
    /// its material read), or after the entering decision's staging, enters
    /// nothing and leaves the consent unspent and unwithdrawn; a later fresh
    /// attempt is handed a decision decided afresh and enters once.
    #[tokio::test]
    async fn a_change_during_a_spend_never_spends_a_stale_decision() {
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let owner_unavailable = Err(crate::ToolError::ReviewUnavailable {
            kind: ReviewUnavailableKind::OwnerUnavailable,
        });
        let kept = |w: &World, id: &ApprovalId| {
            w.record(id).consent.is_some_and(|consent| {
                consent.consumed_at.is_none() && consent.withdrawal.is_none()
            })
        };

        // During the spend's re-adjudication of the staging's fresh decision.
        let (entry, admitted) = w.attempt(&id).expect("resolved");
        w.owner.republish();
        w.owner
            .republish_after_material_read
            .store(1, Ordering::Release);
        let entering = admitted.current().expect("staged afresh");
        let guard = ReviewDispatchGuard::new(entry.clone());
        assert_eq!(entry.check(&entering), Ok(()));
        assert_eq!(entry.consume(&entering), owner_unavailable);
        guard.complete();
        assert!(kept(&w, &id), "unspent and unwithdrawn");

        // After the entering decision's staging, before its spend.
        let (entry, admitted) = w.attempt(&id).expect("resolved");
        let entering = admitted.current().expect("current");
        let guard = ReviewDispatchGuard::new(entry.clone());
        assert_eq!(entry.check(&entering), Ok(()));
        w.owner.republish();
        assert_eq!(entry.consume(&entering), owner_unavailable);
        guard.complete();
        assert!(kept(&w, &id), "unspent and unwithdrawn");
        let notices = w.notices(&id);
        assert!(!notices.contains(&ConsentNoticeKind::Used), "{notices:?}");
        assert!(
            !notices.contains(&ConsentNoticeKind::Withdrawn),
            "{notices:?}"
        );

        // A later fresh attempt is handed a decision decided afresh, and
        // enters once.
        assert_eq!(w.spend_staged(&id), Ok(()));
        assert!(
            w.record(&id)
                .consent
                .is_some_and(|consent| consent.consumed_at.is_some())
        );
    }

    /// A run's own attempt whose spend the approval owner could not settle
    /// now (here its store wrote nothing), with the consent kept, counts as
    /// not made: the run's next boundary attempts it again, and it enters
    /// once.
    #[tokio::test]
    async fn an_owner_unavailable_spend_counts_as_a_run_attempt_not_made() {
        let store = Arc::new(FaultStore::default());
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let tool = Arc::new(EnteringTool::default());
        let (id, context) = fenced_consent(&w, &tool).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        *store.fault.lock() = Some(Fault::BeforeWrite);
        assert!(
            matches!(
                crate::agent::dispatch_retained_consent_in_run(&tool, &context, &id).await,
                crate::agent::RetainedConsentAttempt::NotReady
            ),
            "not made"
        );
        assert_eq!(tool.0.load(Ordering::SeqCst), 0, "nothing entered");
        assert!(
            w.record(&id).consent.is_some_and(
                |consent| consent.consumed_at.is_none() && consent.withdrawal.is_none()
            ),
            "the consent is kept"
        );
        assert!(
            matches!(
                crate::agent::dispatch_retained_consent_in_run(&tool, &context, &id).await,
                crate::agent::RetainedConsentAttempt::Settled(Ok(_))
            ),
            "the next attempt is made"
        );
        assert_eq!(tool.0.load(Ordering::SeqCst), 1, "entered once");
    }

    /// ADR-001 D9 at an attempt's admission: the held decision is settled
    /// before the attempt is handed it, and an attempt is never handed one
    /// that is not current. A related change (the review tier the owners now
    /// require) withdraws the consent and refuses the attempt as
    /// `ContextChanged`; the owners' refusal (a permission they withdrew)
    /// withdraws it as `AuthorityChanged` and refuses the attempt with that
    /// refusal, observed by its owner once. While nothing can be settled (an
    /// owner change in progress) the attempt refuses as `owner_unavailable`
    /// with nothing entered, a run's own attempt counts as not made, and the
    /// consent is kept: once the change completed, the next attempt enters
    /// once.
    #[tokio::test]
    async fn an_attempt_is_handed_only_a_current_decision() {
        // Related.
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        w.owner.r2.store(true, Ordering::Release);
        w.owner.republish();
        assert_eq!(
            w.attempt(&id).err(),
            Some(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::ContextChanged,
                approval_id: None,
            })
        );
        assert_eq!(
            withdrawal_reason(&w.record(&id)),
            Some(ApprovalConsentWithdrawalReason::ContextChanged)
        );

        // Refused.
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let observed = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&observed);
        *w.owner.on_observe.lock() = Some(Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        }));
        w.owner.denied.store(true, Ordering::Release);
        assert_eq!(
            w.attempt(&id).err(),
            Some(crate::ToolError::AuthorizationRefused {
                refusal: OperationRefused::new(OperationRefusalKind::Denied),
            })
        );
        assert_eq!(observed.load(Ordering::SeqCst), 1, "observed once");
        let record = w.record(&id);
        assert_eq!(
            withdrawal_reason(&record),
            Some(ApprovalConsentWithdrawalReason::AuthorityChanged)
        );
        assert!(
            record
                .consent
                .as_ref()
                .is_some_and(|consent| consent.consumed_at.is_none()),
            "nothing spent"
        );
        assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));

        // Nothing can be settled now.
        let w = world();
        let tool = Arc::new(EnteringTool::default());
        let (id, context) = fenced_consent(&w, &tool).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        w.owner.republish();
        w.owner.in_progress.store(true, Ordering::Release);
        assert_eq!(w.attempt(&id).err(), Some(owner_unavailable()));
        assert!(
            matches!(
                crate::agent::dispatch_retained_consent_in_run(&tool, &context, &id).await,
                crate::agent::RetainedConsentAttempt::NotReady
            ),
            "not made"
        );
        assert_eq!(tool.0.load(Ordering::SeqCst), 0, "nothing entered");
        assert!(
            w.record(&id).consent.is_some_and(
                |consent| consent.consumed_at.is_none() && consent.withdrawal.is_none()
            ),
            "the consent is kept"
        );
        w.owner.in_progress.store(false, Ordering::Release);
        assert!(
            matches!(
                crate::agent::dispatch_retained_consent_in_run(&tool, &context, &id).await,
                crate::agent::RetainedConsentAttempt::Settled(Ok(_))
            ),
            "the next attempt is made"
        );
        assert_eq!(tool.0.load(Ordering::SeqCst), 1, "entered once");
    }

    /// A run's own attempt whose effect entered keeps that effect's own
    /// outcome, whatever its kind: an `owner_unavailable` the entered body
    /// itself answered is the attempt's settled result, never an attempt not
    /// made (its consent is spent).
    #[tokio::test]
    async fn an_entered_effects_own_owner_unavailable_answer_is_its_outcome() {
        let w = world();
        let tool = Arc::new(EnteringTool::default());
        tool.1.store(true, Ordering::Release);
        let (id, context) = fenced_consent(&w, &tool).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        assert!(
            matches!(
                crate::agent::dispatch_retained_consent_in_run(&tool, &context, &id).await,
                crate::agent::RetainedConsentAttempt::Settled(Err(
                    crate::ToolError::ReviewUnavailable {
                        kind: ReviewUnavailableKind::OwnerUnavailable,
                    }
                ))
            ),
            "the entered effect's own answer"
        );
        assert_eq!(tool.0.load(Ordering::SeqCst), 1, "entered once");
        assert!(
            w.record(&id)
                .consent
                .is_some_and(|consent| consent.consumed_at.is_some()),
            "spent"
        );
    }

    /// An attempt never waits for the approval owner (attempts run on async
    /// threads). Right after its admission resolved the consent, while it
    /// settles a held decision an owner change made stale, an unrelated
    /// human decision takes the owner's reservation and holds it through its
    /// store write: the attempt returns within its bound as
    /// `owner_unavailable`, with nothing entered and the consent kept. Once
    /// the owner is released, the next attempt enters once.
    #[tokio::test]
    async fn an_attempt_never_waits_for_a_held_owner_reservation() {
        let (w, store, held, release) = gated_world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let unrelated = w
            .approvals
            .request(ApprovalRequest {
                requester: ApprovalPrincipalId::new("human:child").expect("principal"),
                owner: ApprovalOwnerRef::Runtime,
                resource: ApprovalResourceRef {
                    kind: ApprovalResourceKind::Runtime,
                    id: ApprovalResourceId::new("local"),
                },
                proposed_action: ApprovalProposedAction {
                    kind: ApprovalActionKind::Other,
                    summary: "manual gate".to_string(),
                    body: None,
                },
                risk: ApprovalRisk::Low,
                request_body: None,
                allowed_decisions: [ApprovalDecision::Approve].into_iter().collect(),
                expires_at: None,
                metadata: SurfaceMetadata::default(),
                request_provenance: None,
            })
            .expect("unrelated approval")
            .approval_id;
        w.owner.republish();
        // Inside the admission's re-adjudication (its fresh preparation),
        // after the consent resolved.
        let approvals = w.approvals.clone();
        let (holder_tx, holder) = mpsc::channel();
        *w.owner.on_prepare.lock() = Some(Box::new(move || {
            store.armed.store(true, Ordering::Release);
            let holding = std::thread::spawn(move || {
                approvals
                    .decide(
                        &unrelated,
                        ApprovalDecision::Approve,
                        ApprovalPrincipalId::new("human:operator").expect("principal"),
                        None,
                        None,
                    )
                    .expect("decided");
            });
            held.recv_timeout(BOUND)
                .expect("the unrelated decision holds the reservation");
            let _ = holder_tx.send(holding);
        }));
        assert_eq!(
            w.attempt(&id).err(),
            Some(owner_unavailable()),
            "a held owner refuses at once; the attempt never waits"
        );
        let _ = release.send(());
        holder
            .recv_timeout(BOUND)
            .expect("the holder")
            .join()
            .expect("holder thread");
        let record = w.record(&id);
        assert!(
            record.consent.as_ref().is_some_and(
                |consent| consent.consumed_at.is_none() && consent.withdrawal.is_none()
            ),
            "the consent is kept"
        );
        assert!(!w.notices(&id).contains(&ConsentNoticeKind::Used));
        assert_eq!(w.spend_staged(&id), Ok(()), "the next attempt enters once");
        assert!(
            w.record(&id)
                .consent
                .is_some_and(|consent| consent.consumed_at.is_some())
        );
    }

    #[tokio::test]
    async fn a_refused_spend_is_observed_only_after_the_owner_released() {
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let (entry, admitted) = w.attempt(&id).expect("resolved");

        // The observer re-enters the approval owner; the permission is
        // withdrawn between staging and the spend.
        let reentered = Arc::new(AtomicUsize::new(0));
        let approvals = w.approvals.clone();
        let count = Arc::clone(&reentered);
        *w.owner.on_observe.lock() = Some(Arc::new(move || {
            approvals
                .list(ApprovalListFilter::default())
                .expect("the owner is free while a refusal is observed");
            count.fetch_add(1, Ordering::SeqCst);
        }));
        w.owner.denied.store(true, Ordering::Release);

        let (done, spent) = mpsc::channel();
        let spender = std::thread::spawn(move || {
            let guard = ReviewDispatchGuard::new(entry.clone());
            let result = entry.consume(&admitted);
            guard.complete();
            let _ = done.send(result);
        });
        let result = spent
            .recv_timeout(BOUND)
            .expect("no deadlock: the observation runs outside every owner lock");
        spender.join().expect("spender");
        assert_eq!(
            result,
            Err(crate::ToolError::AuthorizationRefused {
                refusal: OperationRefused::new(OperationRefusalKind::Denied),
            })
        );
        assert_eq!(reentered.load(Ordering::SeqCst), 1, "observed once");
        assert_eq!(
            w.statuses().last(),
            Some(&(
                ReviewAttemptStatus::Retired,
                Some(ReviewRetirementReason::AuthorityChanged)
            ))
        );
        assert!(!w.notices(&id).contains(&ConsentNoticeKind::Used));

        // Unchanged permitted control: fresh consent still enters once.
        *w.owner.on_observe.lock() = None;
        w.owner.denied.store(false, Ordering::Release);
        let fresh = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&fresh, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        assert_eq!(w.spend(&fresh), Ok(()));
    }

    /// One builtin tool whose review the dispatcher consumes at its entry;
    /// it counts the bodies it runs.
    #[derive(Default)]
    struct ConsumingTool(AtomicUsize);

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl crate::AgentToolDispatcher for ConsumingTool {
        fn tools(&self) -> Arc<[Arc<crate::types::ToolDef>]> {
            [Arc::new(
                crate::types::ToolDef::new("read_file", "", serde_json::json!({"type": "object"}))
                    .with_provenance(crate::types::ToolProvenance::new(
                        crate::types::ToolSourceKind::Builtin,
                        "fixture",
                    )),
            )]
            .into()
        }

        fn review_entry_support(&self, _tool_name: &str) -> ReviewEntrySupport {
            ReviewEntrySupport::ConsumesAtEntry
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(crate::types::ToolResult::new(call.id.to_string(), "ran".to_string(), false).into())
        }
    }

    /// The host's operation refusal observer: it keeps what it is given.
    #[derive(Default)]
    struct RefusalLog(parking_lot::Mutex<Vec<crate::authorization::OperationRefusalObservation>>);

    impl crate::authorization::OperationRefusalObserver for RefusalLog {
        fn observe(&self, observation: &crate::authorization::OperationRefusalObservation) {
            self.0.lock().push(observation.clone());
        }
    }

    /// A retained consent attempt refused after its candidate resolved, here
    /// because the owner withdrew the permission after the approval, reaches
    /// the host's operation refusal observer once: as the retained call's
    /// result, under the retained operation, with the dispatch fact its model
    /// payload carries. The attempt met the refusal at its staging, before the
    /// retained call's dispatcher ran, so the model reads it certified not
    /// dispatched (`tool_invoked: false`). The first attempt's consent request
    /// is not a refusal and is not observed.
    #[tokio::test]
    async fn a_refused_consent_attempt_reaches_the_host_observer_as_its_call() {
        let w = world();
        let log = Arc::new(RefusalLog::default());
        let session = SessionId::new();
        let context = crate::ToolDispatchContext::default()
            .with_work_authorization(Some(w.work.clone()))
            .with_operation_review(Some(Arc::clone(&w.review)))
            .with_refusal_observer(Some(crate::authorization::RefusalObserverBinding::new(
                Arc::clone(&log) as Arc<dyn crate::authorization::OperationRefusalObserver>,
                session.clone(),
            )));
        let dispatcher = Arc::new(ConsumingTool::default());
        let args = serde_json::value::RawValue::from_string("{}".into()).expect("args");
        let call = ToolCallView {
            id: "call-1",
            name: "read_file",
            args: &args,
        };
        let resolution = crate::ToolExecutionResolutionContext::new(
            crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
                crate::ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(60),
            )])
            .expect("deadline chain"),
        );
        let plan =
            crate::resolve_tool_execution_plan_fenced(&dispatcher, call, &context, &resolution)
                .expect("plan");
        let first =
            crate::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &plan).await;
        let (id, kind) = w
            .route
            .notices
            .lock()
            .last()
            .cloned()
            .expect("requested notice");
        assert_eq!(kind, ConsentNoticeKind::Requested);
        assert_eq!(
            first.err(),
            Some(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::HumanConsentRequired,
                approval_id: Some(id.clone()),
            })
        );
        assert!(
            log.0.lock().is_empty(),
            "a consent request is not a refusal"
        );
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        w.owner.denied.store(true, Ordering::Release);

        let result = crate::dispatch_retained_consent_attempt(&dispatcher, &context, &id).await;
        let refusal = result
            .as_ref()
            .err()
            .and_then(|error| match error {
                crate::ToolError::AuthorizationRefused { refusal } => Some(*refusal),
                _ => None,
            })
            .expect("the withdrawn permission refuses the attempt");
        assert_eq!(refusal.kind(), OperationRefusalKind::Denied);
        assert_eq!(
            refusal.dispatch(),
            crate::authorization::RefusalDispatch::NotDispatched
        );
        assert_eq!(
            result
                .as_ref()
                .err()
                .map(|error| error.to_error_payload()["data"].clone()),
            Some(serde_json::json!({"kind": "denied", "tool_invoked": false})),
            "the model reads the certified refusal"
        );
        assert_eq!(dispatcher.0.load(Ordering::SeqCst), 0, "no body ran");
        let observed = log.0.lock().clone();
        assert_eq!(observed.len(), 1, "{observed:?}");
        let observation = &observed[0];
        assert_eq!(observation.session_id(), &session);
        assert_eq!(
            observation.surface(),
            &crate::authorization::OperationRefusalSurface::ToolCall {
                tool_use_id: "call-1".into()
            }
        );
        assert_eq!(
            observation.outcome(),
            crate::authorization::OperationRefusalOutcome::Refused {
                kind: OperationRefusalKind::Denied,
                dispatch: crate::authorization::RefusalDispatch::NotDispatched,
            }
        );
        assert_eq!(
            observation.disposition(),
            crate::authorization::OperationRefusalDisposition::ToolResult
        );
        assert!(observation.operation_id().is_some());
    }

    /// One builtin tool whose leaf enters its effect through the reviewed
    /// entry, which spends a consent there, and counts the bodies it runs
    /// after that entry. While its flag is set, the entered body itself
    /// answers `owner_unavailable`.
    #[derive(Default)]
    struct EnteringTool(AtomicUsize, AtomicBool);

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl crate::AgentToolDispatcher for EnteringTool {
        fn tools(&self) -> Arc<[Arc<crate::types::ToolDef>]> {
            [Arc::new(
                crate::types::ToolDef::new("read_file", "", serde_json::json!({"type": "object"}))
                    .with_provenance(crate::types::ToolProvenance::new(
                        crate::types::ToolSourceKind::Builtin,
                        "fixture",
                    )),
            )]
            .into()
        }

        fn review_entry_support(&self, _tool_name: &str) -> ReviewEntrySupport {
            ReviewEntrySupport::ConsumesAtEntry
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
            self.dispatch_with_context(call, &crate::ToolDispatchContext::default())
                .await
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &crate::ToolDispatchContext,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
            let _entered = context.enter_reviewed_effect(call, None)?;
            self.0.fetch_add(1, Ordering::SeqCst);
            if self.1.load(Ordering::Acquire) {
                return Err(owner_unavailable());
            }
            Ok(crate::types::ToolResult::new(call.id.to_string(), "ran".to_string(), false).into())
        }
    }

    /// The R3 call of `tool` through the real fenced entry, under `w`'s work
    /// and review: it settles locally with one consent approval. Returns
    /// that approval and the dispatch context a fresh attempt uses.
    async fn fenced_consent(
        w: &World,
        tool: &Arc<EnteringTool>,
    ) -> (ApprovalId, crate::ToolDispatchContext) {
        let context = crate::ToolDispatchContext::default()
            .with_work_authorization(Some(w.work.clone()))
            .with_operation_review(Some(Arc::clone(&w.review)));
        let args = serde_json::value::RawValue::from_string("{}".into()).expect("args");
        let call = ToolCallView {
            id: "call-1",
            name: "read_file",
            args: &args,
        };
        let resolution = crate::ToolExecutionResolutionContext::new(
            crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
                crate::ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(60),
            )])
            .expect("deadline chain"),
        );
        let plan = crate::resolve_tool_execution_plan_fenced(tool, call, &context, &resolution)
            .expect("plan");
        let first = crate::dispatch_tool_execution_plan_fenced(tool, call, &context, &plan).await;
        let (id, kind) = w
            .route
            .notices
            .lock()
            .last()
            .cloned()
            .expect("requested notice");
        assert_eq!(kind, ConsentNoticeKind::Requested);
        assert_eq!(
            first.err(),
            Some(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::HumanConsentRequired,
                approval_id: Some(id.clone()),
            })
        );
        (id, context)
    }

    /// ADR-001 D9 through the real fenced entry. An unrelated owner change
    /// while the consent is pending keeps it pending; after the human's
    /// approval another unrelated change makes the approved decision stale,
    /// and the fresh attempt's stagings prepare it afresh: its leaf spends
    /// the consent against that decision and the operation enters exactly
    /// once. A related change after the approval (the material the approval
    /// presented) refuses the attempt with zero entry and withdraws it.
    #[tokio::test]
    async fn an_unrelated_change_keeps_the_consent_and_the_operation_enters_once() {
        for related in [false, true] {
            let w = world();
            w.owner.set_material(Some("team-checking"));
            let tool = Arc::new(EnteringTool::default());
            let (id, context) = fenced_consent(&w, &tool).await;
            w.owner.republish();
            assert_eq!(
                w.record(&id).status,
                ApprovalStatus::Pending,
                "related={related}"
            );
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
                .expect("approved");
            if related {
                w.owner.set_material(Some("team-savings"));
            } else {
                w.owner.republish();
            }
            let attempt = crate::dispatch_retained_consent_attempt(&tool, &context, &id).await;
            let consent = w.record(&id).consent.expect("consent history");
            if related {
                assert!(
                    matches!(
                        attempt,
                        Err(crate::ToolError::ReviewUnsatisfied {
                            kind: ReviewUnsatisfiedKind::ContextChanged,
                            ..
                        })
                    ),
                    "{attempt:?}"
                );
                assert_eq!(tool.0.load(Ordering::SeqCst), 0, "zero entry");
                assert!(consent.consumed_at.is_none(), "nothing spent");
                assert_eq!(
                    consent.withdrawal.map(|withdrawal| withdrawal.reason),
                    Some(ApprovalConsentWithdrawalReason::ContextChanged)
                );
            } else {
                assert!(attempt.is_ok(), "{attempt:?}");
                assert_eq!(tool.0.load(Ordering::SeqCst), 1, "entered once");
                assert!(consent.consumed_at.is_some(), "spent once");
                assert_eq!(consent.withdrawal, None);
                let again = crate::dispatch_retained_consent_attempt(&tool, &context, &id).await;
                assert!(
                    matches!(
                        again,
                        Err(crate::ToolError::ReviewUnsatisfied {
                            kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                            ..
                        })
                    ),
                    "{again:?}"
                );
                assert_eq!(tool.0.load(Ordering::SeqCst), 1, "never twice");
            }
        }
    }

    #[tokio::test]
    async fn a_declined_consent_never_enters() {
        let w = world();
        let id = w.request(&w.check()).await;
        let declined = w
            .approvals
            .decide_consent(&id, decision(ApprovalDecision::Deny, "lead-1"))
            .expect("declined");
        assert_eq!(declined.status, ApprovalStatus::Denied);
        assert_eq!(
            w.notices(&id),
            vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Denied]
        );
        assert_eq!(
            w.attempt(&id).err(),
            Some(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::Denied,
                approval_id: None,
            })
        );
    }

    #[tokio::test]
    async fn a_failure_before_entry_keeps_the_consent_spendable() {
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");

        // The attempt's leaf fails during preparation and never enters.
        let (entry, _admitted) = w.attempt(&id).expect("resolved");
        ReviewDispatchGuard::new(entry).complete();
        // A cancelled attempt is dropped the same way.
        let (entry, _admitted) = w.attempt(&id).expect("resolved");
        drop(ReviewDispatchGuard::new(entry));

        assert_eq!(w.spend(&id), Ok(()), "a later fresh attempt spends once");
    }

    /// Old clones of an R3 attempt keep their terminal disposition through
    /// the real check point and entry: a spent clone stays already consumed,
    /// and a cancelled or pre-entry-failed dispatch's clone stays ended. A
    /// fresh retained attempt still spends once.
    #[tokio::test]
    async fn stale_r3_clones_keep_their_terminal_disposition() {
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let dispatch_ended = Err(crate::ToolError::ReviewUnavailable {
            kind: ReviewUnavailableKind::DispatchEnded,
        });

        // A cancelled attempt and a pre-entry failure leave old clones.
        let (cancelled, admitted) = w.attempt(&id).expect("resolved");
        let cancelled_clone = cancelled.clone();
        drop(ReviewDispatchGuard::new(cancelled));
        let (failed, _) = w.attempt(&id).expect("resolved");
        let failed_clone = failed.clone();
        ReviewDispatchGuard::new(failed).complete();
        for clone in [&cancelled_clone, &failed_clone] {
            assert_eq!(clone.check(&admitted), dispatch_ended);
            assert_eq!(clone.consume(&admitted), dispatch_ended);
        }

        // A fresh attempt spends once; its clone stays spent.
        let (entry, admitted) = w.attempt(&id).expect("fresh attempt");
        let spent_clone = entry.clone();
        let guard = ReviewDispatchGuard::new(entry.clone());
        assert_eq!(entry.consume(&admitted), Ok(()));
        guard.complete();
        assert_eq!(spent_clone.check(&admitted), Ok(()));
        assert_eq!(
            spent_clone.consume(&admitted),
            Err(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                approval_id: None,
            })
        );
        assert_eq!(
            w.notices(&id)
                .iter()
                .filter(|kind| **kind == ConsentNoticeKind::Used)
                .count(),
            1
        );
    }

    /// R2 human fallback with a configured route: an escalation or an
    /// expired deadline settles and retires the model attempt first (keeping
    /// its own outcome), then the same human owner gets one consent request
    /// for the candidate, which a fresh attempt spends once. A model result
    /// ready only after the deadline never wins.
    #[tokio::test(start_paused = true)]
    async fn r2_escalation_and_deadline_fall_back_to_the_human_route() {
        for (script, model_outcome) in [
            (
                Scripted::Verdict(ReviewVerdict::Escalate),
                (ReviewAttemptStatus::Escalated, None),
            ),
            (
                Scripted::Hang,
                (
                    ReviewAttemptStatus::Retired,
                    Some(ReviewRetirementReason::DeadlineExpired),
                ),
            ),
            (
                Scripted::LateVerdict(ReviewVerdict::Allow),
                (
                    ReviewAttemptStatus::Retired,
                    Some(ReviewRetirementReason::DeadlineExpired),
                ),
            ),
        ] {
            let w = world();
            w.owner.r2.store(true, Ordering::Release);
            *w.reviewer.1.lock() = Some(script);
            let id = w.request(&w.check()).await;
            let statuses = w.statuses();
            assert_eq!(
                statuses.first(),
                Some(&(ReviewAttemptStatus::Pending, None)),
                "the model attempt began"
            );
            assert_eq!(
                statuses.get(1),
                Some(&model_outcome),
                "its own outcome settles first"
            );
            assert!(
                !statuses.contains(&(ReviewAttemptStatus::Allowed, None)),
                "a late model allow never wins"
            );
            assert_eq!(
                statuses.last(),
                Some(&(ReviewAttemptStatus::Pending, None)),
                "then the human attempt is pending"
            );
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
                .expect("approved");
            assert_eq!(w.spend(&id), Ok(()), "the human-approved candidate enters");
        }
    }

    /// Without a human route an R2 escalation stays local feedback, and an
    /// ordinary reviewer failure never falls back even with one: it cannot
    /// yet be told apart from a protected observation failure.
    #[tokio::test]
    async fn r2_without_a_route_or_on_reviewer_failure_stays_local() {
        let w = world();
        w.owner.r2.store(true, Ordering::Release);
        let local = Arc::new(BoundOperationReview::new(
            w.reviewer.clone(),
            w.approvals.clone(),
            R2_DEADLINE,
        ));
        *w.reviewer.1.lock() = Some(Scripted::Verdict(ReviewVerdict::Escalate));
        let args = serde_json::value::RawValue::from_string("{}".into()).expect("args");
        let call = ToolCallView {
            id: "call-1",
            name: "send_message",
            args: &args,
        };
        assert_eq!(
            admit_operation_review(
                Some(&local),
                ReviewEntrySupport::ConsumesAtEntry,
                call,
                &w.check(),
                &w.work
            )
            .await
            .err(),
            Some(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::Escalated,
                approval_id: None,
            })
        );
        *w.reviewer.1.lock() = None;
        assert_eq!(
            w.admit(&w.check(), ReviewEntrySupport::ConsumesAtEntry)
                .await
                .err(),
            Some(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::ReviewerFailed,
            })
        );
        assert!(
            w.approvals
                .list(ApprovalListFilter::default())
                .expect("list")
                .is_empty(),
            "no consent request without a fallback"
        );
    }

    /// The admitting run closed: its unspent consent with no exact native
    /// destination is withdrawn at once, recorded durably and announced. A pending approval is cancelled (not
    /// denied), approved history stays approved, neither can be decided or
    /// spent, and another work association's consent continues.
    #[tokio::test]
    async fn a_closed_run_withdraws_its_unspent_consent() {
        let w = world();
        let other = world_with(w.approvals.clone());
        let pending = w.request(&w.check()).await;
        let approved = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&approved, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let unrelated = other.request(&other.check()).await;

        w.review.close_consent_for_work(&w.work);

        let withdrawn = |id: &ApprovalId| {
            w.record(id)
                .consent
                .and_then(|consent| consent.withdrawal)
                .map(|withdrawal| withdrawal.reason)
        };
        assert_eq!(w.record(&pending).status, ApprovalStatus::Cancelled);
        assert_eq!(
            withdrawn(&pending),
            Some(ApprovalConsentWithdrawalReason::RunEnded)
        );
        assert_eq!(
            w.notices(&pending),
            vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Withdrawn]
        );
        assert_eq!(
            w.approvals
                .decide_consent(&pending, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::ConsentUnavailable {
                approval_id: pending.clone()
            }),
            "a late decision is refused"
        );
        assert!(!w.notices(&pending).contains(&ConsentNoticeKind::Approved));

        assert_eq!(w.record(&approved).status, ApprovalStatus::Approved);
        assert_eq!(
            withdrawn(&approved),
            Some(ApprovalConsentWithdrawalReason::RunEnded)
        );
        assert_eq!(w.attempt(&approved).err(), Some(human_consent_required()));
        assert!(!w.notices(&approved).contains(&ConsentNoticeKind::Used));

        // Unrelated work continues.
        other
            .approvals
            .decide_consent(&unrelated, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("unrelated approval");
        assert_eq!(other.spend(&unrelated), Ok(()));
    }

    /// ADR-001 D9 on approval reads: a pending consent whose decision an
    /// unrelated owner change made stale (here it moves the validity window
    /// decisions are prepared with, as an owner whose window slides with every
    /// preparation does, keeping the tier and material) is re-adjudicated by
    /// its owners (once) and stays pending on get and list, its custody
    /// holding the current decision with the window its owners gave it;
    /// nothing is announced. A later decision approves it and a fresh attempt
    /// spends it once.
    #[tokio::test]
    async fn an_unrelated_owner_change_keeps_a_pending_consent() {
        let w = world();
        w.owner.set_material(Some("team-checking"));
        w.owner.set_expiry(9_000_000_000_000);
        let id = w.request(&w.check()).await;
        let consented = w.held(&id);
        let prepares = w.owner.prepares.load(Ordering::SeqCst);
        w.owner.set_expiry(9_000_000_060_000);
        assert!(consented.check_retained_current().is_err());

        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Pending);
        assert_eq!(withdrawal_reason(&record), None);
        assert!(
            w.approvals
                .list(ApprovalListFilter::default())
                .expect("list")
                .iter()
                .any(|record| record.approval_id == id && record.status == ApprovalStatus::Pending)
        );
        assert_eq!(w.notices(&id), vec![ConsentNoticeKind::Requested]);
        let held = w.held(&id);
        assert!(
            !held.same_check(&consented) && held.check_retained_current().is_ok(),
            "the custody holds the re-adjudicated, current decision"
        );
        assert_eq!(
            held.validity().map(|validity| validity.expires_at_ms),
            Some(9_000_000_060_000),
            "with the window its owners gave it"
        );
        assert_eq!(
            w.owner.prepares.load(Ordering::SeqCst),
            prepares + 1,
            "re-prepared once; the list found it current"
        );

        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        assert_eq!(
            w.notices(&id),
            vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Approved]
        );
        assert_eq!(w.spend(&id), Ok(()));
        assert_eq!(w.notices(&id).last(), Some(&ConsentNoticeKind::Used));
    }

    /// ADR-001 D9: a related change while the consent is pending (the
    /// review tier, or the material the approval presented) still withdraws
    /// it on its next read as `ContextChanged`, and a later decision is
    /// refused. An entering decision of another operation withdraws it at the
    /// attempt's check point.
    #[tokio::test]
    async fn a_related_owner_change_withdraws_a_pending_consent() {
        for change in ["tier", "material"] {
            let w = world();
            w.owner.set_material(Some("team-checking"));
            let id = w.request(&w.check()).await;
            if change == "tier" {
                w.owner.r2.store(true, Ordering::Release);
                w.owner.republish();
            } else {
                w.owner.set_material(Some("team-savings"));
            }
            let record = w.record(&id);
            assert_eq!(record.status, ApprovalStatus::Cancelled, "{change}");
            assert_eq!(
                withdrawal_reason(&record),
                Some(ApprovalConsentWithdrawalReason::ContextChanged),
                "{change}"
            );
            assert_eq!(
                w.notices(&id),
                vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Withdrawn],
                "{change}"
            );
            assert_eq!(
                w.approvals
                    .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
                Err(ApprovalError::ConsentUnavailable {
                    approval_id: id.clone()
                }),
                "{change}"
            );
        }

        // Another operation's decision at the attempt's check point.
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let (entry, _admitted) = w.attempt(&id).expect("resolved");
        let guard = ReviewDispatchGuard::new(entry.clone());
        assert_eq!(
            entry.check(&w.check()),
            Err(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::ContextChanged,
                approval_id: None,
            })
        );
        guard.complete();
        assert_eq!(
            withdrawal_reason(&w.record(&id)),
            Some(ApprovalConsentWithdrawalReason::ContextChanged)
        );
        assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));
    }

    /// ADR-001 D9: what a re-adjudication's owners answer instead of a
    /// decision. Owners that are unavailable decide nothing: the read keeps
    /// the pending consent. Owners that now refuse the operation (a
    /// permission an owner change revoked) withdraw it on the next read as
    /// `AuthorityChanged`, the reason a refusing current check gives, and a
    /// later decision is refused.
    #[tokio::test]
    async fn a_refusal_found_by_a_re_adjudication_withdraws_as_authority_changed() {
        let w = world();
        let id = w.request(&w.check()).await;
        w.owner.prepare_unavailable.store(true, Ordering::Release);
        w.owner.republish();
        let record = w.record(&id);
        assert_eq!(
            record.status,
            ApprovalStatus::Pending,
            "unavailable owners decide nothing"
        );
        assert_eq!(withdrawal_reason(&record), None);
        assert_eq!(w.notices(&id), vec![ConsentNoticeKind::Requested]);

        w.owner.prepare_unavailable.store(false, Ordering::Release);
        w.owner.prepare_refused.store(true, Ordering::Release);
        w.owner.republish();
        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Cancelled);
        assert_eq!(
            withdrawal_reason(&record),
            Some(ApprovalConsentWithdrawalReason::AuthorityChanged)
        );
        assert_eq!(
            w.notices(&id),
            vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Withdrawn]
        );
        assert_eq!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::ConsentUnavailable {
                approval_id: id.clone()
            })
        );
    }

    /// ADR-001 D9: owners that refuse to present the review material when
    /// asked afresh refuse the operation: the consent is withdrawn as
    /// `AuthorityChanged`, as for a refused preparation (and as at a move).
    #[tokio::test]
    async fn a_denied_material_read_withdraws_as_authority_changed() {
        let w = world();
        let id = w.request(&w.check()).await;
        w.owner.material_refused.store(true, Ordering::Release);
        w.owner.republish();
        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Cancelled);
        assert_eq!(
            withdrawal_reason(&record),
            Some(ApprovalConsentWithdrawalReason::AuthorityChanged)
        );
        assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));
    }

    /// ADR-001 D9: a held decision whose owner's validity window for it ended
    /// while the human took their time is decided afresh, never withdrawn
    /// for that alone, and nothing of it is carried forward. Pending, a read
    /// keeps the consent as it is (reads decide afresh only what the owner
    /// publication made stale), and the approval decides it afresh and holds
    /// the fresh decision; approved, the attempt is handed one and enters
    /// once. Owners whose own deadline passed refuse the fresh preparation
    /// and the consent is withdrawn as `AuthorityChanged`, with zero entry;
    /// a related change found then withdraws it as `ContextChanged`.
    #[tokio::test]
    async fn a_decision_whose_window_ended_is_decided_afresh() {
        // Pending: the window ends; a read keeps it as it is, and the
        // approval decides afresh.
        let w = world();
        let id = w.request(&w.check()).await;
        let consented = w.held(&id);
        w.owner.end_windows();
        assert!(
            consented.check_retained_current().is_err(),
            "its own check refuses it"
        );
        let prepares = w.owner.prepares.load(Ordering::SeqCst);
        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Pending);
        assert_eq!(withdrawal_reason(&record), None);
        assert!(w.held(&id).same_check(&consented), "a read decides nothing");
        assert_eq!(w.owner.prepares.load(Ordering::SeqCst), prepares);
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved after the window ended");
        let held = w.held(&id);
        assert!(!held.same_check(&consented) && held.check_retained_current().is_ok());
        assert_eq!(w.owner.prepares.load(Ordering::SeqCst), prepares + 1);

        // Approved: the window ends again before the attempt, which is
        // handed a decision decided afresh and enters once.
        w.owner.end_windows();
        assert_eq!(w.spend(&id), Ok(()));
        let record = w.record(&id);
        assert!(
            record
                .consent
                .as_ref()
                .is_some_and(|consent| consent.consumed_at.is_some())
        );
        assert_eq!(withdrawal_reason(&record), None);

        // The owners' own deadline passed: the approval's fresh preparation
        // refuses.
        let w = world();
        let id = w.request(&w.check()).await;
        w.owner.end_windows();
        w.owner.prepare_refused.store(true, Ordering::Release);
        assert_eq!(w.record(&id).status, ApprovalStatus::Pending);
        assert_eq!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::ConsentUnavailable {
                approval_id: id.clone()
            })
        );
        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Cancelled);
        assert_eq!(
            withdrawal_reason(&record),
            Some(ApprovalConsentWithdrawalReason::AuthorityChanged)
        );
        assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));

        // A related change found once the window ended.
        let w = world();
        let id = w.request(&w.check()).await;
        w.owner.end_windows();
        w.owner.r2.store(true, Ordering::Release);
        assert_eq!(w.record(&id).status, ApprovalStatus::Pending);
        assert_eq!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::ConsentUnavailable {
                approval_id: id.clone()
            })
        );
        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Cancelled);
        assert_eq!(
            withdrawal_reason(&record),
            Some(ApprovalConsentWithdrawalReason::ContextChanged)
        );
    }

    /// ADR-001 D9 for the approver's eligibility. An approved consent never
    /// outlives its approver's eligibility: revoked after the approval, the
    /// attempt enters nothing and the consent is withdrawn as
    /// `AuthorityChanged`, and so is a self-approval the host's policy
    /// permitted once that policy forbids it. An unrelated change of the
    /// eligibility facts (their evidence stamps the whole publication) keeps
    /// it: the spend asks the eligibility owner afresh and enters once.
    #[tokio::test]
    async fn an_approved_consent_never_outlives_its_approvers_eligibility() {
        // Revoked after the approval.
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        w.route
            .publish(|eligible| eligible.retain(|eligible| eligible != "lead-1"));
        assert_eq!(w.spend(&id), Err(human_consent_required()));
        let record = w.record(&id);
        assert!(
            record
                .consent
                .as_ref()
                .is_some_and(|consent| consent.consumed_at.is_none()),
            "nothing spent"
        );
        assert_eq!(
            withdrawal_reason(&record),
            Some(ApprovalConsentWithdrawalReason::AuthorityChanged)
        );
        assert!(!w.notices(&id).contains(&ConsentNoticeKind::Used));

        // A self-approval the host's policy permitted, then forbids.
        let w = world();
        w.route.permit_self_approval(true);
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "human:child"))
            .expect("self-approved under the permitting policy");
        w.route.permit_self_approval(false);
        assert_eq!(w.spend(&id), Err(human_consent_required()));
        assert_eq!(
            withdrawal_reason(&w.record(&id)),
            Some(ApprovalConsentWithdrawalReason::AuthorityChanged)
        );
        assert!(!w.notices(&id).contains(&ConsentNoticeKind::Used));

        // An unrelated change of the eligibility facts.
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        w.route
            .publish(|eligible| eligible.push("lead-2".to_string()));
        assert_eq!(w.spend(&id), Ok(()), "the eligibility owner answers afresh");
        assert_eq!(w.notices(&id).last(), Some(&ConsentNoticeKind::Used));
    }

    /// ADR-001 D9 at the decision. An unrelated change of the eligibility
    /// facts between the eligibility's resolution and the decision's
    /// reservation, or during the decision's own store write, never refuses
    /// or withdraws an eligible approval: the eligibility owner is asked
    /// afresh, and the decision records and publishes. A revocation during
    /// the write still withdraws the consent as `AuthorityChanged`.
    #[tokio::test]
    async fn an_unrelated_eligibility_change_during_a_decision_keeps_it() {
        // Between the resolution and the reservation.
        let w = world();
        let id = w.request(&w.check()).await;
        let route = Arc::clone(&w.route);
        *w.route.after_resolve.lock() = Some(Box::new(move || {
            route.publish(|eligible| eligible.push("lead-2".to_string()));
        }));
        let decided = w
            .approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("still eligible: decided");
        assert_eq!(decided.status, ApprovalStatus::Approved);
        assert_eq!(withdrawal_reason(&decided), None);
        assert_eq!(w.spend(&id), Ok(()));

        // During the decision's own store write: unrelated, then revoked.
        for revoked in [false, true] {
            let store = Arc::new(FaultStore::default());
            let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            let id = w.request(&w.check()).await;
            let route = Arc::clone(&w.route);
            *store.hook.lock() = Some(Box::new(move || {
                route.publish(|eligible| {
                    if revoked {
                        eligible.retain(|eligible| eligible != "lead-1");
                    } else {
                        eligible.push("lead-2".to_string());
                    }
                });
            }));
            let decided = w
                .approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
                .expect("decision recorded");
            assert_eq!(
                decided.status,
                ApprovalStatus::Approved,
                "revoked={revoked}"
            );
            if revoked {
                assert_eq!(
                    withdrawal_reason(&decided),
                    Some(ApprovalConsentWithdrawalReason::AuthorityChanged)
                );
                assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));
            } else {
                assert_eq!(withdrawal_reason(&decided), None);
                assert_eq!(
                    w.notices(&id),
                    vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Approved]
                );
                assert_eq!(w.spend(&id), Ok(()));
            }
        }
    }

    /// ADR-001 D9, bounded. An owner change that meets a read's
    /// re-adjudication (its fresh decision is stale again before the
    /// reservation) is re-adjudicated again, within
    /// `CONSENT_READJUDICATION_BOUND` rounds. A change that meets every round
    /// decides nothing after exactly that many preparations: the consent is
    /// never withdrawn for the bound alone, reads return it unchanged, and a
    /// decision records nothing and answers a retryable `StoreUnavailable`.
    /// An owner change still in progress decides nothing: the read keeps the
    /// consent after one round, and nothing spends the stale decision
    /// meanwhile.
    #[tokio::test]
    async fn re_adjudication_is_bounded_and_settles_nothing_while_a_change_is_in_progress() {
        let bound = usize::from(CONSENT_READJUDICATION_BOUND);

        // Every round but the last meets another change: kept.
        let w = world();
        let id = w.request(&w.check()).await;
        w.owner.republish();
        w.owner
            .republish_after_prepare
            .store(bound - 1, Ordering::Release);
        let prepares = w.owner.prepares.load(Ordering::SeqCst);
        assert_eq!(w.record(&id).status, ApprovalStatus::Pending);
        assert_eq!(w.owner.prepares.load(Ordering::SeqCst), prepares + bound);
        assert!(w.held(&id).check_retained_current().is_ok());
        assert_eq!(w.notices(&id), vec![ConsentNoticeKind::Requested]);

        // Every round meets another change: nothing decided, nothing
        // withdrawn, and a decision records nothing.
        let w = world();
        let id = w.request(&w.check()).await;
        w.owner.republish();
        w.owner
            .republish_after_prepare
            .store(usize::MAX, Ordering::Release);
        let prepares = w.owner.prepares.load(Ordering::SeqCst);
        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Pending);
        assert_eq!(withdrawal_reason(&record), None);
        assert_eq!(
            w.owner.prepares.load(Ordering::SeqCst),
            prepares + bound,
            "bounded"
        );
        assert_eq!(w.notices(&id), vec![ConsentNoticeKind::Requested]);
        assert!(
            matches!(
                w.approvals
                    .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
                Err(ApprovalError::StoreUnavailable(_))
            ),
            "a retryable answer"
        );
        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Pending, "nothing recorded");
        assert!(record.decision.is_none());
        // Once the publication settles, the decision records.
        w.owner.republish_after_prepare.store(0, Ordering::Release);
        let decided = w
            .approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved once settled");
        assert_eq!(decided.status, ApprovalStatus::Approved);

        // A change in progress: nothing decided, nothing spent, kept.
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let (entry, admitted) = w.attempt(&id).expect("resolved");
        w.owner.republish();
        w.owner.in_progress.store(true, Ordering::Release);
        let prepares = w.owner.prepares.load(Ordering::SeqCst);
        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Approved);
        assert_eq!(withdrawal_reason(&record), None);
        assert_eq!(
            w.owner.prepares.load(Ordering::SeqCst),
            prepares + 1,
            "one round, never retried while the change is in progress"
        );
        let guard = ReviewDispatchGuard::new(entry.clone());
        assert_eq!(
            entry.consume(&admitted),
            Err(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::OwnerUnavailable,
            }),
            "the stale decision never enters"
        );
        guard.complete();
        assert!(
            w.record(&id)
                .consent
                .is_some_and(|consent| consent.consumed_at.is_none())
        );
        w.owner.in_progress.store(false, Ordering::Release);
        assert_eq!(w.spend_staged(&id), Ok(()));
    }

    /// ADR-001 D9 at the decision's publication: an owner change during the
    /// decision's own store write makes the retained decision stale before
    /// the allowance is published. Unrelated, it is re-adjudicated and the
    /// allowance published, and a fresh attempt spends it once; related
    /// (the review tier), the consent is withdrawn and the decision stays
    /// history.
    #[tokio::test]
    async fn a_decision_re_adjudicates_a_change_during_its_write() {
        for related in [false, true] {
            let store = Arc::new(FaultStore::default());
            let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            let id = w.request(&w.check()).await;
            let owner = Arc::clone(&w.owner);
            *store.hook.lock() = Some(Box::new(move || {
                if related {
                    owner.r2.store(true, Ordering::Release);
                }
                owner.republish();
            }));
            let decided = w
                .approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
                .expect("decision recorded");
            assert_eq!(
                decided.status,
                ApprovalStatus::Approved,
                "related={related}"
            );
            if related {
                assert_eq!(
                    withdrawal_reason(&decided),
                    Some(ApprovalConsentWithdrawalReason::ContextChanged)
                );
                assert_eq!(
                    w.notices(&id),
                    vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Withdrawn]
                );
                assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));
            } else {
                assert_eq!(withdrawal_reason(&decided), None);
                assert_eq!(
                    w.notices(&id),
                    vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Approved]
                );
                assert_eq!(w.spend(&id), Ok(()));
            }
        }
    }

    /// ADR-001 D9: approval reads decide afresh only a decision the owner
    /// publication made stale. One whose own check refuses it (its owner's
    /// validity window ended) or answers that its owners are unavailable is
    /// never decided afresh by a read, so polling adds no owner call (and
    /// none of their audit rows). An approval then decides it afresh, or,
    /// while its owners are unavailable, records nothing without calling
    /// them; a decline is recorded either way.
    #[tokio::test]
    async fn polling_reads_never_call_the_owners_of_a_refused_or_unavailable_decision() {
        // Its owners are unavailable.
        let w = world();
        let id = w.request(&w.check()).await;
        w.owner.check_unavailable.store(true, Ordering::Release);
        let prepares = w.owner.prepares.load(Ordering::SeqCst);
        for _ in 0..4 {
            assert_eq!(w.record(&id).status, ApprovalStatus::Pending);
            assert!(
                w.approvals
                    .list(ApprovalListFilter::default())
                    .expect("list")
                    .iter()
                    .any(|record| record.approval_id == id)
            );
        }
        assert_eq!(
            w.owner.prepares.load(Ordering::SeqCst),
            prepares,
            "no owner call while polling"
        );
        assert!(matches!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::StoreUnavailable(_))
        ));
        assert_eq!(
            w.owner.prepares.load(Ordering::SeqCst),
            prepares,
            "nor for the approval, which records nothing"
        );
        assert_eq!(w.record(&id).status, ApprovalStatus::Pending);
        let declined = w
            .approvals
            .decide_consent(&id, decision(ApprovalDecision::Deny, "lead-1"))
            .expect("a decline is recorded");
        assert_eq!(declined.status, ApprovalStatus::Denied);
        assert_eq!(
            w.notices(&id),
            vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Denied]
        );

        // Its window ended.
        let w = world();
        let id = w.request(&w.check()).await;
        let consented = w.held(&id);
        w.owner.end_windows();
        let prepares = w.owner.prepares.load(Ordering::SeqCst);
        for _ in 0..4 {
            assert_eq!(w.record(&id).status, ApprovalStatus::Pending);
        }
        assert_eq!(
            w.owner.prepares.load(Ordering::SeqCst),
            prepares,
            "no owner call while polling"
        );
        assert!(w.held(&id).same_check(&consented));
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        assert_eq!(
            w.owner.prepares.load(Ordering::SeqCst),
            prepares + 1,
            "the approval decided it afresh once"
        );
        assert_eq!(w.spend(&id), Ok(()));
    }

    /// ADR-001 D9: a decline stands on no retained decision, so it is
    /// recorded whatever that decision's currentness: while its owners'
    /// change is in progress, or while changes keep meeting every
    /// re-adjudication. An approval records nothing then.
    #[tokio::test]
    async fn a_decline_is_recorded_while_the_owners_keep_changing() {
        for in_progress in [true, false] {
            let w = world();
            let id = w.request(&w.check()).await;
            w.owner.republish();
            if in_progress {
                w.owner.in_progress.store(true, Ordering::Release);
            } else {
                w.owner
                    .republish_after_prepare
                    .store(usize::MAX, Ordering::Release);
            }
            assert!(
                matches!(
                    w.approvals
                        .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
                    Err(ApprovalError::StoreUnavailable(_))
                ),
                "in_progress={in_progress}"
            );
            let declined = w
                .approvals
                .decide_consent(&id, decision(ApprovalDecision::Deny, "lead-1"))
                .expect("a decline is recorded");
            assert_eq!(
                declined.status,
                ApprovalStatus::Denied,
                "in_progress={in_progress}"
            );
            assert_eq!(
                w.notices(&id),
                vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Denied],
                "in_progress={in_progress}"
            );
            assert_eq!(
                w.attempt(&id).err(),
                Some(crate::ToolError::ReviewUnsatisfied {
                    kind: ReviewUnsatisfiedKind::Denied,
                    approval_id: None,
                }),
                "in_progress={in_progress}"
            );
        }
    }

    /// ADR-001 D9: an eligibility owner that cannot say now (an owner change
    /// in progress) decides nothing. An approval records nothing and answers
    /// a retryable `StoreUnavailable`. A spend whose stale evidence meets it
    /// enters nothing and keeps the consent (`owner_unavailable`), and once
    /// it settles the spend enters once; after the decision the owner is
    /// asked about the approval as decided. A decision's publication that
    /// meets it asks again within its bound and then publishes as it is,
    /// and its spend asks once more.
    #[tokio::test]
    async fn an_eligibility_owner_that_cannot_say_decides_nothing() {
        let w = world();
        let id = w.request(&w.check()).await;
        w.route.in_change.store(true, Ordering::Release);
        assert!(matches!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::StoreUnavailable(_))
        ));
        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Pending, "nothing recorded");
        assert!(record.decision.is_none());
        assert_eq!(w.notices(&id), vec![ConsentNoticeKind::Requested]);
        w.route.in_change.store(false, Ordering::Release);
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved once settled");

        // An owner change in progress during the spend's re-resolution.
        w.route
            .publish(|eligible| eligible.push("lead-2".to_string()));
        w.route.in_change.store(true, Ordering::Release);
        assert_eq!(w.spend(&id), Err(owner_unavailable()));
        let record = w.record(&id);
        assert_eq!(record.status, ApprovalStatus::Approved);
        assert_eq!(withdrawal_reason(&record), None, "the consent is kept");
        assert!(
            record
                .consent
                .as_ref()
                .is_some_and(|consent| consent.consumed_at.is_none()),
            "nothing spent"
        );
        assert!(!w.notices(&id).contains(&ConsentNoticeKind::Used));
        assert_eq!(
            w.route.asked.lock().last(),
            Some(&(ApprovalStatus::Approved, Some("lead-1".to_string()))),
            "asked about the approval as decided"
        );
        w.route.in_change.store(false, Ordering::Release);
        assert_eq!(w.spend(&id), Ok(()));
        assert_eq!(w.notices(&id).last(), Some(&ConsentNoticeKind::Used));

        // During the decision's own store write: an eligibility change lands
        // and the owner stays in change.
        let store = Arc::new(FaultStore::default());
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let id = w.request(&w.check()).await;
        let route = Arc::clone(&w.route);
        *store.hook.lock() = Some(Box::new(move || {
            route.publish(|eligible| eligible.push("lead-2".to_string()));
            route.in_change.store(true, Ordering::Release);
        }));
        let decided = w
            .approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("decision recorded");
        assert_eq!(decided.status, ApprovalStatus::Approved);
        assert_eq!(withdrawal_reason(&decided), None);
        assert_eq!(
            w.notices(&id),
            vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Approved],
            "published as it is"
        );
        assert_eq!(w.spend(&id), Err(owner_unavailable()));
        assert_eq!(withdrawal_reason(&w.record(&id)), None);
        w.route.in_change.store(false, Ordering::Release);
        assert_eq!(w.spend(&id), Ok(()));
    }

    #[tokio::test]
    async fn concurrent_fresh_attempts_spend_once() {
        let w = world();
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let (first, first_admitted) = w.attempt(&id).expect("first attempt");
        let (second, second_admitted) = w.attempt(&id).expect("second attempt");
        let guards = (
            ReviewDispatchGuard::new(first.clone()),
            ReviewDispatchGuard::new(second.clone()),
        );
        assert_eq!(first.consume(&first_admitted), Ok(()));
        assert_eq!(
            second.consume(&second_admitted),
            Err(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                approval_id: None,
            })
        );
        guards.0.complete();
        guards.1.complete();
    }

    /// Blocks the armed store write (inside a human `decide`, under the
    /// owner's state lock) until released, for at most `BOUND`.
    struct GatedStore {
        inner: InMemoryApprovalStore,
        armed: AtomicBool,
        /// Armed writes to let through before the gated one.
        skip: AtomicUsize,
        /// Runs once inside the next write.
        hook: parking_lot::Mutex<Option<Box<dyn FnOnce() + Send>>>,
        held: parking_lot::Mutex<mpsc::Sender<()>>,
        release: parking_lot::Mutex<mpsc::Receiver<()>>,
    }

    impl ApprovalStore for GatedStore {
        fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
            self.inner.load_all()
        }

        fn get(
            &self,
            approval_id: &ApprovalId,
        ) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
            self.inner.get(approval_id)
        }

        fn put_if_current(
            &self,
            expected: Option<&ApprovalRecord>,
            record: &ApprovalRecord,
        ) -> Result<(), ApprovalStoreError> {
            let hook = self.hook.lock().take();
            if let Some(hook) = hook {
                hook();
            }
            if self.armed.load(Ordering::Acquire) {
                if self.skip.load(Ordering::Acquire) > 0 {
                    self.skip.fetch_sub(1, Ordering::AcqRel);
                } else {
                    self.armed.store(false, Ordering::Release);
                    let _ = self.held.lock().send(());
                    // A gate never released is a test failure, never a
                    // silent commit.
                    self.release
                        .lock()
                        .recv_timeout(BOUND)
                        .expect("the gated write is released within its bound");
                }
            }
            self.inner.put_if_current(expected, record)
        }

        fn is_persistent(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn a_contended_spend_keeps_the_consent_spendable() {
        let (held_tx, held) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let store = Arc::new(GatedStore {
            inner: InMemoryApprovalStore::new(),
            armed: AtomicBool::new(false),
            skip: AtomicUsize::new(0),
            hook: parking_lot::Mutex::default(),
            held: parking_lot::Mutex::new(held_tx),
            release: parking_lot::Mutex::new(release_rx),
        });
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let (entry, admitted) = w.attempt(&id).expect("resolved");

        // An unrelated human decision holds the owner through its store write.
        let unrelated = w
            .approvals
            .request(ApprovalRequest {
                requester: ApprovalPrincipalId::new("human:child").expect("principal"),
                owner: ApprovalOwnerRef::Runtime,
                resource: ApprovalResourceRef {
                    kind: ApprovalResourceKind::Runtime,
                    id: ApprovalResourceId::new("local"),
                },
                proposed_action: ApprovalProposedAction {
                    kind: ApprovalActionKind::Other,
                    summary: "manual gate".to_string(),
                    body: None,
                },
                risk: ApprovalRisk::Low,
                request_body: None,
                allowed_decisions: [ApprovalDecision::Approve].into_iter().collect(),
                expires_at: None,
                metadata: SurfaceMetadata::default(),
                request_provenance: None,
            })
            .expect("unrelated approval")
            .approval_id;
        store.armed.store(true, Ordering::Release);
        let approvals = w.approvals.clone();
        let holder = std::thread::spawn(move || {
            approvals
                .decide(
                    &unrelated,
                    ApprovalDecision::Approve,
                    ApprovalPrincipalId::new("human:operator").expect("principal"),
                    None,
                    None,
                )
                .expect("decided");
        });
        held.recv_timeout(BOUND)
            .expect("decide holds the reservation");

        let guard = ReviewDispatchGuard::new(entry.clone());
        assert_eq!(
            entry.consume(&admitted),
            Err(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::OwnerUnavailable,
            }),
            "a held owner refuses at once; the spend never waits"
        );
        guard.complete();
        let _ = release.send(());
        holder.join().expect("holder thread");

        assert_eq!(
            w.spend(&id),
            Ok(()),
            "nothing was spent: it still enters once"
        );
    }

    /// Fails the next armed conditional write, either before anything is
    /// written or after the write committed.
    #[derive(Default)]
    struct FaultStore {
        inner: InMemoryApprovalStore,
        fault: parking_lot::Mutex<Option<Fault>>,
        /// Writes to let through before the fault applies.
        skip: AtomicUsize,
        /// Runs once inside the next write.
        hook: parking_lot::Mutex<Option<Box<dyn FnOnce() + Send>>>,
        /// Row reads fail while set.
        unreadable: AtomicBool,
    }

    #[derive(Clone, Copy, Debug)]
    enum Fault {
        BeforeWrite,
        AfterWrite,
    }

    impl ApprovalStore for FaultStore {
        fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
            self.inner.load_all()
        }

        fn get(
            &self,
            approval_id: &ApprovalId,
        ) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
            if self.unreadable.load(Ordering::Acquire) {
                return Err(ApprovalStoreError::NotCommitted(
                    "injected unreadable row".to_string(),
                ));
            }
            self.inner.get(approval_id)
        }

        fn put_if_current(
            &self,
            expected: Option<&ApprovalRecord>,
            record: &ApprovalRecord,
        ) -> Result<(), ApprovalStoreError> {
            let hook = self.hook.lock().take();
            if let Some(hook) = hook {
                hook();
            }
            if self.skip.load(Ordering::Acquire) > 0 {
                self.skip.fetch_sub(1, Ordering::AcqRel);
                return self.inner.put_if_current(expected, record);
            }
            match self.fault.lock().take() {
                None => self.inner.put_if_current(expected, record),
                Some(Fault::BeforeWrite) => Err(ApprovalStoreError::NotCommitted(
                    "injected failure before the write".to_string(),
                )),
                Some(Fault::AfterWrite) => {
                    self.inner.put_if_current(expected, record)?;
                    Err(ApprovalStoreError::CommitUnknown(
                        "injected failure after the write".to_string(),
                    ))
                }
            }
        }

        fn is_persistent(&self) -> bool {
            true
        }
    }

    /// The exact cause of `id`'s withdrawal while its record is still owed.
    fn owed_withdrawal_cause(
        w: &World,
        id: &ApprovalId,
    ) -> Option<ApprovalConsentWithdrawalReason> {
        w.approvals
            .state
            .read()
            .unreconciled_withdrawals
            .get(id)
            .map(|pending| pending.cause.clone())
    }

    async fn approved_with(store: &Arc<FaultStore>) -> (World, ApprovalId) {
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        (w, id)
    }

    /// A spend whose store write provably committed nothing changes nothing:
    /// the consent stays spendable and an explicit later attempt spends once.
    #[tokio::test]
    async fn a_spend_that_wrote_nothing_keeps_the_consent_spendable() {
        let store = Arc::new(FaultStore::default());
        let (w, id) = approved_with(&store).await;
        *store.fault.lock() = Some(Fault::BeforeWrite);
        assert_eq!(
            w.spend(&id),
            Err(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::OwnerUnavailable,
            })
        );
        assert!(w.record(&id).consent.and_then(|c| c.consumed_at).is_none());
        assert!(!w.notices(&id).contains(&ConsentNoticeKind::Used));
        assert_eq!(w.spend(&id), Ok(()), "a later explicit attempt spends");
    }

    /// A spend whose store write may have committed is typed unknown: zero
    /// entry, the custody is retired (no refund, no resend), and the row is
    /// reconciled from the store before any reuse.
    #[tokio::test]
    async fn a_spend_whose_commit_is_unknown_is_reconciled_never_respent() {
        let store = Arc::new(FaultStore::default());
        let (w, id) = approved_with(&store).await;
        *store.fault.lock() = Some(Fault::AfterWrite);
        assert_eq!(
            w.spend(&id),
            Err(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::StoreCommitUnknown,
            })
        );
        assert!(!w.notices(&id).contains(&ConsentNoticeKind::Used));
        assert_eq!(
            w.attempt(&id).err(),
            Some(human_consent_required()),
            "no custody survives an uncertain spend"
        );
        // Reading reconciles with the store, which did commit the spend.
        assert!(w.record(&id).consent.and_then(|c| c.consumed_at).is_some());
        assert_eq!(
            w.attempt(&id).err(),
            Some(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                approval_id: None,
            })
        );
    }

    /// Another owner of the same store spent the consent first: this owner's
    /// spend is a typed conflict, never spendable unavailability, and the
    /// reconciled row reports the spend.
    #[tokio::test]
    async fn a_row_spent_by_another_owner_conflicts_and_reconciles() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let id = w.request(&w.check()).await;
        let approved = w
            .approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");

        // Another owner of the same store already committed the spend.
        let mut elsewhere = approved.clone();
        elsewhere.consent = Some(ApprovalConsentRecord {
            consumed_at: Some(Utc::now()),
            ..approved.consent.clone().unwrap_or_default()
        });
        store
            .put_if_current(Some(&approved), &elsewhere)
            .expect("other owner's commit");

        assert_eq!(
            w.spend(&id),
            Err(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::StoreConflict,
            }),
            "the conditional commit refuses: zero entry"
        );
        assert_eq!(
            store.load_all().expect("rows"),
            vec![elsewhere.clone()],
            "the other owner's commit survives"
        );
        assert_eq!(w.record(&id), elsewhere, "reconciled from the store");
        assert_eq!(
            w.attempt(&id).err(),
            Some(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                approval_id: None,
            })
        );
    }

    /// Opening the same store in another service reads consent history only.
    /// An empty consent map proves that this service holds no candidate, not
    /// that the issuing owner lost custody, so nothing is withdrawn; an
    /// attempt through the reader reports the custody as unavailable. The
    /// live issuing owner still spends its approved consent once.
    #[tokio::test]
    async fn a_second_owner_of_the_store_reads_consent_history_only() {
        let store = Arc::new(CountingStore::default());
        let live = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let pending = live.request(&live.check()).await;
        let approved = live.request(&live.check()).await;
        live.approvals
            .decide_consent(&approved, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved by the live owner");
        let writes = store.writes.load(Ordering::SeqCst);

        let reader = world_with(ApprovalService::with_store(store.clone()).expect("reader"));
        let pending_row = reader.record(&pending);
        assert_eq!(pending_row.status, ApprovalStatus::Pending);
        assert!(
            pending_row
                .consent
                .and_then(|consent| consent.withdrawal)
                .is_none()
        );
        let approved_row = reader.record(&approved);
        assert_eq!(approved_row.status, ApprovalStatus::Approved);
        assert!(
            approved_row
                .consent
                .and_then(|consent| consent.withdrawal)
                .is_none()
        );
        assert_eq!(
            reader.attempt(&approved).err(),
            Some(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::OwnerUnavailable,
            }),
            "a reader without the custody reports it unavailable, not missing consent"
        );
        assert_eq!(
            store.writes.load(Ordering::SeqCst),
            writes,
            "the reader writes nothing: no withdrawal on open, none for its attempt"
        );
        assert_eq!(
            live.spend(&approved),
            Ok(()),
            "the live issuing owner still spends"
        );
    }

    /// A decision through a reader of the store, which holds no candidate,
    /// is refused before any commit and changes nothing; the live issuing
    /// owner still records its decision.
    #[tokio::test]
    async fn a_decision_through_a_store_reader_commits_nothing() {
        let store = Arc::new(CountingStore::default());
        let live = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let id = live.request(&live.check()).await;
        let reader = world_with(ApprovalService::with_store(store.clone()).expect("reader"));
        let writes = store.writes.load(Ordering::SeqCst);
        assert_eq!(
            reader
                .approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::ConsentUnavailable {
                approval_id: id.clone()
            })
        );
        assert_eq!(
            store.writes.load(Ordering::SeqCst),
            writes,
            "the refused decision writes nothing"
        );
        assert_eq!(live.record(&id).status, ApprovalStatus::Pending);
        assert_eq!(live.notices(&id), vec![ConsentNoticeKind::Requested]);
        live.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("the live owner decides");
        assert_eq!(live.spend(&id), Ok(()));
    }

    /// A second denial finds no candidate (the first removed it): the owner
    /// reports consent unavailable, commits nothing and notifies nothing.
    #[tokio::test]
    async fn a_second_denial_reports_the_owner_outcome_and_adds_nothing() {
        let store = Arc::new(CountingStore::default());
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let id = w.request(&w.check()).await;
        let denied = w
            .approvals
            .decide_consent(&id, decision(ApprovalDecision::Deny, "lead-1"))
            .expect("denied");
        let writes = store.writes.load(Ordering::SeqCst);
        assert_eq!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Deny, "lead-1")),
            Err(ApprovalError::ConsentUnavailable {
                approval_id: id.clone()
            })
        );
        assert_eq!(w.record(&id), denied, "the first outcome stands");
        assert_eq!(
            store.writes.load(Ordering::SeqCst),
            writes,
            "no second commit"
        );
        assert_eq!(
            w.notices(&id),
            vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Denied]
        );
    }

    /// Restore validates consent history and keeps its one-use fact in the
    /// generated owner: a consumed history needs an approved decision by a
    /// qualified human approver who is the decision's actor. A valid spent
    /// history reopens as spent, with no custody.
    #[tokio::test]
    async fn restore_rejects_contradictory_consent_history() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        assert_eq!(w.spend(&id), Ok(()));
        let spent = store.get(&id).expect("read").expect("spent row");

        // A valid spent history reopens as spent history only.
        let reopened = world_with(ApprovalService::with_store(store).expect("reopened"));
        assert!(
            reopened
                .record(&id)
                .consent
                .and_then(|c| c.consumed_at)
                .is_some()
        );
        assert_eq!(
            admit_consent_attempt(Some(&reopened.review), &id, &reopened.work, None).err(),
            Some(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                approval_id: None,
            })
        );

        let approver = |principal: PrincipalRef| {
            let mut row = spent.clone();
            if let Some(consent) = row.consent.as_mut() {
                consent.approver = Some(ApprovalConsentApprover {
                    principal,
                    assurance: ApprovalAssurance::PersonAuthenticated,
                });
            }
            row
        };
        let team = || TrustDomainId::new("team").expect("domain");
        let mut absent = spent.clone();
        if let Some(consent) = absent.consent.as_mut() {
            consent.approver = None;
        }
        let nonhuman = approver(
            PrincipalRef::in_domain(PrincipalKind::ServiceAccount, "lead-1", team())
                .expect("principal"),
        );
        let unqualified =
            approver(PrincipalRef::new(PrincipalKind::Human, "lead-1").expect("principal"));
        let mismatched = approver(
            PrincipalRef::in_domain(PrincipalKind::Human, "lead-2", team()).expect("principal"),
        );
        let mut consumed_pending = spent.clone();
        consumed_pending.status = ApprovalStatus::Pending;
        consumed_pending.decision = None;
        if let Some(consent) = consumed_pending.consent.as_mut() {
            consent.approver = None;
        }
        let mut consumed_denied = spent.clone();
        consumed_denied.status = ApprovalStatus::Denied;
        if let Some(decision) = consumed_denied.decision.as_mut() {
            decision.decision = ApprovalDecision::Deny;
        }
        for (case, row) in [
            ("absent approver", absent),
            ("nonhuman approver", nonhuman),
            ("unqualified approver", unqualified),
            ("approver is not the actor", mismatched),
            ("consumed pending", consumed_pending),
            ("consumed denied", consumed_denied),
        ] {
            let store = Arc::new(InMemoryApprovalStore::new());
            store.put_if_current(None, &row).expect("seed");
            assert!(
                matches!(
                    ApprovalService::with_store(store),
                    Err(ApprovalError::InvalidStoredRecord { .. })
                ),
                "{case} must not restore"
            );
        }
    }

    /// A gated store whose next armed conditional write blocks until
    /// released, counting every write.
    fn gated_world() -> (World, Arc<GatedStore>, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (held_tx, held) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let store = Arc::new(GatedStore {
            inner: InMemoryApprovalStore::new(),
            armed: AtomicBool::new(false),
            skip: AtomicUsize::new(0),
            hook: parking_lot::Mutex::default(),
            held: parking_lot::Mutex::new(held_tx),
            release: parking_lot::Mutex::new(release_rx),
        });
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        (w, store, held, release)
    }

    /// The final check of a spend comes after its store write and the owner
    /// exit: an authority change or an expiry during the write refuses the
    /// entry (zero effect), the committed spend stays spent (no refund, no
    /// second spend), and the unchanged control enters.
    #[tokio::test]
    async fn a_spend_refused_after_its_write_enters_nothing_and_stays_spent() {
        for change in [None, Some("authority"), Some("deadline")] {
            let (w, store, held, release) = gated_world();
            let id = w.request(&w.check()).await;
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
                .expect("approved");
            let (entry, admitted) = w.attempt(&id).expect("resolved");
            store.armed.store(true, Ordering::Release);
            let spender = {
                let entry = entry.clone();
                std::thread::spawn(move || {
                    let guard = ReviewDispatchGuard::new(entry.clone());
                    let result = entry.consume(&admitted);
                    guard.complete();
                    result
                })
            };
            held.recv_timeout(BOUND).expect("the spend holds its write");
            match change {
                Some("authority") => w.owner.denied.store(true, Ordering::Release),
                Some(_) => w.approvals.clock_skew.store(
                    i64::try_from(VALIDITY.as_secs()).expect("seconds") + 1,
                    Ordering::SeqCst,
                ),
                None => {}
            }
            let _ = release.send(());
            let result = spender.join().expect("spender");
            match change {
                None => assert_eq!(result, Ok(()), "unchanged control enters"),
                Some("authority") => assert_eq!(
                    result,
                    Err(crate::ToolError::AuthorizationRefused {
                        refusal: OperationRefused::new(OperationRefusalKind::Denied),
                    })
                ),
                Some(_) => assert_eq!(result, Err(human_consent_required())),
            }
            w.owner.denied.store(false, Ordering::Release);
            w.approvals.clock_skew.store(0, Ordering::SeqCst);
            // Committed history either way: spent once, never again.
            assert!(
                store
                    .get(&id)
                    .expect("read")
                    .and_then(|row| row.consent)
                    .and_then(|consent| consent.consumed_at)
                    .is_some()
            );
            assert_eq!(
                w.attempt(&id).err(),
                Some(crate::ToolError::ReviewUnsatisfied {
                    kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                    approval_id: None,
                })
            );
        }
    }

    /// Eligibility revoked while the decision's store write is in flight:
    /// the decision is committed history, but no usable allowance is
    /// published (no Approved notice, no spend). The unchanged control
    /// approves and spends.
    #[tokio::test]
    async fn an_eligibility_revoked_during_the_decision_write_publishes_no_allowance() {
        for revoke in [true, false] {
            let (w, store, held, release) = gated_world();
            let id = w.request(&w.check()).await;
            store.armed.store(true, Ordering::Release);
            let decider = {
                let approvals = w.approvals.clone();
                let id = id.clone();
                std::thread::spawn(move || {
                    approvals.decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
                })
            };
            held.recv_timeout(BOUND)
                .expect("the decision holds its write");
            if revoke {
                w.route
                    .publish(|eligible| eligible.retain(|id| id != "lead-1"));
            }
            let _ = release.send(());
            let decided = decider.join().expect("decider").expect("decision recorded");
            assert_eq!(
                decided.status,
                ApprovalStatus::Approved,
                "committed history"
            );
            if revoke {
                assert_eq!(
                    decided
                        .consent
                        .and_then(|consent| consent.withdrawal)
                        .map(|withdrawal| withdrawal.reason),
                    Some(ApprovalConsentWithdrawalReason::AuthorityChanged)
                );
                assert_eq!(
                    w.notices(&id),
                    vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Withdrawn]
                );
                assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));
            } else {
                assert_eq!(
                    w.notices(&id),
                    vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Approved]
                );
                assert_eq!(w.spend(&id), Ok(()));
            }
        }
    }

    /// An R2 spend makes no store call on its owner exit, even with a queued
    /// run withdrawal: the withdrawal's custody is retired there in memory,
    /// and its durable record and notice follow on the next store-capable
    /// owner call. The R2 final check after the owner exit refuses a
    /// permission withdrawn by then: zero effect, and the attempt stays spent.
    #[tokio::test]
    async fn an_r2_spend_stays_store_free_and_rechecks_after_the_owner_exit() {
        let store = Arc::new(CountingStore::default());
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let other = world_with(w.approvals.clone());
        let pending = other.request(&other.check()).await;
        w.owner.r2.store(true, Ordering::Release);
        *w.reviewer.1.lock() = Some(Scripted::Verdict(ReviewVerdict::Allow));

        // Unchanged control, with the other run's withdrawal queued.
        let check = w.check();
        let entry = w
            .admit(&check, ReviewEntrySupport::ConsumesAtEntry)
            .await
            .expect("allowed and retained");
        w.approvals
            .queued_withdrawals
            .lock()
            .push(other.work.clone());
        let writes = store.writes.load(Ordering::SeqCst);
        let guard = ReviewDispatchGuard::new(entry.clone());
        assert_eq!(entry.consume(&check), Ok(()));
        assert_eq!(
            store.writes.load(Ordering::SeqCst),
            writes,
            "no store call from the spend to its effect"
        );
        guard.complete();
        assert!(
            !w.approvals.state.read().consent.contains_key(&pending),
            "the queued withdrawal's custody is retired on the R2 owner exit"
        );
        // Settled by the owner's own handoff once the spend's reports ran
        // after the effect: no get, list, decide or request nudge.
        let stored = store.get(&pending).expect("read").expect("row");
        assert_eq!(stored.status, ApprovalStatus::Cancelled);
        assert_eq!(
            stored
                .consent
                .and_then(|consent| consent.withdrawal)
                .map(|withdrawal| withdrawal.reason),
            Some(ApprovalConsentWithdrawalReason::RunEnded)
        );
        assert_eq!(
            other.notices(&pending),
            vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Withdrawn]
        );
        assert!(w.approvals.state.read().pending_withdrawals.is_empty());

        // The permission is withdrawn after the reservation's final check.
        let check = w.check();
        let entry = w
            .admit(&check, ReviewEntrySupport::ConsumesAtEntry)
            .await
            .expect("allowed and retained");
        let checks = w.owner.checks.load(Ordering::SeqCst);
        w.owner.deny_from.store(checks + 2, Ordering::SeqCst);
        let guard = ReviewDispatchGuard::new(entry.clone());
        assert_eq!(
            entry.consume(&check),
            Err(crate::ToolError::AuthorizationRefused {
                refusal: OperationRefused::new(OperationRefusalKind::Denied),
            })
        );
        guard.complete();
        assert_eq!(
            w.statuses().last(),
            Some(&(ReviewAttemptStatus::Used, None)),
            "the spend stays spent"
        );
        assert_eq!(
            entry.consume(&check),
            Err(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                approval_id: None,
            })
        );
    }

    /// A second run closing while the first run's withdrawal is being
    /// recorded is drained by the holder before it unlocks: both custodies
    /// are retired and both withdrawals recorded and announced once, with
    /// no later owner call.
    #[tokio::test]
    async fn a_run_closing_during_a_withdrawal_write_is_drained_before_unlock() {
        let (w, store, held, release) = gated_world();
        let other = world_with(w.approvals.clone());
        let first = w.request(&w.check()).await;
        let second = other.request(&other.check()).await;
        store.armed.store(true, Ordering::Release);
        let closer = {
            let review = Arc::clone(&w.review);
            let work = w.work.clone();
            std::thread::spawn(move || review.close_consent_for_work(&work))
        };
        held.recv_timeout(BOUND)
            .expect("the first withdrawal holds its write");
        // The second close completes while the first write is still held:
        // it never waits on the owner or a queue mutex.
        let (closed, second_closed) = mpsc::channel();
        let second_closer = {
            let review = Arc::clone(&other.review);
            let work = other.work.clone();
            std::thread::spawn(move || {
                review.close_consent_for_work(&work);
                let _ = closed.send(());
            })
        };
        second_closed
            .recv_timeout(BOUND)
            .expect("the second close returns while the first write is held");
        second_closer.join().expect("second closer");
        let _ = release.send(());
        closer.join().expect("closer");

        // Owner state and the store only; no get, list or decide.
        {
            let state = w.approvals.state.read();
            assert!(state.consent.is_empty(), "both custodies retired");
            for id in [&first, &second] {
                let record = state.records.get(id).expect("record");
                assert_eq!(record.status, ApprovalStatus::Cancelled);
                assert!(!state.unreconciled.contains_key(id));
            }
        }
        for (world, id) in [(&w, &first), (&other, &second)] {
            let stored = store.get(id).expect("read").expect("row");
            assert_eq!(stored.status, ApprovalStatus::Cancelled);
            assert_eq!(
                stored
                    .consent
                    .and_then(|consent| consent.withdrawal)
                    .map(|withdrawal| withdrawal.reason),
                Some(ApprovalConsentWithdrawalReason::RunEnded)
            );
            assert_eq!(
                world.notices(id),
                vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Withdrawn],
                "announced once"
            );
        }
    }

    /// A stale withdrawal whose store write wrote nothing stops the read: no
    /// status is derived from the old record, the custody is retired at
    /// once, and the withdrawal stays owed with its exact cause, announced
    /// to no one. Reconciliation settles it against the stored pending row:
    /// the generated owner's unlinked consent settlement cancels the
    /// approval in the same write that records the withdrawal, announced
    /// once. Every later read, past the deadline too, finds the durable row
    /// and the generated owner both Cancelled. The change is a related one
    /// (the owners now require R2): an unrelated one would keep the consent.
    #[tokio::test]
    async fn a_failed_stale_withdrawal_stops_the_read_and_reconciles() {
        let store = Arc::new(FaultStore::default());
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let id = w.request(&w.check()).await;
        w.owner.r2.store(true, Ordering::Release);
        w.owner.republish();
        *store.fault.lock() = Some(Fault::BeforeWrite);
        assert_eq!(
            w.approvals.get(&id),
            Err(ApprovalError::Unreconciled {
                approval_id: id.clone()
            })
        );
        let stored = store.get(&id).expect("read").expect("row");
        assert_eq!(stored.status, ApprovalStatus::Pending, "nothing written");
        assert!(
            stored
                .consent
                .and_then(|consent| consent.withdrawal)
                .is_none()
        );
        assert!(
            !w.approvals.state.read().consent.contains_key(&id),
            "custody retired at once"
        );
        assert_eq!(
            owed_withdrawal_cause(&w, &id),
            Some(ApprovalConsentWithdrawalReason::ContextChanged),
            "the withdrawal stays owed with its exact cause"
        );
        assert_eq!(w.notices(&id), vec![ConsentNoticeKind::Requested]);

        // Reconciled from the store, then settled against the stored row.
        let row = w.record(&id);
        assert_eq!(row.status, ApprovalStatus::Cancelled);
        assert_eq!(
            withdrawal_reason(&row),
            Some(ApprovalConsentWithdrawalReason::ContextChanged)
        );
        assert_eq!(store.get(&id).expect("read"), Some(row.clone()));
        assert_eq!(owed_withdrawal_cause(&w, &id), None, "settled");
        assert_eq!(
            w.notices(&id),
            vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Withdrawn],
            "announced once, after its record"
        );
        // Refreshed again, now and past the deadline: the durable row and
        // the generated owner both stay Cancelled.
        for skew in [0, i64::try_from(VALIDITY.as_secs()).expect("seconds") + 1] {
            w.approvals.clock_skew.store(skew, Ordering::SeqCst);
            assert_eq!(w.record(&id), row, "skew={skew}");
            assert_eq!(
                w.approvals
                    .state
                    .read()
                    .authority
                    .status_for(&id.to_string()),
                Some(ApprovalLifecycleStatus::Cancelled),
                "skew={skew}"
            );
        }
        w.approvals.clock_skew.store(0, Ordering::SeqCst);
        assert_eq!(store.get(&id).expect("read"), Some(row));
        assert_eq!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::ConsentUnavailable {
                approval_id: id.clone()
            })
        );
        assert!(
            ApprovalService::with_store(store).is_ok(),
            "valid on reopen"
        );
    }

    /// Counts every conditional write of an in-memory store.
    #[derive(Default)]
    struct CountingStore {
        inner: InMemoryApprovalStore,
        writes: AtomicUsize,
    }

    impl ApprovalStore for CountingStore {
        fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
            self.inner.load_all()
        }

        fn get(
            &self,
            approval_id: &ApprovalId,
        ) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
            self.inner.get(approval_id)
        }

        fn put_if_current(
            &self,
            expected: Option<&ApprovalRecord>,
            record: &ApprovalRecord,
        ) -> Result<(), ApprovalStoreError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.inner.put_if_current(expected, record)
        }

        fn is_persistent(&self) -> bool {
            true
        }
    }

    /// The decision's final checks come after its owner exit: an
    /// eligibility revoked, or the request expired, while that exit records
    /// an unrelated run's withdrawal leaves committed Approved history with
    /// its recorded withdrawal, no Approved notice and no spend. The
    /// unchanged control publishes and spends.
    #[tokio::test]
    async fn the_decision_final_checks_follow_its_owner_exit() {
        for change in [None, Some("revoke"), Some("expire")] {
            let (w, store, held, release) = gated_world();
            let other = world_with(w.approvals.clone());
            let id = w.request(&w.check()).await;
            let unrelated = other.request(&other.check()).await;
            // The unrelated run closes during the decision's own write, while
            // the owner is held: its withdrawal is queued and settled on the
            // decision's owner exit, where its write is gated.
            {
                let review = Arc::clone(&other.review);
                let work = other.work.clone();
                *store.hook.lock() = Some(Box::new(move || review.close_consent_for_work(&work)));
            }
            store.skip.store(1, Ordering::Release);
            store.armed.store(true, Ordering::Release);
            let decider = {
                let approvals = w.approvals.clone();
                let id = id.clone();
                std::thread::spawn(move || {
                    approvals.decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
                })
            };
            held.recv_timeout(BOUND)
                .expect("the owner exit holds the unrelated withdrawal's write");
            match change {
                Some("revoke") => w
                    .route
                    .publish(|eligible| eligible.retain(|id| id != "lead-1")),
                Some(_) => w.approvals.clock_skew.store(
                    i64::try_from(VALIDITY.as_secs()).expect("seconds") + 1,
                    Ordering::SeqCst,
                ),
                None => {}
            }
            let _ = release.send(());
            let decided = decider.join().expect("decider").expect("decision recorded");
            assert_eq!(
                decided.status,
                ApprovalStatus::Approved,
                "committed history"
            );
            assert_eq!(
                other.notices(&unrelated),
                vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Withdrawn]
            );
            let reason = decided
                .consent
                .and_then(|consent| consent.withdrawal)
                .map(|withdrawal| withdrawal.reason);
            match change {
                None => {
                    assert_eq!(reason, None);
                    assert_eq!(
                        w.notices(&id),
                        vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Approved]
                    );
                    assert_eq!(w.spend(&id), Ok(()));
                }
                Some(change) => {
                    assert_eq!(
                        reason,
                        Some(if change == "revoke" {
                            ApprovalConsentWithdrawalReason::AuthorityChanged
                        } else {
                            ApprovalConsentWithdrawalReason::Expired
                        })
                    );
                    assert_eq!(
                        w.notices(&id),
                        vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Withdrawn]
                    );
                    w.approvals.clock_skew.store(0, Ordering::SeqCst);
                    assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));
                }
            }
        }
    }

    /// When the decision's own withdrawal cannot be recorded, the result is
    /// the typed Unreconciled outcome, never a stale Approved success: the
    /// first commit stays, custody is retired, no Approved or Withdrawn
    /// notice is sent, and the withdrawal stays owed with its exact cause.
    /// Reconciliation settles it once against the row the store holds:
    /// written when nothing was, recognised (never written again) when the
    /// write had committed, and announced once. The approval stays Approved
    /// and nothing spends it.
    #[tokio::test]
    async fn a_failed_decision_withdrawal_is_unreconciled_not_approved() {
        for fault in [Fault::BeforeWrite, Fault::AfterWrite] {
            let store = Arc::new(FaultStore::default());
            let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            let id = w.request(&w.check()).await;
            // Revoked during the decision's own write; the withdrawal is the
            // second write and fails.
            let route = Arc::clone(&w.route);
            *store.hook.lock() = Some(Box::new(move || {
                route.publish(|eligible| eligible.retain(|id| id != "lead-1"));
            }));
            store.skip.store(1, Ordering::Release);
            *store.fault.lock() = Some(fault);
            assert_eq!(
                w.approvals
                    .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
                Err(ApprovalError::Unreconciled {
                    approval_id: id.clone()
                })
            );
            assert_eq!(w.notices(&id), vec![ConsentNoticeKind::Requested]);
            assert!(!w.approvals.state.read().consent.contains_key(&id));
            assert_eq!(
                owed_withdrawal_cause(&w, &id),
                Some(ApprovalConsentWithdrawalReason::AuthorityChanged),
                "{fault:?}"
            );
            let stored = store.get(&id).expect("read").expect("row");
            assert_eq!(
                stored.status,
                ApprovalStatus::Approved,
                "first commit stays"
            );
            assert_eq!(
                stored
                    .consent
                    .as_ref()
                    .and_then(|c| c.withdrawal.clone())
                    .is_some(),
                matches!(fault, Fault::AfterWrite)
            );

            let row = w.record(&id);
            assert_eq!(row.status, ApprovalStatus::Approved, "{fault:?}");
            assert_eq!(
                withdrawal_reason(&row),
                Some(ApprovalConsentWithdrawalReason::AuthorityChanged),
                "settled against the reconciled row: {fault:?}"
            );
            assert_eq!(store.get(&id).expect("read"), Some(row.clone()));
            if let Fault::AfterWrite = fault {
                assert_eq!(row, stored, "the committed withdrawal is not written again");
            }
            assert_eq!(owed_withdrawal_cause(&w, &id), None, "{fault:?}");
            assert_eq!(
                w.notices(&id),
                vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Withdrawn],
                "announced once, after its record: {fault:?}"
            );
            assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));
            assert_eq!(w.record(&id), row, "a later read changes nothing");
            assert_eq!(
                w.notices(&id)
                    .iter()
                    .filter(|kind| **kind == ConsentNoticeKind::Withdrawn)
                    .count(),
                1,
                "{fault:?}"
            );
        }
    }

    /// A pending request that expires and whose withdrawal cannot be
    /// recorded is never returned as current: the read is Unreconciled, the
    /// committed expiry stays, and the next read reconciles and records the
    /// owed withdrawal on the expired row, which keeps its status.
    #[tokio::test]
    async fn a_failed_expiry_withdrawal_is_unreconciled_on_read() {
        let store = Arc::new(FaultStore::default());
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let id = w.request(&w.check()).await;
        w.approvals.clock_skew.store(
            i64::try_from(VALIDITY.as_secs()).expect("seconds") + 1,
            Ordering::SeqCst,
        );
        store.skip.store(1, Ordering::Release);
        *store.fault.lock() = Some(Fault::BeforeWrite);
        assert_eq!(
            w.approvals.get(&id),
            Err(ApprovalError::Unreconciled {
                approval_id: id.clone()
            })
        );
        assert!(
            w.approvals
                .list(ApprovalListFilter::default())
                .expect("list reconciles first")
                .iter()
                .any(|record| record.approval_id == id
                    && record.status == ApprovalStatus::Expired
                    && withdrawal_reason(record) == Some(ApprovalConsentWithdrawalReason::Expired))
        );
        assert!(!w.approvals.state.read().consent.contains_key(&id));
        assert_eq!(owed_withdrawal_cause(&w, &id), None, "settled");
    }

    #[tokio::test]
    async fn a_restored_consent_row_is_history_only() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let approved = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&approved, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let pending = w.request(&w.check()).await;

        // A second service in this process opens the same store: it issued
        // nothing and holds no candidates. This is not a process restart.
        let reopened = world_with(ApprovalService::with_store(store).expect("second service"));
        let restored = reopened.record(&approved);
        assert_eq!(restored.status, ApprovalStatus::Approved);
        assert!(restored.consent.is_some(), "the consent history survives");
        assert_eq!(
            reopened
                .approvals
                .decide_consent(&pending, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::ConsentUnavailable {
                approval_id: pending.clone()
            })
        );
        assert_eq!(
            reopened.approvals.decide(
                &pending,
                ApprovalDecision::Approve,
                ApprovalPrincipalId::new("human:lead-1").expect("principal"),
                None,
                None,
            ),
            Err(ApprovalError::AuthenticationRequired {
                approval_id: pending.clone()
            }),
            "the restored consent flag still refuses caller text"
        );
        assert_eq!(
            admit_consent_attempt(Some(&reopened.review), &approved, &reopened.work, None).err(),
            Some(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::OwnerUnavailable,
            }),
            "a historical Approved row is not spendable consent: the second service \
             holds no custody for it, so it is unavailable here"
        );
    }

    fn hosting_domain(lock_dir: &str) -> crate::session_hosting::HostingDomainRef {
        crate::session_hosting::HostingPaths {
            hosting_lock_dir: lock_dir.into(),
            cold_delivery_lock: format!("{lock_dir}/cold-delivery.lock").into(),
            database: None,
        }
        .domain_ref()
    }

    fn consent_scope(lock_dir: &str, session_id: SessionId) -> ApprovalConsentScope {
        ApprovalConsentScope {
            domain: hosting_domain(lock_dir),
            session_id,
        }
    }

    /// Consent owner loss (3): custody issued under work that carries the
    /// runtime owner's hosting claim lineage keeps that lineage for as long
    /// as the custody lives, so the session's claim is never released under
    /// live custody, even after the run's own work context is gone. Work
    /// without a lineage gives its custody none.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn consent_custody_keeps_the_session_hosting_claim_lineage() {
        use crate::session_hosting::{
            HostingCapability, HostingOwner, HostingPaths, SessionServing, grant_session_hosting,
            local_session_serving, session_hosting_lock_path,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let capability = HostingCapability::OsLock(Arc::new(HostingPaths {
            hosting_lock_dir: dir.path().join("hosting"),
            cold_delivery_lock: dir.path().join("cold-delivery.lock"),
            database: None,
        }));
        let owner = HostingOwner::mint();
        let session = SessionId::new();
        let claim = grant_session_hosting(&capability, &owner, &session).expect("claim");
        let mut w = world();
        let unhosted = w.request(&w.check()).await;
        assert!(
            w.approvals
                .state
                .read()
                .consent
                .get(&unhosted)
                .expect("live custody")
                .hosting
                .is_none(),
            "work without a lineage gives its custody none"
        );

        w.work = w
            .work
            .clone()
            .with_consent_scope(ApprovalConsentScope {
                domain: capability.domain_ref().expect("a cross-process domain"),
                session_id: session.clone(),
            })
            .with_consent_hosting(claim);
        let hosted = w.request(&w.check()).await;
        let kept = w
            .approvals
            .state
            .read()
            .consent
            .get(&hosted)
            .expect("live custody")
            .hosting
            .clone()
            .expect("the custody keeps the lineage");
        assert_eq!(kept.session_id(), &session);
        assert_eq!(kept.owner(), &owner);
        drop(kept);

        // The run's work context goes; only the live custody holds the claim.
        w.work = WorkAuthorizationContext::new(
            Arc::new(OwnerWork(Arc::clone(&w.owner))),
            OperationExecutionScope::Domain,
        );
        assert_eq!(
            local_session_serving(&capability, &owner, &session),
            SessionServing::HeldHere,
            "live custody keeps the session's claim held"
        );
        let peer = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(session_hosting_lock_path(
                &dir.path().join("hosting"),
                &session,
            ))
            .expect("the claim's lock file");
        assert!(
            peer.try_lock().is_err(),
            "no other process can take the claim under live custody"
        );
    }

    /// Consent owner loss (3): custody that moves into a resumed run keeps
    /// the hosting claim lineage it was admitted under. Once the closed run's
    /// work and the parked candidate are gone, and the resumed run's own work
    /// carries no lineage, the moved custody alone keeps the session's claim
    /// held.
    #[tokio::test]
    async fn transferred_consent_custody_keeps_the_session_hosting_claim_lineage() {
        use crate::session_hosting::{
            HostingCapability, HostingOwner, HostingPaths, SessionServing, grant_session_hosting,
            local_session_serving,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let capability = HostingCapability::OsLock(Arc::new(HostingPaths {
            hosting_lock_dir: dir.path().join("hosting"),
            cold_delivery_lock: dir.path().join("cold-delivery.lock"),
            database: None,
        }));
        let owner = HostingOwner::mint();
        let session = SessionId::new();
        let claim = grant_session_hosting(&capability, &owner, &session).expect("claim");
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = w
            .work
            .clone()
            .with_consent_hosting(claim)
            .with_retained_work(origin_work("runtime-a"));
        let id = parked_approval(&w).await;
        w.review.close_consent_for_work(&w.work);
        let input = InputId::new();
        admit_notice(&w, &id, &input);
        let (resumed_work, run_id) = resumed(&w, &[&input]);
        assert!(
            resumed_work.consent_hosting().is_none(),
            "the resumed run's work carries no lineage of its own"
        );
        // The closed run's work goes.
        w.work = WorkAuthorizationContext::new(
            Arc::new(OwnerWork(Arc::clone(&w.owner))),
            OperationExecutionScope::Domain,
        );

        w.review.open_consent_for_run(&resumed_work, &run_id);

        assert_eq!(
            custody_of(&store, &id),
            Some(ApprovalConsentCustody::Transferred {
                input_id: input.to_string()
            }),
            "the custody moved"
        );
        assert_eq!(
            local_session_serving(&capability, &owner, &session),
            SessionServing::HeldHere,
            "the moved custody keeps the session's claim held"
        );
        let kept = w
            .approvals
            .state
            .read()
            .consent
            .get(&id)
            .expect("moved custody")
            .hosting
            .clone()
            .expect("the moved custody keeps the lineage");
        assert_eq!(kept.session_id(), &session);
        assert_eq!(kept.owner(), &owner);
    }

    /// The owner scope of the admitting run is committed with the consent
    /// approval's issuance and survives every later transition of its row
    /// unchanged: decision, spend, denial, run withdrawal and expiry (here
    /// observed by a second service of the store). Work
    /// without a scope issues none.
    #[tokio::test]
    async fn the_consent_scope_is_committed_at_issuance_and_never_changes() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let scope = consent_scope("realm-a/hosting", SessionId::new());
        w.work = w.work.clone().with_consent_scope(scope.clone());
        let scope_of = |id: &ApprovalId| {
            store
                .get(id)
                .expect("read")
                .expect("row")
                .consent
                .expect("consent")
                .scope
        };

        let approved = w.request(&w.check()).await;
        assert_eq!(scope_of(&approved), Some(scope.clone()), "issued with it");
        w.approvals
            .decide_consent(&approved, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        assert_eq!(
            scope_of(&approved),
            Some(scope.clone()),
            "decision keeps it"
        );
        assert_eq!(w.spend(&approved), Ok(()));
        assert_eq!(scope_of(&approved), Some(scope.clone()), "spend keeps it");

        let denied = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&denied, decision(ApprovalDecision::Deny, "lead-1"))
            .expect("denied");
        assert_eq!(scope_of(&denied), Some(scope.clone()), "denial keeps it");

        let withdrawn = w.request(&w.check()).await;
        w.review.close_consent_for_work(&w.work);
        let row = store.get(&withdrawn).expect("read").expect("row");
        assert_eq!(row.status, ApprovalStatus::Cancelled);
        assert!(row.consent.and_then(|consent| consent.withdrawal).is_some());
        assert_eq!(
            scope_of(&withdrawn),
            Some(scope.clone()),
            "withdrawal keeps it"
        );

        let expiring = w.request(&w.check()).await;
        let row = store.get(&expiring).expect("read").expect("row");
        let mut past = row.clone();
        past.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        store
            .put_if_current(Some(&row), &past)
            .expect("an expiry already due in the store");
        let second = ApprovalService::with_store(store.clone()).expect("second service");
        second
            .list(ApprovalListFilter::default())
            .expect("list observes expiry");
        let expired = store.get(&expiring).expect("read").expect("row");
        assert_eq!(expired.status, ApprovalStatus::Expired);
        assert_eq!(scope_of(&expiring), Some(scope), "expiry keeps it");

        let unscoped = world_with(ApprovalService::with_store(store.clone()).expect("unscoped"));
        let plain = unscoped.request(&unscoped.check()).await;
        assert_eq!(scope_of(&plain), None, "work without a scope issues none");
    }

    /// The stored form of the scope is compatible both ways. A scoped row
    /// round-trips exactly; a consent row written without one (by a writer
    /// that predates it) restores as unscoped history, which takes no part
    /// in recovery, wake or handoff; and equal session ids in two hosting
    /// domains are different scopes.
    #[test]
    fn the_stored_consent_scope_is_compatible_and_domain_qualified() {
        let session = SessionId::new();
        let scope = consent_scope("realm-a/hosting", session.clone());
        assert_ne!(
            scope,
            consent_scope("realm-b/hosting", session.clone()),
            "an equal session id in another domain is another scope"
        );
        assert_eq!(scope, consent_scope("realm-a/hosting", session));

        let consent = ApprovalConsentRecord {
            scope: Some(scope.clone()),
            ..ApprovalConsentRecord::default()
        };
        let stored = serde_json::to_value(&consent).expect("serialize");
        assert_eq!(
            stored["scope"]["domain"],
            serde_json::Value::String(scope.domain.as_str().to_owned())
        );
        assert_eq!(
            serde_json::from_value::<ApprovalConsentRecord>(stored).expect("deserialize"),
            consent,
            "a scoped row round-trips exactly"
        );

        let predating = serde_json::json!({});
        assert_eq!(
            serde_json::from_value::<ApprovalConsentRecord>(predating).expect("deserialize"),
            ApprovalConsentRecord::default(),
            "a row written before the scope restores unscoped"
        );
        assert!(
            serde_json::to_value(ApprovalConsentRecord::default())
                .expect("serialize")
                .get("scope")
                .is_none(),
            "an unscoped row stores no scope field"
        );
    }

    /// A consent row restored without a scope is ordinary history: the same
    /// owner rules apply to it as before the scope existed.
    #[tokio::test]
    async fn an_unscoped_stored_consent_row_restores_as_history() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let approved = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&approved, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let row = store.get(&approved).expect("read").expect("row");
        assert_eq!(
            row.consent
                .as_ref()
                .and_then(|consent| consent.scope.clone()),
            None
        );

        let second = world_with(ApprovalService::with_store(store).expect("second service"));
        assert_eq!(second.record(&approved), row, "restored unchanged");
        assert_eq!(
            admit_consent_attempt(Some(&second.review), &approved, &second.work, None).err(),
            Some(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::OwnerUnavailable,
            })
        );
    }

    fn origin_work(runtime_id: &str) -> crate::retained_work::RetainedWorkIdentity {
        crate::retained_work::RetainedWorkIdentity::new(
            runtime_id,
            RunId::new(),
            Vec::new(),
            std::collections::BTreeMap::new(),
            None,
        )
    }

    fn deliveries_of(
        store: &InMemoryApprovalStore,
        id: &ApprovalId,
    ) -> Vec<(ApprovalConsentTransition, ApprovalConsentDeliveryState)> {
        store
            .get(id)
            .expect("read")
            .expect("row")
            .consent
            .expect("consent")
            .deliveries
            .into_iter()
            .map(|delivery| (delivery.transition, delivery.state))
            .collect()
    }

    /// The exact destination of a consent's notices (its scope and the
    /// admitting run's retained work) is committed at issuance, and each
    /// committed transition then owes its own notice with its own row write:
    /// a decision owes its notice before or after its run's close, a denial
    /// owes its notice, a consent whose validity lapsed before the close is
    /// withdrawn there and owes that withdrawal next to its decision's
    /// notice, a park owes nothing, and a spend owes nothing new. A later
    /// transition never settles or replaces an earlier one's notice.
    #[tokio::test(start_paused = true)]
    async fn every_committed_consent_transition_owes_its_own_notice() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        let session = SessionId::new();
        let scope = consent_scope("realm-a/hosting", session.clone());
        let origin = origin_work("runtime-a");
        w.work = w
            .work
            .clone()
            .with_consent_scope(scope.clone())
            .with_retained_work(origin.clone());
        let owed = ApprovalConsentDeliveryState::Owed;

        // A decision, then the candidate's validity lapses before the close.
        let lapsed = w.request(&w.check()).await;
        let issued = store.get(&lapsed).expect("read").expect("row");
        assert_eq!(
            issued
                .consent
                .as_ref()
                .and_then(|consent| consent.origin.clone()),
            Some(ApprovalConsentOrigin {
                retained_work: origin.clone()
            }),
            "the origin is committed at issuance, with the scope"
        );
        assert!(
            deliveries_of(&store, &lapsed).is_empty(),
            "issuance owes nothing"
        );
        w.approvals
            .decide_consent(&lapsed, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        assert_eq!(
            deliveries_of(&store, &lapsed),
            vec![(ApprovalConsentTransition::Approved, owed.clone())]
        );
        tokio::time::advance(VALIDITY + Duration::from_secs(1)).await;

        // A live decision, then the run closes before any attempt.
        let approved = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&approved, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        assert_eq!(
            deliveries_of(&store, &approved),
            vec![(ApprovalConsentTransition::Approved, owed.clone())]
        );

        let denied = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&denied, decision(ApprovalDecision::Deny, "lead-1"))
            .expect("denied");
        assert_eq!(
            deliveries_of(&store, &denied),
            vec![(ApprovalConsentTransition::Denied, owed.clone())]
        );

        let spent = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&spent, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        assert_eq!(w.spend(&spent), Ok(()));
        assert_eq!(
            deliveries_of(&store, &spent),
            vec![(ApprovalConsentTransition::Approved, owed.clone())],
            "the spend owes nothing new"
        );

        let pending = w.request(&w.check()).await;
        w.review.close_consent_for_work(&w.work);
        assert_eq!(
            deliveries_of(&store, &lapsed),
            vec![
                (ApprovalConsentTransition::Approved, owed.clone()),
                (ApprovalConsentTransition::Withdrawn, owed.clone()),
            ],
            "the close withdraws a lapsed consent and keeps the decision's notice"
        );
        assert_eq!(
            deliveries_of(&store, &approved),
            vec![(ApprovalConsentTransition::Approved, owed.clone())],
            "a park owes nothing"
        );
        assert!(
            deliveries_of(&store, &pending).is_empty(),
            "a park owes nothing"
        );
        assert_eq!(
            deliveries_of(&store, &denied),
            vec![(ApprovalConsentTransition::Denied, owed.clone())],
            "a declined consent has no custody left to close"
        );
        w.approvals
            .decide_consent(&pending, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("decided after the close");
        assert_eq!(
            deliveries_of(&store, &pending),
            vec![(ApprovalConsentTransition::Approved, owed.clone())],
            "a decision after the close owes its notice"
        );

        // The issuing owner answers for exactly what it committed.
        let facts = w
            .approvals
            .consent_delivery(&approved, ApprovalConsentTransition::Approved)
            .expect("readable")
            .expect("owed");
        assert_eq!(facts.scope, scope, "the full scope, domain included");
        assert_eq!(facts.scope.session_id, session);
        assert_eq!(facts.retained_work, origin);
        assert_eq!(facts.state, owed);
        assert!(facts.result_digest.starts_with("sha256:"));
        assert_eq!(
            w.approvals
                .consent_delivery(&approved, ApprovalConsentTransition::Approved)
                .expect("readable")
                .map(|again| again.result_digest),
            Some(facts.result_digest),
            "the digest is stable"
        );
        assert_ne!(
            w.approvals
                .consent_delivery(&lapsed, ApprovalConsentTransition::Withdrawn)
                .expect("readable")
                .expect("owed")
                .result_digest,
            w.approvals
                .consent_delivery(&lapsed, ApprovalConsentTransition::Approved)
                .expect("readable")
                .expect("owed")
                .result_digest,
            "each transition has its own digest"
        );
        assert_eq!(
            w.approvals
                .consent_delivery(&denied, ApprovalConsentTransition::Approved)
                .expect("readable"),
            None,
            "a transition that never committed owes nothing"
        );
        let discovered = w.approvals.owed_consent_deliveries().expect("available");
        assert!(discovered.unreconciled.is_empty());
        let mut owed_list = discovered.owed;
        owed_list.sort();
        let mut expected = vec![
            (lapsed.clone(), ApprovalConsentTransition::Approved),
            (lapsed.clone(), ApprovalConsentTransition::Withdrawn),
            (approved.clone(), ApprovalConsentTransition::Approved),
            (denied.clone(), ApprovalConsentTransition::Denied),
            (spent.clone(), ApprovalConsentTransition::Approved),
            (pending, ApprovalConsentTransition::Approved),
        ];
        expected.sort();
        assert_eq!(owed_list, expected);
    }

    /// Without both a scope and an origin a consent has no exact native
    /// destination: it owes no notice for any transition, and its live
    /// consent keeps working as before.
    #[tokio::test]
    async fn an_unrouted_consent_owes_no_notice() {
        for (scoped, retained) in [(true, false), (false, true), (false, false)] {
            let store = Arc::new(InMemoryApprovalStore::new());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            if scoped {
                w.work = w
                    .work
                    .clone()
                    .with_consent_scope(consent_scope("realm-a/hosting", SessionId::new()));
            }
            if retained {
                w.work = w.work.clone().with_retained_work(origin_work("runtime-a"));
            }
            let approved = w.request(&w.check()).await;
            w.approvals
                .decide_consent(&approved, decision(ApprovalDecision::Approve, "lead-1"))
                .expect("approved");
            let denied = w.request(&w.check()).await;
            w.approvals
                .decide_consent(&denied, decision(ApprovalDecision::Deny, "lead-1"))
                .expect("denied");
            let pending = w.request(&w.check()).await;
            assert_eq!(w.spend(&approved), Ok(()), "live consent still spends");
            w.review.close_consent_for_work(&w.work);
            for id in [&approved, &denied, &pending] {
                assert!(
                    deliveries_of(&store, id).is_empty(),
                    "scoped={scoped} retained={retained}: no destination, no notice"
                );
            }
            assert_eq!(
                w.approvals.owed_consent_deliveries(),
                Ok(OwedConsentDeliveries::default())
            );
        }
    }

    /// `work` with an exact native destination for its consent notices: an
    /// owner scope and the admitting run's retained work.
    fn routed(work: WorkAuthorizationContext) -> WorkAuthorizationContext {
        work.with_consent_scope(consent_scope("realm-a/hosting", SessionId::new()))
            .with_retained_work(origin_work("runtime-a"))
    }

    fn custody_of(
        store: &InMemoryApprovalStore,
        id: &ApprovalId,
    ) -> Option<ApprovalConsentCustody> {
        store
            .get(id)
            .expect("read")
            .expect("row")
            .consent
            .and_then(|consent| consent.custody)
    }

    fn is_parked(w: &World, id: &ApprovalId) -> bool {
        w.approvals.state.read().parked.contains(id)
    }

    /// A run's close parks the unspent consent it admitted whose notices
    /// have an exact native destination, instead of withdrawing it: the park
    /// is recorded durably, the approval keeps its status and announces
    /// nothing, nothing spends a parked candidate (neither a fresh attempt
    /// nor one resolved before the close), and the closed run's retained
    /// decision going stale never withdraws it.
    #[tokio::test]
    async fn a_closed_run_parks_its_routed_consent() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let approved = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&approved, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let pending = w.request(&w.check()).await;
        let in_flight = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&in_flight, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        let (entry, admitted) = w.attempt(&in_flight).expect("resolved before the close");

        w.review.close_consent_for_work(&w.work);

        for id in [&approved, &pending, &in_flight] {
            assert!(is_parked(&w, id), "custody kept");
            assert_eq!(
                custody_of(&store, id),
                Some(ApprovalConsentCustody::Parked),
                "the park is recorded durably"
            );
            assert!(
                w.record(id)
                    .consent
                    .and_then(|consent| consent.withdrawal)
                    .is_none(),
                "nothing is withdrawn"
            );
            assert!(!w.notices(id).contains(&ConsentNoticeKind::Withdrawn));
            assert_eq!(
                w.attempt(id).err(),
                Some(human_consent_required()),
                "a fresh attempt never resolves a parked candidate"
            );
        }
        assert_eq!(w.record(&approved).status, ApprovalStatus::Approved);
        assert_eq!(w.record(&pending).status, ApprovalStatus::Pending);
        let guard = ReviewDispatchGuard::new(entry.clone());
        assert_eq!(
            entry.consume(&admitted),
            Err(human_consent_required()),
            "an attempt resolved before the close never spends after it"
        );
        guard.complete();
        assert!(is_parked(&w, &in_flight), "and leaves the parked custody");
        assert!(
            w.record(&in_flight)
                .consent
                .and_then(|consent| consent.consumed_at)
                .is_none()
        );

        // The closed run's retained decision is no longer current: reading
        // the approvals never withdraws a parked candidate for it.
        w.owner.sequence.fetch_add(1, Ordering::AcqRel);
        for id in [&approved, &pending, &in_flight] {
            let _ = w.record(id);
            assert!(
                is_parked(&w, id),
                "the staleness backstop leaves a parked candidate"
            );
            assert!(!w.notices(id).contains(&ConsentNoticeKind::Withdrawn));
        }
    }

    /// The decided consent a scan names, or `None` when it was deferred.
    fn named(scan: DecidedConsentScan) -> Option<Vec<ApprovalId>> {
        match scan {
            DecidedConsentScan::Decided(decided) => Some(
                decided
                    .into_iter()
                    .map(|consent| consent.approval_id)
                    .collect(),
            ),
            DecidedConsentScan::Deferred => None,
        }
    }

    /// The approved consent of a still-open run is named for that run alone,
    /// and only when the run asks about it (decided, published, unparked,
    /// unexpired and unspent). A held owner defers the look and never reports
    /// none. The run spends it before its close: the close then finds nothing
    /// of it to park or withdraw, while pending consent of the same run parks.
    /// Spent consent is never named again, and parked consent never is.
    #[tokio::test]
    async fn a_run_spends_its_decided_consent_before_its_close() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let decided = w.request(&w.check()).await;
        let pending = w.request(&w.check()).await;
        let requested = [decided.clone(), pending.clone()];
        assert_eq!(
            named(
                w.approvals
                    .decided_consent_for_run(&w.work, None, &requested)
            ),
            Some(Vec::new()),
            "nothing is decided yet"
        );
        w.approvals
            .decide_consent(&decided, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        assert_eq!(
            named(
                w.approvals
                    .decided_consent_for_run(&w.work, None, &requested)
            ),
            Some(vec![decided.clone()]),
            "only this run's approved consent"
        );
        assert_eq!(
            named(w.approvals.decided_consent_for_run(
                &w.work,
                None,
                std::slice::from_ref(&pending)
            )),
            Some(Vec::new()),
            "consent the run does not ask about is never examined"
        );
        {
            let _held = w.approvals.state.write();
            assert_eq!(
                w.approvals
                    .decided_consent_for_run(&w.work, None, &requested),
                DecidedConsentScan::Deferred,
                "a held owner defers the look"
            );
        }
        let other_work = routed(WorkAuthorizationContext::new(
            Arc::new(OwnerWork(Arc::clone(&w.owner))),
            OperationExecutionScope::Domain,
        ));
        assert_eq!(
            named(
                w.approvals
                    .decided_consent_for_run(&other_work, None, &requested)
            ),
            Some(Vec::new()),
            "another work's run has none"
        );
        assert_eq!(
            named(
                w.approvals
                    .decided_consent_for_run(&w.work, Some(&RunId::new()), &requested)
            ),
            Some(Vec::new()),
            "another run of the work has none"
        );

        assert_eq!(w.spend(&decided), Ok(()), "the open run spends it once");
        assert_eq!(
            named(
                w.approvals
                    .decided_consent_for_run(&w.work, None, &requested)
            ),
            Some(Vec::new()),
            "spent consent is never named again"
        );

        w.review.close_consent_for_work(&w.work);
        assert!(!is_parked(&w, &decided), "nothing of it is left to park");
        let consent = w.record(&decided).consent.expect("consent history");
        assert!(consent.consumed_at.is_some(), "spent before the close");
        assert!(consent.withdrawal.is_none(), "never withdrawn by the close");
        assert!(!w.notices(&decided).contains(&ConsentNoticeKind::Withdrawn));
        assert!(is_parked(&w, &pending), "the run's pending consent parks");
        w.approvals
            .decide_consent(&pending, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("a decision after the close commits");
        assert_eq!(
            named(
                w.approvals
                    .decided_consent_for_run(&w.work, None, &requested)
            ),
            Some(Vec::new()),
            "parked consent is never named for the closed run"
        );
    }

    /// A human decision after the run's close commits on the parked
    /// candidate even though the closed run's retained decision is no longer
    /// current: it is announced and owes its notice, and the candidate stays
    /// parked and unspendable. Its validity still applies: a parked
    /// candidate past its deadline is withdrawn on its decision instead.
    #[tokio::test(start_paused = true)]
    async fn a_decision_after_the_close_commits_on_the_parked_consent() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let pending = w.request(&w.check()).await;
        let lapsing = w.request(&w.check()).await;
        w.review.close_consent_for_work(&w.work);
        w.owner.sequence.fetch_add(1, Ordering::AcqRel);

        let decided = w
            .approvals
            .decide_consent(&pending, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("decided after the close");
        assert_eq!(decided.status, ApprovalStatus::Approved);
        assert_eq!(
            store.get(&pending).expect("read").expect("row"),
            decided,
            "committed"
        );
        assert_eq!(
            deliveries_of(&store, &pending),
            vec![(
                ApprovalConsentTransition::Approved,
                ApprovalConsentDeliveryState::Owed
            )]
        );
        assert_eq!(
            w.notices(&pending).last(),
            Some(&ConsentNoticeKind::Approved)
        );
        assert!(is_parked(&w, &pending), "still parked");
        assert_eq!(
            custody_of(&store, &pending),
            Some(ApprovalConsentCustody::Parked)
        );
        assert_eq!(w.attempt(&pending).err(), Some(human_consent_required()));

        tokio::time::advance(VALIDITY + Duration::from_secs(1)).await;
        assert_eq!(
            w.approvals
                .decide_consent(&lapsing, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::Expired {
                approval_id: lapsing.clone()
            })
        );
        assert!(!is_parked(&w, &lapsing));
        assert_eq!(
            deliveries_of(&store, &lapsing),
            vec![(
                ApprovalConsentTransition::Withdrawn,
                ApprovalConsentDeliveryState::Owed
            )],
            "the expiry withdraws it and owes that withdrawal"
        );
    }

    /// A decision racing its run's close commits on the parked row: the park
    /// changes only the executable custody, which eligibility never reads,
    /// whether it was recorded before the decision's reservation or is
    /// recorded inside it. Nothing conflicts and nothing is abandoned.
    #[tokio::test]
    async fn a_decision_racing_the_close_commits_on_the_parked_row() {
        for recorded_first in [true, false] {
            let store = Arc::new(InMemoryApprovalStore::new());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            let id = w.request(&w.check()).await;
            let approvals = w.approvals.clone();
            let review = Arc::clone(&w.review);
            let work = w.work.clone();
            *w.route.after_resolve.lock() = Some(Box::new(move || {
                if recorded_first {
                    review.close_consent_for_work(&work);
                } else {
                    // A store-free release: the park waits for its record.
                    let mut state = approvals.state.write();
                    approvals.retire_work_custody_locked(&mut state, &work);
                }
            }));
            let decided = w
                .approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
                .expect("the racing decision commits");
            assert_eq!(decided.status, ApprovalStatus::Approved, "{recorded_first}");
            assert_eq!(store.get(&id).expect("read").expect("row"), decided);
            assert_eq!(
                custody_of(&store, &id),
                Some(ApprovalConsentCustody::Parked)
            );
            assert_eq!(
                deliveries_of(&store, &id),
                vec![(
                    ApprovalConsentTransition::Approved,
                    ApprovalConsentDeliveryState::Owed
                )]
            );
            assert!(is_parked(&w, &id));
            assert!(w.approvals.state.read().unreconciled.is_empty());
            assert_eq!(w.notices(&id).last(), Some(&ConsentNoticeKind::Approved));
            assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));
        }
    }

    /// A park whose store write provably committed nothing parked nothing:
    /// the candidate is withdrawn as its run closed, through the
    /// withdrawal's own record, and nothing spends it.
    #[tokio::test]
    async fn a_park_that_wrote_nothing_withdraws_as_the_run_closed() {
        let store = Arc::new(FaultStore::default());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        *store.fault.lock() = Some(Fault::BeforeWrite);

        w.review.close_consent_for_work(&w.work);

        assert!(!is_parked(&w, &id));
        assert_eq!(custody_of(&store.inner, &id), None, "nothing was parked");
        let row = w.record(&id);
        assert_eq!(row.status, ApprovalStatus::Approved);
        assert_eq!(
            row.consent
                .and_then(|consent| consent.withdrawal)
                .map(|withdrawal| withdrawal.reason),
            Some(ApprovalConsentWithdrawalReason::RunEnded),
            "withdrawn as its run closed"
        );
        assert_eq!(
            deliveries_of(&store.inner, &id),
            vec![
                (
                    ApprovalConsentTransition::Approved,
                    ApprovalConsentDeliveryState::Owed
                ),
                (
                    ApprovalConsentTransition::Withdrawn,
                    ApprovalConsentDeliveryState::Owed
                ),
            ]
        );
        assert_eq!(w.notices(&id).last(), Some(&ConsentNoticeKind::Withdrawn));
        assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));
    }

    /// A park whose store write may have committed is reconciled, never
    /// spent: its custody is abandoned at once, a later decision finds no
    /// candidate, and the reconciled row reports the park the store holds
    /// with nothing withdrawn or announced.
    #[tokio::test]
    async fn a_park_whose_commit_is_unknown_is_reconciled_never_spent() {
        let store = Arc::new(FaultStore::default());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let id = w.request(&w.check()).await;
        *store.fault.lock() = Some(Fault::AfterWrite);

        w.review.close_consent_for_work(&w.work);

        assert!(!is_parked(&w, &id));
        assert!(w.approvals.state.read().unreconciled.contains_key(&id));
        assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));
        assert_eq!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::ConsentUnavailable {
                approval_id: id.clone()
            }),
            "no candidate survives an uncertain park"
        );
        let row = w.record(&id);
        assert_eq!(row.status, ApprovalStatus::Pending);
        assert_eq!(
            row.consent.and_then(|consent| consent.custody),
            Some(ApprovalConsentCustody::Parked),
            "reconciled from the store"
        );
        assert!(deliveries_of(&store.inner, &id).is_empty());
        assert!(!w.notices(&id).contains(&ConsentNoticeKind::Withdrawn));
    }

    /// Commit the receipt of `id`'s approved notice: admitted as `input`
    /// into its destination session (what the delivery composition records).
    fn admit_notice(w: &World, id: &ApprovalId, input: &InputId) {
        let mut state = w.approvals.state.write();
        let record = state.records.get(id).cloned().expect("row");
        let mut admitted = record.clone();
        let consent = admitted.consent.as_mut().expect("consent");
        let session_id = consent.scope.as_ref().expect("scope").session_id.clone();
        let delivery = consent
            .deliveries
            .iter_mut()
            .find(|delivery| delivery.transition == ApprovalConsentTransition::Approved)
            .expect("owed");
        delivery.state = ApprovalConsentDeliveryState::Admitted {
            session_id,
            input_id: input.to_string(),
        };
        w.approvals
            .commit_row_locked(&mut state, Some(&record), &admitted)
            .expect("receipt committed");
        state.records.insert(id.clone(), admitted);
    }

    /// A resumed run of `w`'s routed work: its own run and identity, the same
    /// destination, staged with `inputs` resuming the closed run's work.
    fn resumed(w: &World, inputs: &[&InputId]) -> (WorkAuthorizationContext, RunId) {
        let run_id = RunId::new();
        let origin = w.work.retained_work().expect("routed").clone();
        let work = WorkAuthorizationContext::new(
            Arc::new(OwnerWork(Arc::clone(&w.owner))),
            OperationExecutionScope::Domain,
        )
        .with_consent_scope(w.work.consent_scope().expect("routed").clone())
        .with_retained_work(crate::retained_work::RetainedWorkIdentity::new(
            "runtime-a",
            run_id.clone(),
            Vec::new(),
            std::collections::BTreeMap::new(),
            None,
        ))
        .with_resumed_inputs(
            inputs
                .iter()
                .map(|input_id| ResumedInput {
                    input_id: (*input_id).clone(),
                    resumes: origin.clone(),
                })
                .collect(),
        );
        (work, run_id)
    }

    fn attempt_in(
        w: &World,
        id: &ApprovalId,
        work: &WorkAuthorizationContext,
        run_id: &RunId,
    ) -> Result<(ReviewEntry, PreparedOperationCheck), crate::ToolError> {
        admit_consent_attempt(Some(&w.review), id, work, Some(run_id))
    }

    fn spend_in(
        w: &World,
        id: &ApprovalId,
        work: &WorkAuthorizationContext,
        run_id: &RunId,
    ) -> Result<(), crate::ToolError> {
        let (entry, admitted) = attempt_in(w, id, work, run_id)?;
        let guard = ReviewDispatchGuard::new(entry.clone());
        let spent = entry.consume(&admitted);
        guard.complete();
        spent
    }

    fn owner_unavailable() -> crate::ToolError {
        crate::ToolError::ReviewUnavailable {
            kind: ReviewUnavailableKind::OwnerUnavailable,
        }
    }

    /// A routed run's approved consent, parked at its close.
    async fn parked_approval(w: &World) -> ApprovalId {
        let id = w.request(&w.check()).await;
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        id
    }

    /// A resumed run staged with the input its committed receipt names takes
    /// the parked consent over: the transfer is recorded on the row, the same
    /// attempt then resolves only in that run (never the closed run's work
    /// or another run of the resumed work) and spends exactly once.
    #[tokio::test]
    async fn a_resumed_run_takes_over_the_parked_consent_its_receipt_names() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let id = parked_approval(&w).await;
        w.review.close_consent_for_work(&w.work);
        let input = InputId::new();
        admit_notice(&w, &id, &input);
        let (resumed_work, run_id) = resumed(&w, &[&input]);

        w.review.open_consent_for_run(&resumed_work, &run_id);

        assert!(!is_parked(&w, &id));
        assert_eq!(
            custody_of(&store, &id),
            Some(ApprovalConsentCustody::Transferred {
                input_id: input.to_string()
            })
        );
        assert_eq!(
            w.attempt(&id).err(),
            Some(human_consent_required()),
            "the closed run's work never resolves it"
        );
        assert_eq!(
            attempt_in(&w, &id, &resumed_work, &RunId::new()).err(),
            Some(human_consent_required()),
            "nor another run of the resumed work"
        );
        assert_eq!(spend_in(&w, &id, &resumed_work, &run_id), Ok(()));
        assert!(
            w.record(&id)
                .consent
                .and_then(|consent| consent.consumed_at)
                .is_some()
        );
        assert_eq!(
            spend_in(&w, &id, &resumed_work, &run_id).err(),
            Some(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                approval_id: None,
            }),
            "once"
        );
        assert_eq!(w.notices(&id).last(), Some(&ConsentNoticeKind::Used));
    }

    /// Only the exact receipt moves parked consent: a resumed run staged
    /// with another input of the same work, or with the receipt's input but
    /// resuming other work, moves nothing and strands nothing at its close;
    /// a context naming another run moves nothing. The exact run then moves
    /// it.
    #[tokio::test]
    async fn only_the_exact_receipt_moves_parked_consent() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let id = parked_approval(&w).await;
        w.review.close_consent_for_work(&w.work);
        let input = InputId::new();
        admit_notice(&w, &id, &input);

        let other_input = InputId::new();
        let (copy, copy_run) = resumed(&w, &[&other_input]);
        w.review.open_consent_for_run(&copy, &copy_run);
        assert!(is_parked(&w, &id), "another input of the same work");
        assert_eq!(
            attempt_in(&w, &id, &copy, &copy_run).err(),
            Some(owner_unavailable()),
            "a run resuming its work waits for its own receipt"
        );

        let (elsewhere, elsewhere_run) = {
            let (work, run_id) = resumed(&w, &[&input]);
            let work = work.with_resumed_inputs(vec![ResumedInput {
                input_id: input.clone(),
                resumes: origin_work("runtime-a"),
            }]);
            (work, run_id)
        };
        w.review.open_consent_for_run(&elsewhere, &elsewhere_run);
        assert!(
            is_parked(&w, &id),
            "the receipt's input resuming other work"
        );
        assert_eq!(
            attempt_in(&w, &id, &elsewhere, &elsewhere_run).err(),
            Some(human_consent_required())
        );

        let (exact, exact_run) = resumed(&w, &[&input]);
        w.review.open_consent_for_run(&exact, &RunId::new());
        assert!(is_parked(&w, &id), "a context naming another run");

        w.review.close_consent_for_work(&copy);
        w.review.close_consent_for_work(&elsewhere);
        assert!(is_parked(&w, &id), "their closes strand nothing");
        assert_eq!(
            custody_of(&store, &id),
            Some(ApprovalConsentCustody::Parked)
        );

        w.review.open_consent_for_run(&exact, &exact_run);
        assert!(!is_parked(&w, &id));
        assert_eq!(spend_in(&w, &id, &exact, &exact_run), Ok(()));
    }

    /// A run of the consent's origin work staged with the receipt's input,
    /// but for another destination (another session in the same hosting
    /// domain, or the same session in another domain), never takes the
    /// consent: with the receipt committed before it opens, it moves nothing
    /// and its close strands nothing; with the receipt published after its
    /// close, that close settles nothing. The run for the committed
    /// destination then moves the consent, and it spends once.
    #[tokio::test]
    async fn a_receipt_moves_consent_only_into_its_destination() {
        for (other_domain, receipt_first) in
            [(false, true), (false, false), (true, true), (true, false)]
        {
            let case = format!("other_domain={other_domain} receipt_first={receipt_first}");
            let store = Arc::new(InMemoryApprovalStore::new());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            let session = destination(&w);
            let id = parked_approval(&w).await;
            w.review.close_consent_for_work(&w.work);
            let input = InputId::new();
            let publish = || {
                w.approvals
                    .settle_consent_delivery(
                        &id,
                        ApprovalConsentTransition::Approved,
                        admitted(&session, &input),
                    )
                    .expect("receipt settled");
            };
            if receipt_first {
                publish();
            }
            let (work, run_id) = resumed(&w, &[&input]);
            let other_destination = work.with_consent_scope(if other_domain {
                consent_scope("realm-b/hosting", session.clone())
            } else {
                consent_scope("realm-a/hosting", SessionId::new())
            });

            w.review.open_consent_for_run(&other_destination, &run_id);
            assert!(is_parked(&w, &id), "another destination: {case}");
            w.review.close_consent_for_work(&other_destination);
            assert!(is_parked(&w, &id), "its close strands nothing: {case}");
            if !receipt_first {
                publish();
                assert!(
                    is_parked(&w, &id),
                    "another destination's close is not this consent's run: {case}"
                );
            }
            assert!(
                w.record(&id)
                    .consent
                    .and_then(|consent| consent.withdrawal)
                    .is_none(),
                "{case}"
            );

            let (exact, exact_run) = resumed(&w, &[&input]);
            w.review.open_consent_for_run(&exact, &exact_run);
            assert!(!is_parked(&w, &id), "{case}");
            assert_eq!(
                custody_of(&store, &id),
                Some(ApprovalConsentCustody::Transferred {
                    input_id: input.to_string()
                }),
                "{case}"
            );
            assert_eq!(spend_in(&w, &id, &exact, &exact_run), Ok(()), "{case}");
            assert_eq!(
                spend_in(&w, &id, &exact, &exact_run).err(),
                Some(crate::ToolError::ReviewUnsatisfied {
                    kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                    approval_id: None,
                }),
                "once: {case}"
            );
        }
    }

    /// A move stays unspendable through its preparation and its store
    /// commit. A proven no-commit keeps the consent parked, and the resumed
    /// run's close then withdraws what it never took over (nothing left
    /// unreachable). An unknown outcome retires the custody for
    /// reconciliation and nothing executes.
    #[tokio::test]
    async fn a_move_publishes_custody_only_after_its_commit_confirms() {
        for fault in [Fault::BeforeWrite, Fault::AfterWrite] {
            let store = Arc::new(FaultStore::default());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            let id = parked_approval(&w).await;
            w.review.close_consent_for_work(&w.work);
            let input = InputId::new();
            admit_notice(&w, &id, &input);
            let (resumed_work, run_id) = resumed(&w, &[&input]);
            let during_preparation = Arc::new(parking_lot::Mutex::new(None));
            {
                let (review, work, run, seen, id) = (
                    Arc::clone(&w.review),
                    resumed_work.clone(),
                    run_id.clone(),
                    Arc::clone(&during_preparation),
                    id.clone(),
                );
                *w.owner.on_prepare.lock() = Some(Box::new(move || {
                    *seen.lock() =
                        Some(admit_consent_attempt(Some(&review), &id, &work, Some(&run)).err());
                }));
            }
            *store.fault.lock() = Some(fault);

            w.review.open_consent_for_run(&resumed_work, &run_id);

            assert_eq!(
                *during_preparation.lock(),
                Some(Some(owner_unavailable())),
                "unspendable while the resumed run's owners prepare it"
            );
            match fault {
                Fault::BeforeWrite => {
                    assert!(is_parked(&w, &id), "a proven no-commit keeps it parked");
                    assert_eq!(
                        custody_of(&store.inner, &id),
                        Some(ApprovalConsentCustody::Parked)
                    );
                    assert_eq!(
                        attempt_in(&w, &id, &resumed_work, &run_id).err(),
                        Some(owner_unavailable())
                    );
                    w.review.close_consent_for_work(&resumed_work);
                    assert!(!is_parked(&w, &id));
                    let row = w.record(&id);
                    assert_eq!(
                        row.consent
                            .as_ref()
                            .and_then(|consent| consent.withdrawal.as_ref())
                            .map(|withdrawal| withdrawal.reason.clone()),
                        Some(ApprovalConsentWithdrawalReason::RunEnded),
                        "the close withdraws what its run never took over"
                    );
                    assert_eq!(
                        row.consent.and_then(|consent| consent
                            .delivery(ApprovalConsentTransition::Withdrawn)
                            .map(|delivery| delivery.state.clone())),
                        Some(ApprovalConsentDeliveryState::Owed)
                    );
                    assert_eq!(w.notices(&id).last(), Some(&ConsentNoticeKind::Withdrawn));
                }
                Fault::AfterWrite => {
                    assert!(!is_parked(&w, &id));
                    assert!(w.approvals.state.read().unreconciled.contains_key(&id));
                    assert_eq!(
                        attempt_in(&w, &id, &resumed_work, &run_id).err(),
                        Some(human_consent_required()),
                        "nothing executes after an uncertain move"
                    );
                    assert_eq!(
                        w.record(&id).consent.and_then(|consent| consent.custody),
                        Some(ApprovalConsentCustody::Transferred {
                            input_id: input.to_string()
                        }),
                        "reconciled from the store"
                    );
                    assert_eq!(
                        attempt_in(&w, &id, &resumed_work, &run_id).err(),
                        Some(human_consent_required())
                    );
                }
            }
        }
    }

    /// The resumed run's own owners decide the exact operation afresh: an
    /// actual refusal withdraws the consent with its own category; an
    /// unavailable owner leaves it parked until the run's close withdraws
    /// it.
    #[tokio::test]
    async fn a_refused_or_unavailable_move_settles_by_its_category() {
        for unavailable in [false, true] {
            let store = Arc::new(InMemoryApprovalStore::new());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            let id = parked_approval(&w).await;
            w.review.close_consent_for_work(&w.work);
            let input = InputId::new();
            admit_notice(&w, &id, &input);
            let (resumed_work, run_id) = resumed(&w, &[&input]);
            if unavailable {
                w.owner.prepare_unavailable.store(true, Ordering::Release);
            } else {
                w.owner.prepare_refused.store(true, Ordering::Release);
            }

            w.review.open_consent_for_run(&resumed_work, &run_id);

            let withdrawn = |w: &World| {
                w.record(&id)
                    .consent
                    .and_then(|consent| consent.withdrawal)
                    .map(|withdrawal| withdrawal.reason)
            };
            if unavailable {
                assert!(is_parked(&w, &id), "readiness retries, never settles");
                assert_eq!(withdrawn(&w), None);
                w.review.close_consent_for_work(&resumed_work);
                assert_eq!(
                    withdrawn(&w),
                    Some(ApprovalConsentWithdrawalReason::RunEnded)
                );
            } else {
                assert!(!is_parked(&w, &id));
                assert_eq!(
                    withdrawn(&w),
                    Some(ApprovalConsentWithdrawalReason::AuthorityChanged),
                    "an actual refusal settles here"
                );
            }
            assert_eq!(
                custody_of(&store, &id),
                Some(ApprovalConsentCustody::Parked)
            );
            assert_eq!(w.notices(&id).last(), Some(&ConsentNoticeKind::Withdrawn));
            assert_eq!(
                attempt_in(&w, &id, &resumed_work, &run_id).err(),
                Some(human_consent_required())
            );
        }
    }

    /// ADR-001 D9 at the move. The resumed run's own owners' read of the
    /// review material is taken over only for the account the approval
    /// presented. A mandate that now names the resumed run's requester is the
    /// moved consent's new baseline: an unrelated owner change after the move
    /// keeps it, and it enters once, while a change of that material after
    /// the move withdraws it as `ContextChanged`, with zero entry. An account
    /// its owners rebound while the consent was parked withdraws it at the
    /// move as `ContextChanged`, with zero entry.
    #[tokio::test]
    async fn a_move_takes_the_resumed_material_only_for_the_presented_account() {
        for case in ["mandate", "changed_after_the_move", "account"] {
            let store = Arc::new(InMemoryApprovalStore::new());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            w.owner.set_material(Some("team-checking"));
            w.owner
                .set_mandate(Some("on behalf of the closed run's requester"));
            let id = parked_approval(&w).await;
            w.review.close_consent_for_work(&w.work);
            let input = InputId::new();
            admit_notice(&w, &id, &input);
            let (resumed_work, run_id) = resumed(&w, &[&input]);
            // While parked.
            if case == "account" {
                w.owner.set_material(Some("team-savings"));
            } else {
                w.owner
                    .set_mandate(Some("on behalf of the resumed run's requester"));
            }

            w.review.open_consent_for_run(&resumed_work, &run_id);

            let withdrawn = |w: &World| withdrawal_reason(&w.record(&id));
            let spent = |w: &World| {
                w.record(&id)
                    .consent
                    .is_some_and(|consent| consent.consumed_at.is_some())
            };
            match case {
                "mandate" => {
                    assert!(!is_parked(&w, &id), "moved");
                    // An unrelated change after the move.
                    w.owner.republish();
                    assert_eq!(spend_in(&w, &id, &resumed_work, &run_id), Ok(()));
                    assert!(spent(&w), "entered once");
                    assert_eq!(withdrawn(&w), None);
                }
                "changed_after_the_move" => {
                    assert!(!is_parked(&w, &id), "moved");
                    w.owner.set_mandate(Some("on behalf of another requester"));
                    assert_eq!(
                        spend_in(&w, &id, &resumed_work, &run_id).err(),
                        Some(crate::ToolError::ReviewUnsatisfied {
                            kind: ReviewUnsatisfiedKind::ContextChanged,
                            approval_id: None,
                        })
                    );
                    assert!(!spent(&w), "zero entry");
                    assert_eq!(
                        withdrawn(&w),
                        Some(ApprovalConsentWithdrawalReason::ContextChanged)
                    );
                }
                _ => {
                    assert!(!is_parked(&w, &id), "settled at the move");
                    assert_eq!(
                        custody_of(&store, &id),
                        Some(ApprovalConsentCustody::Parked),
                        "never moved"
                    );
                    assert_eq!(
                        withdrawn(&w),
                        Some(ApprovalConsentWithdrawalReason::ContextChanged)
                    );
                    assert_eq!(
                        attempt_in(&w, &id, &resumed_work, &run_id).err(),
                        Some(human_consent_required())
                    );
                    assert!(!spent(&w), "zero entry");
                }
            }
        }
    }

    /// ADR-001 D9 at the move: the resumed run's owners' refusal of the
    /// review material settles the parked consent like their refusal of its
    /// operation, by its category (here `AuthorityChanged`), with zero entry.
    /// Owners that cannot read it now leave it parked: it never moves with
    /// the material it held, nothing resolves it meanwhile, and the resumed
    /// run's next boundary reads the material afresh, moves it, and it
    /// enters once.
    #[tokio::test]
    async fn a_failed_material_read_at_the_move_settles_like_a_refused_preparation() {
        for unavailable in [false, true] {
            let store = Arc::new(InMemoryApprovalStore::new());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            let id = parked_approval(&w).await;
            w.review.close_consent_for_work(&w.work);
            let input = InputId::new();
            admit_notice(&w, &id, &input);
            let (resumed_work, run_id) = resumed(&w, &[&input]);
            if unavailable {
                w.owner.material_unavailable.store(true, Ordering::Release);
            } else {
                w.owner.material_refused.store(true, Ordering::Release);
            }

            w.review.open_consent_for_run(&resumed_work, &run_id);

            assert_eq!(
                custody_of(&store, &id),
                Some(ApprovalConsentCustody::Parked),
                "never moved, unavailable={unavailable}"
            );
            if unavailable {
                assert!(is_parked(&w, &id), "still parked");
                assert_eq!(
                    attempt_in(&w, &id, &resumed_work, &run_id).err(),
                    Some(owner_unavailable()),
                    "not ready"
                );
                w.owner.material_unavailable.store(false, Ordering::Release);
                w.review
                    .retry_parked_consent_for_run(&resumed_work, &run_id);
                assert!(!is_parked(&w, &id), "moved at the run's next boundary");
                assert_eq!(spend_in(&w, &id, &resumed_work, &run_id), Ok(()));
                assert_eq!(withdrawal_reason(&w.record(&id)), None);
            } else {
                assert!(!is_parked(&w, &id));
                assert_eq!(
                    withdrawal_reason(&w.record(&id)),
                    Some(ApprovalConsentWithdrawalReason::AuthorityChanged)
                );
                assert_eq!(
                    attempt_in(&w, &id, &resumed_work, &run_id).err(),
                    Some(human_consent_required())
                );
                assert!(!w.notices(&id).contains(&ConsentNoticeKind::Used));
            }
        }
    }

    /// ADR-001 D9 at the move: an owner change in progress while the resumed
    /// run's owners prepare the operation or read its review material
    /// decides nothing. The consent stays parked, never withdrawn, and an
    /// attempt meanwhile is not ready; a boundary of the resumed run while
    /// the change is still in progress moves nothing, and once it completed
    /// the run's next boundary moves the consent and it enters once.
    #[tokio::test]
    async fn a_move_meeting_an_owner_change_stays_parked_until_a_later_boundary() {
        for during_material_read in [false, true] {
            let store = Arc::new(InMemoryApprovalStore::new());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            let id = parked_approval(&w).await;
            w.review.close_consent_for_work(&w.work);
            let input = InputId::new();
            admit_notice(&w, &id, &input);
            let (resumed_work, run_id) = resumed(&w, &[&input]);
            let in_change = if during_material_read {
                &w.owner.material_in_change
            } else {
                &w.owner.in_progress
            };
            in_change.store(true, Ordering::Release);

            w.review.open_consent_for_run(&resumed_work, &run_id);

            assert!(
                is_parked(&w, &id),
                "still parked, during_material_read={during_material_read}"
            );
            assert_eq!(
                custody_of(&store, &id),
                Some(ApprovalConsentCustody::Parked)
            );
            assert_eq!(withdrawal_reason(&w.record(&id)), None, "never withdrawn");
            assert_eq!(
                attempt_in(&w, &id, &resumed_work, &run_id).err(),
                Some(owner_unavailable()),
                "not ready"
            );
            w.review
                .retry_parked_consent_for_run(&resumed_work, &run_id);
            assert!(is_parked(&w, &id), "the change is still in progress");

            in_change.store(false, Ordering::Release);
            w.review
                .retry_parked_consent_for_run(&resumed_work, &run_id);
            assert!(!is_parked(&w, &id), "moved at the next boundary");
            assert_eq!(
                custody_of(&store, &id),
                Some(ApprovalConsentCustody::Transferred {
                    input_id: input.to_string()
                })
            );
            assert_eq!(spend_in(&w, &id, &resumed_work, &run_id), Ok(()));
            let record = w.record(&id);
            assert!(
                record
                    .consent
                    .as_ref()
                    .is_some_and(|consent| consent.consumed_at.is_some()),
                "entered once"
            );
            assert_eq!(withdrawal_reason(&record), None);
        }
    }

    /// ADR-001 D9 for the approver's eligibility after a move: the moved
    /// custody keeps the approver, the approval as decided and the evidence.
    /// An unrelated change of the eligibility facts before the move keeps
    /// the consent, and the resumed run's spend enters once. The approver
    /// revoked before the move is refused at the spend, which withdraws the
    /// consent as `AuthorityChanged`, with zero entry.
    #[tokio::test]
    async fn a_moved_consent_keeps_its_approvers_eligibility() {
        for revoked in [false, true] {
            let store = Arc::new(InMemoryApprovalStore::new());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            let id = parked_approval(&w).await;
            w.review.close_consent_for_work(&w.work);
            let input = InputId::new();
            admit_notice(&w, &id, &input);
            let (resumed_work, run_id) = resumed(&w, &[&input]);
            if revoked {
                w.route
                    .publish(|eligible| eligible.retain(|eligible| eligible != "lead-1"));
            } else {
                w.route
                    .publish(|eligible| eligible.push("lead-2".to_string()));
            }

            w.review.open_consent_for_run(&resumed_work, &run_id);

            assert!(!is_parked(&w, &id), "moved, revoked={revoked}");
            let spent = spend_in(&w, &id, &resumed_work, &run_id);
            let record = w.record(&id);
            if revoked {
                assert_eq!(spent.err(), Some(human_consent_required()));
                assert!(
                    record
                        .consent
                        .as_ref()
                        .is_some_and(|consent| consent.consumed_at.is_none()),
                    "zero entry"
                );
                assert_eq!(
                    withdrawal_reason(&record),
                    Some(ApprovalConsentWithdrawalReason::AuthorityChanged)
                );
            } else {
                assert_eq!(spent, Ok(()));
                assert!(
                    record
                        .consent
                        .as_ref()
                        .is_some_and(|consent| consent.consumed_at.is_some()),
                    "entered once"
                );
                assert_eq!(withdrawal_reason(&record), None);
            }
        }
    }

    /// One closed run can park several consents; each moves only by its own
    /// receipt, into the resumed run staged with that input, and a pending
    /// one (no decision, no receipt) moves nowhere. A moved consent its
    /// resumed run never spent is withdrawn at that run's close.
    #[tokio::test]
    async fn parked_consents_move_by_their_own_receipts() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let first = parked_approval(&w).await;
        let second = parked_approval(&w).await;
        let pending = w.request(&w.check()).await;
        w.review.close_consent_for_work(&w.work);
        let (first_input, second_input) = (InputId::new(), InputId::new());
        admit_notice(&w, &first, &first_input);
        admit_notice(&w, &second, &second_input);

        let (first_run_work, first_run) = resumed(&w, &[&first_input]);
        w.review.open_consent_for_run(&first_run_work, &first_run);
        assert!(!is_parked(&w, &first));
        assert!(is_parked(&w, &second) && is_parked(&w, &pending));

        let (second_run_work, second_run) = resumed(&w, &[&second_input]);
        w.review.open_consent_for_run(&second_run_work, &second_run);
        assert!(!is_parked(&w, &second));
        assert!(is_parked(&w, &pending), "a pending consent moves nowhere");

        assert_eq!(
            attempt_in(&w, &first, &second_run_work, &second_run).err(),
            Some(human_consent_required()),
            "each moved only into its own run"
        );
        assert_eq!(spend_in(&w, &second, &second_run_work, &second_run), Ok(()));

        w.review.close_consent_for_work(&first_run_work);
        assert_eq!(
            w.record(&first)
                .consent
                .and_then(|consent| consent.withdrawal)
                .map(|withdrawal| withdrawal.reason),
            Some(ApprovalConsentWithdrawalReason::RunEnded),
            "moved once: an unspent move is withdrawn at its run's close"
        );
        assert_eq!(
            custody_of(&store, &first),
            Some(ApprovalConsentCustody::Transferred {
                input_id: first_input.to_string()
            }),
            "custody history stays"
        );
        assert!(is_parked(&w, &pending));
    }

    /// The consent a resumed run's boundaries read as moved into it: only
    /// the consent whose transfer into that exact run the owner committed,
    /// in live, unexpired custody recorded as `Transferred` to one of the
    /// run's own staged inputs. Another run's consent, a run id the owner
    /// never opened for the work, a pending consent, and spent or expired
    /// consent are never named, and a held owner defers the read.
    #[tokio::test(start_paused = true)]
    async fn a_resumed_run_reads_only_the_consent_moved_into_it() {
        let mut w = world_with(
            ApprovalService::with_store(Arc::new(InMemoryApprovalStore::new())).expect("service"),
        );
        w.work = routed(w.work.clone());
        let first = parked_approval(&w).await;
        let second = parked_approval(&w).await;
        let pending = w.request(&w.check()).await;
        w.review.close_consent_for_work(&w.work);
        let (first_input, second_input) = (InputId::new(), InputId::new());
        admit_notice(&w, &first, &first_input);
        admit_notice(&w, &second, &second_input);
        let (first_work, first_run) = resumed(&w, &[&first_input]);
        let (second_work, second_run) = resumed(&w, &[&second_input]);
        let moved = |work: &WorkAuthorizationContext, run_id: &RunId| {
            w.approvals.consent_moved_into_run(work, Some(run_id))
        };
        assert_eq!(
            moved(&first_work, &first_run),
            MovedConsentScan::Moved(Vec::new()),
            "nothing moved before the run opened"
        );

        w.review.open_consent_for_run(&first_work, &first_run);
        w.review.open_consent_for_run(&second_work, &second_run);
        assert_eq!(
            moved(&first_work, &first_run),
            MovedConsentScan::Moved(vec![first.clone()])
        );
        assert_eq!(
            moved(&second_work, &second_run),
            MovedConsentScan::Moved(vec![second])
        );
        assert_eq!(
            moved(&first_work, &second_run),
            MovedConsentScan::Moved(Vec::new()),
            "a run id the owner never opened for this work reads nothing"
        );
        assert!(is_parked(&w, &pending), "a pending consent moves nowhere");
        {
            let _held = w.approvals.state.write();
            assert_eq!(
                moved(&first_work, &first_run),
                MovedConsentScan::Deferred,
                "a held owner defers the read"
            );
        }
        assert_eq!(
            ApprovalService::unavailable("closed")
                .consent_moved_into_run(&first_work, Some(&first_run)),
            MovedConsentScan::Moved(Vec::new()),
            "an unavailable owner holds no custody: nothing moved, never a deferral"
        );

        assert_eq!(spend_in(&w, &first, &first_work, &first_run), Ok(()));
        assert_eq!(
            moved(&first_work, &first_run),
            MovedConsentScan::Moved(Vec::new()),
            "spent consent is never named again"
        );
        tokio::time::advance(VALIDITY + Duration::from_secs(1)).await;
        assert_eq!(
            moved(&second_work, &second_run),
            MovedConsentScan::Moved(Vec::new()),
            "expired custody is never named"
        );
    }

    /// A receipt published through the owner while the resumed run staged
    /// with its input runs moves the consent into that run then: until then
    /// the attempt is not ready, never missing consent. Published after that
    /// run closed, the receipt is joined with the run's close: the consent is
    /// withdrawn explicitly (`RunEnded`), never transferred or spent.
    #[tokio::test]
    async fn a_receipt_committed_during_the_resumed_run_moves_its_consent_then() {
        for closed_first in [false, true] {
            let store = Arc::new(InMemoryApprovalStore::new());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            let session = destination(&w);
            let id = parked_approval(&w).await;
            w.review.close_consent_for_work(&w.work);
            let input = InputId::new();
            let (resumed_work, run_id) = resumed(&w, &[&input]);
            w.review.open_consent_for_run(&resumed_work, &run_id);
            assert!(is_parked(&w, &id));
            assert_eq!(
                attempt_in(&w, &id, &resumed_work, &run_id).err(),
                Some(owner_unavailable())
            );
            if closed_first {
                w.review.close_consent_for_work(&resumed_work);
                assert!(is_parked(&w, &id), "its receipt may still come");
            }

            w.approvals
                .settle_consent_delivery(
                    &id,
                    ApprovalConsentTransition::Approved,
                    admitted(&session, &input),
                )
                .expect("receipt settled");

            assert!(!is_parked(&w, &id), "closed_first={closed_first}");
            if !closed_first {
                assert_eq!(spend_in(&w, &id, &resumed_work, &run_id), Ok(()));
                continue;
            }
            assert_eq!(
                w.record(&id)
                    .consent
                    .and_then(|consent| consent.withdrawal)
                    .map(|withdrawal| withdrawal.reason),
                Some(ApprovalConsentWithdrawalReason::RunEnded),
                "settled explicitly at its run's close"
            );
            assert!(
                !matches!(
                    custody_of(&store, &id),
                    Some(ApprovalConsentCustody::Transferred { .. })
                ),
                "never transferred"
            );
            assert_eq!(
                attempt_in(&w, &id, &resumed_work, &run_id).err(),
                Some(human_consent_required()),
                "never spent"
            );
            assert_eq!(
                w.notices(&id)
                    .iter()
                    .filter(|kind| **kind == ConsentNoticeKind::Withdrawn)
                    .count(),
                1
            );
        }
    }

    /// A decision whose store write committed but whose acknowledgement was
    /// lost leaves the owner's memory behind the store. Discovery reconciles
    /// that row through the owner's existing path and finds the committed
    /// obligation, without an unrelated read, list or restart.
    #[tokio::test]
    async fn a_lost_commit_ack_obligation_stays_discoverable() {
        let store = Arc::new(FaultStore::default());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let id = w.request(&w.check()).await;
        *store.fault.lock() = Some(Fault::AfterWrite);
        assert_eq!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::StoreCommitUnknown {
                approval_id: id.clone()
            })
        );

        assert_eq!(
            w.approvals.owed_consent_deliveries(),
            Ok(OwedConsentDeliveries {
                owed: vec![(id, ApprovalConsentTransition::Approved)],
                unreconciled: Vec::new(),
                parked_notice_inputs: Vec::new(),
            })
        );
    }

    /// A withdrawal written only to the store (its own acknowledgement lost,
    /// no notice announced) owes its notice beside the decision's: discovery
    /// finds both through the owner's reconciliation.
    #[tokio::test]
    async fn a_withdrawal_written_only_to_its_store_is_discovered() {
        let store = Arc::new(FaultStore::default());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let id = w.request(&w.check()).await;
        // Revoked during the decision's own write; the withdrawal is the
        // second write, committed with its acknowledgement lost.
        let route = Arc::clone(&w.route);
        *store.hook.lock() = Some(Box::new(move || {
            route.publish(|eligible| eligible.retain(|id| id != "lead-1"));
        }));
        store.skip.store(1, Ordering::Release);
        *store.fault.lock() = Some(Fault::AfterWrite);
        assert_eq!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1")),
            Err(ApprovalError::Unreconciled {
                approval_id: id.clone()
            })
        );
        assert!(!w.notices(&id).contains(&ConsentNoticeKind::Withdrawn));

        let mut owed = w.approvals.owed_consent_deliveries().expect("available");
        owed.owed.sort();
        assert_eq!(
            owed,
            OwedConsentDeliveries {
                owed: vec![
                    (id.clone(), ApprovalConsentTransition::Approved),
                    (id, ApprovalConsentTransition::Withdrawn),
                ],
                unreconciled: Vec::new(),
                parked_notice_inputs: Vec::new(),
            }
        );
    }

    /// A row discovery cannot reconcile now is reported, never dropped as
    /// "nothing owed", and a later discovery finds its obligation.
    #[tokio::test]
    async fn discovery_reports_a_row_it_cannot_read_yet() {
        let store = Arc::new(FaultStore::default());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let id = w.request(&w.check()).await;
        *store.fault.lock() = Some(Fault::AfterWrite);
        assert!(
            w.approvals
                .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
                .is_err()
        );
        store.unreadable.store(true, Ordering::Release);
        assert_eq!(
            w.approvals.owed_consent_deliveries(),
            Ok(OwedConsentDeliveries {
                owed: Vec::new(),
                unreconciled: vec![id.clone()],
                parked_notice_inputs: Vec::new(),
            })
        );
        store.unreadable.store(false, Ordering::Release);
        assert_eq!(
            w.approvals.owed_consent_deliveries(),
            Ok(OwedConsentDeliveries {
                owed: vec![(id, ApprovalConsentTransition::Approved)],
                unreconciled: Vec::new(),
                parked_notice_inputs: Vec::new(),
            })
        );
    }

    /// An unavailable owner's discovery is an error, never "nothing owed".
    #[test]
    fn an_unavailable_owner_never_reports_nothing_owed() {
        assert!(matches!(
            ApprovalService::unavailable("closed").owed_consent_deliveries(),
            Err(ApprovalError::StoreUnavailable(_))
        ));
    }

    fn admitted(session_id: &SessionId, input_id: &InputId) -> ConsentDeliveryReceipt {
        ConsentDeliveryReceipt::Admitted {
            session_id: session_id.clone(),
            input_id: input_id.clone(),
        }
    }

    fn destination(w: &World) -> SessionId {
        w.work.consent_scope().expect("routed").session_id.clone()
    }

    /// A receipt settles exactly its own owed notice: an admission for
    /// another destination, a transition that never committed and an
    /// unrouted approval are refused; the same receipt again (a retry after
    /// a lost acknowledgement) changes nothing; a copied delivery's receipt
    /// or a later failure never replaces the first.
    #[tokio::test]
    async fn a_receipt_settles_only_its_own_owed_notice() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let session = destination(&w);
        let id = parked_approval(&w).await;
        let input = InputId::new();
        let approved = ApprovalConsentTransition::Approved;
        let settled = ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: input.to_string(),
        };

        assert_eq!(
            w.approvals
                .settle_consent_delivery(&id, approved, admitted(&SessionId::new(), &input)),
            Err(ConsentReceiptError::WrongDestination {
                approval_id: id.clone()
            })
        );
        assert_eq!(
            w.approvals.settle_consent_delivery(
                &id,
                ApprovalConsentTransition::Denied,
                admitted(&session, &input)
            ),
            Err(ConsentReceiptError::NotOwed {
                approval_id: id.clone(),
                transition: ApprovalConsentTransition::Denied,
            })
        );
        assert_eq!(
            w.approvals
                .settle_consent_delivery(&id, approved, admitted(&session, &input)),
            Ok(settled.clone())
        );
        assert_eq!(
            w.approvals
                .settle_consent_delivery(&id, approved, admitted(&session, &input)),
            Ok(settled.clone()),
            "the same receipt again changes nothing"
        );
        for other in [
            admitted(&session, &InputId::new()),
            ConsentDeliveryReceipt::Failed {
                reason: "refused".to_string(),
            },
        ] {
            assert_eq!(
                w.approvals.settle_consent_delivery(&id, approved, other),
                Err(ConsentReceiptError::Conflicting {
                    approval_id: id.clone(),
                    settled: settled.clone(),
                }),
                "the first receipt stands"
            );
        }
        assert_eq!(deliveries_of(&store, &id), vec![(approved, settled)]);
        assert_eq!(
            w.spend(&id),
            Ok(()),
            "a receipt is a delivery fact only: the live consent is untouched"
        );

        let unrouted = world();
        let plain = parked_approval(&unrouted).await;
        assert_eq!(
            unrouted.approvals.settle_consent_delivery(
                &plain,
                approved,
                admitted(&session, &input)
            ),
            Err(ConsentReceiptError::NotOwed {
                approval_id: plain,
                transition: approved,
            })
        );
    }

    /// A receipt committed for a resumed run already open moves the parked
    /// consent into it at once.
    #[tokio::test]
    async fn a_receipt_moves_parked_consent_into_its_open_run() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let session = destination(&w);
        let id = parked_approval(&w).await;
        w.review.close_consent_for_work(&w.work);
        let input = InputId::new();
        let (resumed_work, run_id) = resumed(&w, &[&input]);
        w.review.open_consent_for_run(&resumed_work, &run_id);
        assert!(is_parked(&w, &id));

        w.approvals
            .settle_consent_delivery(
                &id,
                ApprovalConsentTransition::Approved,
                admitted(&session, &input),
            )
            .expect("settled");

        assert!(!is_parked(&w, &id));
        assert_eq!(spend_in(&w, &id, &resumed_work, &run_id), Ok(()));
    }

    /// A late receipt is a delivery fact only: it never revives consent that
    /// was withdrawn meanwhile (here its validity lapsed while parked), and
    /// it never touches a fresh attempt's own approval.
    #[tokio::test]
    async fn a_late_receipt_never_revives_withdrawn_consent() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let session = destination(&w);
        let id = parked_approval(&w).await;
        w.review.close_consent_for_work(&w.work);
        w.approvals.clock_skew.store(
            i64::try_from(VALIDITY.as_secs()).expect("seconds") + 1,
            Ordering::SeqCst,
        );
        assert_eq!(
            w.record(&id)
                .consent
                .and_then(|consent| consent.withdrawal)
                .map(|withdrawal| withdrawal.reason),
            Some(ApprovalConsentWithdrawalReason::Expired),
            "an approved parked consent lapses on read"
        );
        assert!(!is_parked(&w, &id));
        w.approvals.clock_skew.store(0, Ordering::SeqCst);
        let fresh = w.request(&w.check()).await;
        let input = InputId::new();
        let (resumed_work, run_id) = resumed(&w, &[&input]);
        w.review.open_consent_for_run(&resumed_work, &run_id);

        assert!(
            w.approvals
                .settle_consent_delivery(
                    &id,
                    ApprovalConsentTransition::Approved,
                    admitted(&session, &input)
                )
                .is_ok(),
            "the late receipt settles its notice"
        );
        assert_eq!(
            attempt_in(&w, &id, &resumed_work, &run_id).err(),
            Some(human_consent_required()),
            "and revives nothing"
        );
        assert!(
            deliveries_of(&store, &fresh).is_empty(),
            "a fresh attempt's approval is untouched"
        );
        assert_eq!(
            w.approvals.settle_consent_delivery(
                &fresh,
                ApprovalConsentTransition::Approved,
                admitted(&session, &input)
            ),
            Err(ConsentReceiptError::NotOwed {
                approval_id: fresh.clone(),
                transition: ApprovalConsentTransition::Approved,
            })
        );
    }

    /// Parked consent its approved notice can no longer bring to a run is
    /// withdrawn, nothing left unreachable: a receipt naming an input whose
    /// only resumed run already closed, or a proven undeliverable notice.
    #[tokio::test]
    async fn an_unreachable_parked_consent_is_withdrawn_on_its_receipt() {
        for late in [true, false] {
            let store = Arc::new(InMemoryApprovalStore::new());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            let session = destination(&w);
            let id = parked_approval(&w).await;
            w.review.close_consent_for_work(&w.work);
            let input = InputId::new();
            let receipt = if late {
                let (resumed_work, run_id) = resumed(&w, &[&input]);
                w.review.open_consent_for_run(&resumed_work, &run_id);
                w.review.close_consent_for_work(&resumed_work);
                assert!(is_parked(&w, &id), "its receipt may still come");
                admitted(&session, &input)
            } else {
                ConsentDeliveryReceipt::Failed {
                    reason: "owner retired".to_string(),
                }
            };

            w.approvals
                .settle_consent_delivery(&id, ApprovalConsentTransition::Approved, receipt)
                .expect("settled");

            assert!(!is_parked(&w, &id), "late={late}");
            assert_eq!(
                w.record(&id)
                    .consent
                    .and_then(|consent| consent.withdrawal)
                    .map(|withdrawal| withdrawal.reason),
                Some(ApprovalConsentWithdrawalReason::RunEnded)
            );
            assert_eq!(w.notices(&id).last(), Some(&ConsentNoticeKind::Withdrawn));
        }
    }

    /// A receipt write that provably committed nothing can simply be retried,
    /// its custody intact. One whose outcome is unknown stops the consent
    /// being spendable at once and keeps its withdrawal owed: the retry
    /// reconciles the row first, finds the exact admitted receipt, and the
    /// confirmed row records the withdrawal with its own owed notice, once.
    /// When that withdrawal's own write fails too, before or after it
    /// committed, the row is unknown again and its reconciliation says so;
    /// the next retry settles the withdrawal, announced once. Nothing is
    /// entered, granted or resent.
    #[tokio::test]
    async fn a_receipt_retry_after_an_unknown_write_reconciles_first() {
        for (fault, settlement) in [
            (Fault::BeforeWrite, None),
            (Fault::AfterWrite, None),
            (Fault::AfterWrite, Some(Fault::BeforeWrite)),
            (Fault::AfterWrite, Some(Fault::AfterWrite)),
        ] {
            let store = Arc::new(FaultStore::default());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            let session = destination(&w);
            let id = parked_approval(&w).await;
            let input = InputId::new();
            let approved = ApprovalConsentTransition::Approved;
            *store.fault.lock() = Some(fault);

            let first =
                w.approvals
                    .settle_consent_delivery(&id, approved, admitted(&session, &input));
            match fault {
                Fault::BeforeWrite => assert!(matches!(
                    first,
                    Err(ConsentReceiptError::Approval(
                        ApprovalError::StoreUnavailable(_)
                    ))
                )),
                Fault::AfterWrite => assert_eq!(
                    first,
                    Err(ConsentReceiptError::Approval(
                        ApprovalError::StoreCommitUnknown {
                            approval_id: id.clone()
                        }
                    ))
                ),
            }
            let receipted = vec![(
                approved,
                ApprovalConsentDeliveryState::Admitted {
                    session_id: session.clone(),
                    input_id: input.to_string(),
                },
            )];
            if let Some(settlement) = settlement {
                // The owed withdrawal's own write fails too: the row is
                // unknown again, and its reconciliation reports that.
                *store.fault.lock() = Some(settlement);
                assert_eq!(
                    w.approvals.reconcile(&id),
                    Err(ApprovalError::Unreconciled {
                        approval_id: id.clone()
                    }),
                    "a failed settlement write leaves the row unknown"
                );
                let mut stored = receipted.clone();
                if let Fault::AfterWrite = settlement {
                    stored.push((
                        ApprovalConsentTransition::Withdrawn,
                        ApprovalConsentDeliveryState::Owed,
                    ));
                }
                assert_eq!(deliveries_of(&store.inner, &id), stored);
                assert!(
                    !w.notices(&id).contains(&ConsentNoticeKind::Withdrawn),
                    "nothing announced before the withdrawal is confirmed"
                );
            }
            assert_eq!(
                w.approvals
                    .settle_consent_delivery(&id, approved, admitted(&session, &input)),
                Ok(ApprovalConsentDeliveryState::Admitted {
                    session_id: session.clone(),
                    input_id: input.to_string(),
                }),
                "the retry settles exactly once"
            );
            let admitted_receipt = ApprovalConsentDeliveryState::Admitted {
                session_id: session.clone(),
                input_id: input.to_string(),
            };
            if let Fault::BeforeWrite = fault {
                assert_eq!(w.spend(&id), Ok(()), "nothing changed: the custody spends");
                continue;
            }
            assert_eq!(
                w.attempt(&id).err(),
                Some(human_consent_required()),
                "an uncertain write retired the custody: zero entry"
            );
            assert_eq!(
                w.record(&id)
                    .consent
                    .and_then(|consent| consent.withdrawal)
                    .map(|withdrawal| withdrawal.reason),
                Some(ApprovalConsentWithdrawalReason::Abandoned),
                "settled explicitly against the confirmed row"
            );
            let settled = vec![
                (approved, admitted_receipt.clone()),
                (
                    ApprovalConsentTransition::Withdrawn,
                    ApprovalConsentDeliveryState::Owed,
                ),
            ];
            assert_eq!(
                deliveries_of(&store.inner, &id),
                settled,
                "the actual receipt stays, and the withdrawal owes its own notice"
            );
            assert_eq!(
                w.approvals
                    .settle_consent_delivery(&id, approved, admitted(&session, &input)),
                Ok(admitted_receipt),
                "a later retry changes nothing"
            );
            assert_eq!(deliveries_of(&store.inner, &id), settled);
            assert_eq!(
                w.notices(&id)
                    .iter()
                    .filter(|kind| **kind == ConsentNoticeKind::Withdrawn)
                    .count(),
                1,
                "announced once"
            );
        }
    }

    /// The owner announces every committed decision, every recorded
    /// withdrawal, every recorded park and every new receipt to a delivery
    /// composition; an issuance or the same receipt again announces nothing.
    #[tokio::test]
    async fn obligations_and_parks_are_announced() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let session = destination(&w);
        let mut changes = w.approvals.subscribe_consent_deliveries();
        let mut announced = || {
            let changed = changes.has_changed().expect("open");
            changes.borrow_and_update();
            changed
        };

        let id = w.request(&w.check()).await;
        assert!(!announced(), "issuance owes nothing");
        w.approvals
            .decide_consent(&id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("approved");
        assert!(announced(), "a decision");
        w.review.close_consent_for_work(&w.work);
        assert!(announced(), "a park");
        let input = InputId::new();
        w.approvals
            .settle_consent_delivery(
                &id,
                ApprovalConsentTransition::Approved,
                admitted(&session, &input),
            )
            .expect("settled");
        assert!(announced(), "a new receipt");
        w.approvals
            .settle_consent_delivery(
                &id,
                ApprovalConsentTransition::Approved,
                admitted(&session, &input),
            )
            .expect("the same receipt");
        assert!(!announced(), "the same receipt again");
        let pending = w.request(&w.check()).await;
        let (resumed_work, run_id) = resumed(&w, &[&InputId::new()]);
        w.review.open_consent_for_run(&resumed_work, &run_id);
        w.approvals
            .decide_consent(&pending, decision(ApprovalDecision::Deny, "lead-1"))
            .expect("denied");
        assert!(announced(), "a denial");
        let withdrawn = w.request(&w.check()).await;
        // After an owner change the owners refuse the operation: the read's
        // re-adjudication withdraws the consent.
        w.owner.prepare_refused.store(true, Ordering::Release);
        w.owner.republish();
        let _ = w.record(&withdrawn);
        assert!(announced(), "a withdrawal");
    }

    /// The scope an owner-loss proof covers for `w`'s routed work.
    fn proof_scope(w: &World) -> ApprovalConsentScope {
        w.work.consent_scope().expect("routed").clone()
    }

    fn settled_row(outcome: Result<OwnerLossSettlement, ApprovalError>) -> ApprovalRecord {
        let record = match &outcome {
            Ok(OwnerLossSettlement::Settled(record)) => Some(record.clone()),
            _ => None,
        };
        assert!(record.is_some(), "expected a settled row, got {outcome:?}");
        record.expect("a settled row")
    }

    fn withdrawal_reason(record: &ApprovalRecord) -> Option<ApprovalConsentWithdrawalReason> {
        record
            .consent
            .as_ref()
            .and_then(|consent| consent.withdrawal.as_ref())
            .map(|withdrawal| withdrawal.reason.clone())
    }

    /// A second owner holding the proof settles a lost owner's rows, row
    /// only and in one conditional write each: a pending row is cancelled
    /// (never denied), also past its deadline, which is kept; approved
    /// history is kept; the withdrawal is `Abandoned` and its notice owed
    /// beside any earlier one. A settled row is settled once.
    #[tokio::test]
    async fn an_owner_loss_settles_pending_and_approved_history_owing_the_withdrawal() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut first = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        first.work = routed(first.work.clone());
        let scope = proof_scope(&first);
        let pending = first.request(&first.check()).await;
        let lapsed = first.request(&first.check()).await;
        let approved = parked_approval(&first).await;
        let deadline = store.get(&lapsed).expect("read").expect("row").expires_at;
        // The first owner's process is gone; a second owner of the store
        // holds the owner-loss proof for its session.
        let second = ApprovalService::with_store(store.clone()).expect("second owner");
        let owed = ApprovalConsentDeliveryState::Owed;

        let cancelled = settled_row(second.settle_lost_owner_consent(&pending, &scope));
        assert_eq!(
            cancelled.status,
            ApprovalStatus::Cancelled,
            "never a denial"
        );
        assert_eq!(
            withdrawal_reason(&cancelled),
            Some(ApprovalConsentWithdrawalReason::Abandoned)
        );
        assert_eq!(
            store.get(&pending).expect("read").expect("row"),
            cancelled,
            "one conditional write"
        );
        assert_eq!(
            deliveries_of(&store, &pending),
            vec![(ApprovalConsentTransition::Withdrawn, owed.clone())]
        );

        second.clock_skew.store(
            i64::try_from(VALIDITY.as_secs()).expect("seconds") + 1,
            Ordering::SeqCst,
        );
        let lapsed_row = settled_row(second.settle_lost_owner_consent(&lapsed, &scope));
        assert_eq!(
            lapsed_row.status,
            ApprovalStatus::Cancelled,
            "a pending row past its deadline is settled, never rewritten as expired"
        );
        assert_eq!(lapsed_row.expires_at, deadline, "its deadline is kept");
        second.clock_skew.store(0, Ordering::SeqCst);

        let kept = settled_row(second.settle_lost_owner_consent(&approved, &scope));
        assert_eq!(
            kept.status,
            ApprovalStatus::Approved,
            "approved history is kept"
        );
        assert!(kept.decision.is_some());
        assert_eq!(
            withdrawal_reason(&kept),
            Some(ApprovalConsentWithdrawalReason::Abandoned)
        );
        assert_eq!(
            deliveries_of(&store, &approved),
            vec![
                (ApprovalConsentTransition::Approved, owed.clone()),
                (ApprovalConsentTransition::Withdrawn, owed),
            ]
        );

        for id in [&pending, &lapsed, &approved] {
            assert_eq!(
                second.settle_lost_owner_consent(id, &scope),
                Ok(OwnerLossSettlement::Refused(OwnerLossRefusal::Settled)),
                "settled once"
            );
        }
    }

    /// An owner loss never writes what it must not: custody the live owner
    /// still holds, a row outside the proof's scope, a missing or
    /// non-consent row, spent, expired or denied history, and a row whose
    /// stored form is unknown.
    #[tokio::test]
    async fn an_owner_loss_refuses_what_it_must_not_write() {
        let store = Arc::new(FaultStore::default());
        let mut first = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        first.work = routed(first.work.clone());
        let scope = proof_scope(&first);
        let held = first.request(&first.check()).await;
        let spent = parked_approval(&first).await;
        assert_eq!(first.spend(&spent), Ok(()));
        let expired = first.request(&first.check()).await;
        first.approvals.clock_skew.store(
            i64::try_from(VALIDITY.as_secs()).expect("seconds") + 1,
            Ordering::SeqCst,
        );
        assert_eq!(first.record(&expired).status, ApprovalStatus::Expired);
        first.approvals.clock_skew.store(0, Ordering::SeqCst);
        let denied = first.request(&first.check()).await;
        first
            .approvals
            .decide_consent(&denied, decision(ApprovalDecision::Deny, "lead-1"))
            .expect("denied");
        let args = serde_json::value::RawValue::from_string("{}".into()).expect("args");
        let plain = first
            .approvals
            .request(
                consent_request(
                    ToolCallView {
                        id: "call-plain",
                        name: "send_message",
                        args: &args,
                    },
                    ConsentPresentation {
                        requester: ApprovalPrincipalId::new("human:child").expect("principal"),
                        risk: ApprovalRisk::High,
                        metadata: SurfaceMetadata::default(),
                    },
                    VALIDITY,
                )
                .expect("request"),
            )
            .expect("a plain approval")
            .approval_id;
        let refused = |refusal| Ok(OwnerLossSettlement::Refused(refusal));

        assert_eq!(
            first.approvals.settle_lost_owner_consent(&held, &scope),
            refused(OwnerLossRefusal::HeldHere),
            "the live owner holds its custody"
        );
        let second = ApprovalService::with_store(store.clone()).expect("second owner");
        let before: Vec<_> = [&held, &spent, &expired, &denied, &plain]
            .iter()
            .map(|id| store.get(id).expect("read"))
            .collect();
        for (id, refusal) in [
            (&spent, OwnerLossRefusal::Consumed),
            (&expired, OwnerLossRefusal::Expired),
            (&denied, OwnerLossRefusal::Settled),
            (&plain, OwnerLossRefusal::NotConsent),
        ] {
            assert_eq!(
                second.settle_lost_owner_consent(id, &scope),
                refused(refusal),
                "{refusal:?}"
            );
        }
        assert_eq!(
            second.settle_lost_owner_consent(
                &held,
                &consent_scope("realm-a/hosting", SessionId::new())
            ),
            refused(OwnerLossRefusal::OutOfScope),
            "the proof covers another scope"
        );
        assert_eq!(
            second.settle_lost_owner_consent(&ApprovalId::new(), &scope),
            refused(OwnerLossRefusal::NotFound)
        );
        let after: Vec<_> = [&held, &spent, &expired, &denied, &plain]
            .iter()
            .map(|id| store.get(id).expect("read"))
            .collect();
        assert_eq!(after, before, "a refusal writes nothing");

        *store.fault.lock() = Some(Fault::AfterWrite);
        assert_eq!(
            second.settle_lost_owner_consent(&held, &scope),
            Err(ApprovalError::StoreCommitUnknown {
                approval_id: held.clone()
            })
        );
        assert_eq!(
            second.settle_lost_owner_consent(&held, &scope),
            Err(ApprovalError::Unreconciled {
                approval_id: held.clone()
            }),
            "a row whose stored form is unknown stays untouched"
        );
    }

    /// Parked or transferred custody recorded on a row belonged to the
    /// lost owner's process: while that owner lives it holds both and
    /// refuses; once it is gone, a second owner holding the proof for the
    /// row's scope settles both like any unspent approved row and keeps the
    /// custody history. A proof for another scope settles nothing.
    #[tokio::test]
    async fn an_owner_loss_settles_parked_and_transferred_history_proof_bound() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut first = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        first.work = routed(first.work.clone());
        let scope = proof_scope(&first);
        let parked = parked_approval(&first).await;
        let moved = parked_approval(&first).await;
        first.review.close_consent_for_work(&first.work);
        let input = InputId::new();
        admit_notice(&first, &moved, &input);
        let (resumed_work, run_id) = resumed(&first, &[&input]);
        first.review.open_consent_for_run(&resumed_work, &run_id);
        let transferred = ApprovalConsentCustody::Transferred {
            input_id: input.to_string(),
        };
        assert_eq!(
            custody_of(&store, &parked),
            Some(ApprovalConsentCustody::Parked)
        );
        assert_eq!(custody_of(&store, &moved), Some(transferred.clone()));
        for id in [&parked, &moved] {
            assert_eq!(
                first.approvals.settle_lost_owner_consent(id, &scope),
                Ok(OwnerLossSettlement::Refused(OwnerLossRefusal::HeldHere)),
                "the live owner holds both custodies"
            );
        }

        let second = ApprovalService::with_store(store.clone()).expect("second owner");
        assert_eq!(
            second.settle_lost_owner_consent(
                &parked,
                &consent_scope("realm-a/hosting", SessionId::new())
            ),
            Ok(OwnerLossSettlement::Refused(OwnerLossRefusal::OutOfScope)),
            "proof-bound"
        );
        for (id, custody) in [
            (&parked, ApprovalConsentCustody::Parked),
            (&moved, transferred),
        ] {
            let row = settled_row(second.settle_lost_owner_consent(id, &scope));
            assert_eq!(row.status, ApprovalStatus::Approved);
            assert_eq!(
                row.consent
                    .as_ref()
                    .and_then(|consent| consent.custody.clone()),
                Some(custody),
                "custody history kept"
            );
            assert_eq!(
                withdrawal_reason(&row),
                Some(ApprovalConsentWithdrawalReason::Abandoned)
            );
            assert!(deliveries_of(&store, id).contains(&(
                ApprovalConsentTransition::Withdrawn,
                ApprovalConsentDeliveryState::Owed
            )));
        }
    }

    /// A parked consent whose admitted notice input ended before the move is
    /// withdrawn with a cause naming that input, recorded with its owed
    /// `Withdrawn` notice in one write. The earlier notice's receipt (or its
    /// still-owed state, for an input that ended before any receipt) stays
    /// exactly as it was, and nothing of the input's terminal is mirrored.
    /// When that write fails, before or after it committed, the whole
    /// withdrawal stays owed and nothing is announced; the owner's ordinary
    /// reconciliation then settles it once against the confirmed row (a
    /// committed write is recognised, never written again), with the same
    /// exact cause, receipt and single `Withdrawn` obligation. Nothing
    /// spends the consent meanwhile or after.
    #[tokio::test]
    async fn a_notice_input_that_ended_is_named_by_the_withdrawal() {
        for fault in [None, Some(Fault::BeforeWrite), Some(Fault::AfterWrite)] {
            for receipted in [true, false] {
                let case = format!("fault={fault:?} receipted={receipted}");
                let store = Arc::new(FaultStore::default());
                let mut w =
                    world_with(ApprovalService::with_store(store.clone()).expect("service"));
                w.work = routed(w.work.clone());
                let id = parked_approval(&w).await;
                w.review.close_consent_for_work(&w.work);
                let input = InputId::new();
                if receipted {
                    admit_notice(&w, &id, &input);
                }
                let approved_notice = deliveries_of(&store.inner, &id)
                    .into_iter()
                    .find(|(transition, _)| *transition == ApprovalConsentTransition::Approved)
                    .expect("the decision's notice");
                let settled = vec![
                    approved_notice,
                    (
                        ApprovalConsentTransition::Withdrawn,
                        ApprovalConsentDeliveryState::Owed,
                    ),
                ];
                let cause = ApprovalConsentWithdrawalReason::NoticeInputEnded {
                    input_id: input.to_string(),
                };
                *store.fault.lock() = fault;

                {
                    let mut state = w.approvals.state.write();
                    w.approvals.withdraw_consent_with_cause_locked(
                        &mut state,
                        &id,
                        ReviewRetirementReason::Abandoned,
                        cause.clone(),
                    );
                    w.approvals.release_state(state).run();
                }
                assert!(!is_parked(&w, &id), "{case}");
                assert!(
                    !w.approvals.state.read().consent.contains_key(&id),
                    "nothing can spend it: {case}"
                );
                if let Some(fault) = fault {
                    // Owed in full and announced to no one; the row is unknown.
                    assert_eq!(
                        owed_withdrawal_cause(&w, &id),
                        Some(cause.clone()),
                        "{case}"
                    );
                    assert!(
                        w.approvals.state.read().unreconciled.contains_key(&id),
                        "{case}"
                    );
                    assert!(
                        !w.notices(&id).contains(&ConsentNoticeKind::Withdrawn),
                        "{case}"
                    );
                    let written = store.get(&id).expect("read").expect("row");
                    assert_eq!(
                        withdrawal_reason(&written),
                        matches!(fault, Fault::AfterWrite).then(|| cause.clone()),
                        "{case}"
                    );
                    let reconciled = w.approvals.reconcile(&id).expect("settled");
                    if let Fault::AfterWrite = fault {
                        assert_eq!(reconciled, written, "never written again: {case}");
                    }
                    assert_eq!(owed_withdrawal_cause(&w, &id), None, "{case}");
                }
                let row = w.record(&id);
                assert_ne!(
                    withdrawal_reason(&row),
                    Some(ApprovalConsentWithdrawalReason::RunEnded),
                    "no resumed run took the input: never reported as a run end"
                );
                assert_eq!(withdrawal_reason(&row), Some(cause), "{case}");
                assert_eq!(row.status, ApprovalStatus::Approved, "{case}");
                assert_eq!(
                    deliveries_of(&store.inner, &id),
                    settled,
                    "the earlier notice is left as it was, one Withdrawn owed: {case}"
                );
                assert_eq!(
                    custody_of(&store.inner, &id),
                    Some(ApprovalConsentCustody::Parked),
                    "{case}"
                );
                assert_eq!(
                    w.notices(&id)
                        .iter()
                        .filter(|kind| **kind == ConsentNoticeKind::Withdrawn)
                        .count(),
                    1,
                    "announced once: {case}"
                );
                assert_eq!(
                    w.notices(&id).last(),
                    Some(&ConsentNoticeKind::Withdrawn),
                    "{case}"
                );
                assert_eq!(
                    w.attempt(&id).err(),
                    Some(human_consent_required()),
                    "{case}"
                );
            }
        }
    }

    /// `w`'s parked consent `id` with its approved notice admitted as a new
    /// input of its destination, through the owner's receipt.
    fn admit_parked_notice(w: &World, id: &ApprovalId) -> InputId {
        let input = InputId::new();
        w.approvals
            .settle_consent_delivery(
                id,
                ApprovalConsentTransition::Approved,
                admitted(&destination(w), &input),
            )
            .expect("receipt");
        input
    }

    fn receipt_of(
        w: &World,
        input: &InputId,
    ) -> (ApprovalConsentTransition, ApprovalConsentDeliveryState) {
        (
            ApprovalConsentTransition::Approved,
            ApprovalConsentDeliveryState::Admitted {
                session_id: destination(w),
                input_id: input.to_string(),
            },
        )
    }

    fn withdrawn_notices(w: &World, id: &ApprovalId) -> usize {
        w.notices(id)
            .iter()
            .filter(|kind| **kind == ConsentNoticeKind::Withdrawn)
            .count()
    }

    /// Discovery lists exactly the parked consent whose approved notice's
    /// receipt admitted an input into its own destination: a receipt taken
    /// while the run still held the consent counts from its park, and a
    /// notice still owed, an undeliverable one, consent a resumed run took
    /// over and spent consent are never listed.
    #[tokio::test]
    async fn discovery_lists_only_parked_consent_waiting_on_its_admitted_input() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let scope = proof_scope(&w);
        let approved = ApprovalConsentTransition::Approved;
        let owed = parked_approval(&w).await;
        let early = parked_approval(&w).await;
        let late = parked_approval(&w).await;
        let failed = parked_approval(&w).await;
        let moved = parked_approval(&w).await;
        let spent = parked_approval(&w).await;
        let early_input = admit_parked_notice(&w, &early);
        assert!(
            w.approvals
                .owed_consent_deliveries()
                .expect("discovery")
                .parked_notice_inputs
                .is_empty(),
            "its run still holds the consent"
        );
        assert_eq!(w.spend(&spent), Ok(()));
        w.review.close_consent_for_work(&w.work);
        let late_input = admit_parked_notice(&w, &late);
        w.approvals
            .settle_consent_delivery(
                &failed,
                approved,
                ConsentDeliveryReceipt::Failed {
                    reason: "owner retired".to_string(),
                },
            )
            .expect("failed receipt");
        let moved_input = admit_parked_notice(&w, &moved);
        let (resumed_work, run_id) = resumed(&w, &[&moved_input]);
        w.review.open_consent_for_run(&resumed_work, &run_id);
        assert!(!is_parked(&w, &moved), "the resumed run took it over");

        let mut expected = vec![
            ParkedNoticeInput {
                approval_id: early.clone(),
                scope: scope.clone(),
                input_id: early_input,
            },
            ParkedNoticeInput {
                approval_id: late.clone(),
                scope,
                input_id: late_input,
            },
        ];
        expected.sort_by(|left, right| left.approval_id.cmp(&right.approval_id));
        let discovered = w.approvals.owed_consent_deliveries().expect("discovery");
        assert_eq!(discovered.parked_notice_inputs, expected);
        assert!(discovered.owed.contains(&(owed, approved)));
        assert!(is_parked(&w, &early) && is_parked(&w, &late));
    }

    /// Parked consent whose admitted notice input ended is withdrawn once:
    /// its cause names the input, its `Withdrawn` notice is owed beside the
    /// kept receipt, and nothing can spend or move it. Only the exact parked
    /// row whose receipt names that input for the full destination settles:
    /// another input, the same session in another hosting domain and a
    /// repeat change nothing.
    #[tokio::test]
    async fn an_ended_notice_input_withdraws_its_parked_consent_once() {
        let store = Arc::new(InMemoryApprovalStore::new());
        let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
        w.work = routed(w.work.clone());
        let scope = proof_scope(&w);
        let id = parked_approval(&w).await;
        w.review.close_consent_for_work(&w.work);
        let input = admit_parked_notice(&w, &id);
        let receipt = receipt_of(&w, &input);
        let elsewhere = consent_scope("realm-b/hosting", destination(&w));

        for (other_scope, other_input) in [(&elsewhere, &input), (&scope, &InputId::new())] {
            assert_eq!(
                w.approvals
                    .settle_consent_notice_input_ended(&id, other_scope, other_input),
                Ok(NoticeInputEndedSettlement::NotWaiting)
            );
        }
        assert!(
            is_parked(&w, &id),
            "nothing waits on another input or domain"
        );
        assert_eq!(deliveries_of(&store, &id), vec![receipt.clone()]);

        assert_eq!(
            w.approvals
                .settle_consent_notice_input_ended(&id, &scope, &input),
            Ok(NoticeInputEndedSettlement::Withdrawn)
        );
        assert!(!is_parked(&w, &id));
        assert_eq!(
            withdrawal_reason(&w.record(&id)),
            Some(ApprovalConsentWithdrawalReason::NoticeInputEnded {
                input_id: input.to_string(),
            })
        );
        let settled = vec![
            receipt,
            (
                ApprovalConsentTransition::Withdrawn,
                ApprovalConsentDeliveryState::Owed,
            ),
        ];
        assert_eq!(
            deliveries_of(&store, &id),
            settled,
            "the receipt stays; the withdrawal owes its own notice"
        );
        assert_eq!(
            custody_of(&store, &id),
            Some(ApprovalConsentCustody::Parked),
            "custody history kept"
        );
        assert_eq!(w.attempt(&id).err(), Some(human_consent_required()));

        assert_eq!(
            w.approvals
                .settle_consent_notice_input_ended(&id, &scope, &input),
            Ok(NoticeInputEndedSettlement::NotWaiting),
            "settled once"
        );
        assert_eq!(deliveries_of(&store, &id), settled);
        let discovered = w.approvals.owed_consent_deliveries().expect("discovery");
        assert!(discovered.parked_notice_inputs.is_empty());
        assert!(
            discovered
                .owed
                .contains(&(id.clone(), ApprovalConsentTransition::Withdrawn))
        );
        let (resumed_work, run_id) = resumed(&w, &[&input]);
        w.review.open_consent_for_run(&resumed_work, &run_id);
        assert_eq!(
            attempt_in(&w, &id, &resumed_work, &run_id).err(),
            Some(human_consent_required()),
            "a run staged with the input later takes nothing over"
        );
        assert_eq!(withdrawn_notices(&w, &id), 1, "announced once");
    }

    /// A resumed run's move and an ended notice input's settlement have one
    /// winner, whichever comes first: consent the run took over is never
    /// withdrawn for its input, and consent withdrawn while the run was
    /// preparing the move (outside every owner lock) never moves.
    #[tokio::test]
    async fn a_resumed_run_and_an_ended_notice_input_have_one_winner() {
        for moved_first in [true, false] {
            let store = Arc::new(InMemoryApprovalStore::new());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            let scope = proof_scope(&w);
            let id = parked_approval(&w).await;
            w.review.close_consent_for_work(&w.work);
            let input = admit_parked_notice(&w, &id);
            let (resumed_work, run_id) = resumed(&w, &[&input]);
            if moved_first {
                w.review.open_consent_for_run(&resumed_work, &run_id);
                assert_eq!(
                    custody_of(&store, &id),
                    Some(ApprovalConsentCustody::Transferred {
                        input_id: input.to_string(),
                    })
                );
                assert_eq!(
                    w.approvals
                        .settle_consent_notice_input_ended(&id, &scope, &input),
                    Ok(NoticeInputEndedSettlement::NotWaiting),
                    "the run owns the consent now"
                );
                assert_eq!(withdrawal_reason(&w.record(&id)), None);
                assert_eq!(spend_in(&w, &id, &resumed_work, &run_id), Ok(()));
                continue;
            }
            let settled = Arc::new(parking_lot::Mutex::new(None));
            let during = {
                let approvals = w.approvals.clone();
                let (id, scope, input, settled) = (
                    id.clone(),
                    scope.clone(),
                    input.clone(),
                    Arc::clone(&settled),
                );
                move || {
                    *settled.lock() =
                        Some(approvals.settle_consent_notice_input_ended(&id, &scope, &input));
                }
            };
            *w.owner.on_prepare.lock() = Some(Box::new(during));
            w.review.open_consent_for_run(&resumed_work, &run_id);
            assert_eq!(
                *settled.lock(),
                Some(Ok(NoticeInputEndedSettlement::Withdrawn)),
                "settled while the move was being prepared"
            );
            assert_eq!(
                custody_of(&store, &id),
                Some(ApprovalConsentCustody::Parked),
                "the move committed nothing"
            );
            assert_eq!(
                withdrawal_reason(&w.record(&id)),
                Some(ApprovalConsentWithdrawalReason::NoticeInputEnded {
                    input_id: input.to_string(),
                })
            );
            assert_eq!(
                attempt_in(&w, &id, &resumed_work, &run_id).err(),
                Some(human_consent_required())
            );
            assert_eq!(withdrawn_notices(&w, &id), 1);
        }
    }

    /// An ended notice input's withdrawal whose own write fails, before or
    /// after it committed, retires the custody at once and keeps the
    /// withdrawal owed with its cause: the row is unknown, a discovery that
    /// cannot read it yet reports it, and the next discovery's ordinary
    /// reconciliation records the one withdrawal and its owed notice, the
    /// admitted receipt kept. Nothing is announced before it is confirmed,
    /// and nothing is admitted, spent or moved again.
    #[tokio::test]
    async fn an_uncertain_notice_input_withdrawal_is_reconciled_once() {
        for fault in [Fault::BeforeWrite, Fault::AfterWrite] {
            let store = Arc::new(FaultStore::default());
            let mut w = world_with(ApprovalService::with_store(store.clone()).expect("service"));
            w.work = routed(w.work.clone());
            let scope = proof_scope(&w);
            let id = parked_approval(&w).await;
            w.review.close_consent_for_work(&w.work);
            let input = admit_parked_notice(&w, &id);
            let receipt = receipt_of(&w, &input);
            let cause = ApprovalConsentWithdrawalReason::NoticeInputEnded {
                input_id: input.to_string(),
            };
            let owed_withdrawal = (
                ApprovalConsentTransition::Withdrawn,
                ApprovalConsentDeliveryState::Owed,
            );

            *store.fault.lock() = Some(fault);
            assert_eq!(
                w.approvals
                    .settle_consent_notice_input_ended(&id, &scope, &input),
                Err(ApprovalError::Unreconciled {
                    approval_id: id.clone()
                })
            );
            assert!(!is_parked(&w, &id));
            assert_eq!(
                w.attempt(&id).err(),
                Some(human_consent_required()),
                "the custody is retired at once"
            );
            let mut stored = vec![receipt.clone()];
            if let Fault::AfterWrite = fault {
                stored.push(owed_withdrawal.clone());
            }
            assert_eq!(deliveries_of(&store.inner, &id), stored);
            assert_eq!(withdrawn_notices(&w, &id), 0, "nothing announced yet");

            store.unreadable.store(true, Ordering::Release);
            let unread = w.approvals.owed_consent_deliveries().expect("discovery");
            assert_eq!(
                unread.unreconciled,
                vec![id.clone()],
                "reported, never dropped"
            );
            assert!(unread.parked_notice_inputs.is_empty());
            store.unreadable.store(false, Ordering::Release);

            let discovered = w.approvals.owed_consent_deliveries().expect("discovery");
            assert!(discovered.unreconciled.is_empty());
            assert!(discovered.parked_notice_inputs.is_empty());
            assert!(
                discovered
                    .owed
                    .contains(&(id.clone(), ApprovalConsentTransition::Withdrawn))
            );
            assert_eq!(withdrawal_reason(&w.record(&id)), Some(cause.clone()));
            let settled = vec![receipt, owed_withdrawal];
            assert_eq!(
                deliveries_of(&store.inner, &id),
                settled,
                "one withdrawal; the admitted receipt kept"
            );
            assert_eq!(withdrawn_notices(&w, &id), 1);

            assert_eq!(
                w.approvals
                    .settle_consent_notice_input_ended(&id, &scope, &input),
                Ok(NoticeInputEndedSettlement::NotWaiting)
            );
            assert_eq!(
                w.approvals
                    .reconcile(&id)
                    .map(|row| withdrawal_reason(&row)),
                Ok(Some(cause))
            );
            assert_eq!(deliveries_of(&store.inner, &id), settled);
            let (resumed_work, run_id) = resumed(&w, &[&input]);
            w.review.open_consent_for_run(&resumed_work, &run_id);
            assert_eq!(
                attempt_in(&w, &id, &resumed_work, &run_id).err(),
                Some(human_consent_required()),
                "never re-admitted"
            );
            assert_eq!(withdrawn_notices(&w, &id), 1, "announced once");
        }
    }

    /// The notice-input cause is stored as its own typed variant beside the
    /// existing string causes, which keep their stored form.
    #[test]
    fn the_notice_input_cause_is_stored_typed() {
        let cause = ApprovalConsentWithdrawalReason::NoticeInputEnded {
            input_id: "input-1".to_string(),
        };
        let stored = serde_json::to_value(&cause).expect("serialize");
        assert_eq!(
            stored,
            serde_json::json!({"notice_input_ended": {"input_id": "input-1"}})
        );
        assert_eq!(
            serde_json::from_value::<ApprovalConsentWithdrawalReason>(stored).expect("deserialize"),
            cause
        );
        assert_eq!(
            serde_json::from_value::<ApprovalConsentWithdrawalReason>(serde_json::json!(
                "run_ended"
            ))
            .expect("deserialize"),
            ApprovalConsentWithdrawalReason::RunEnded,
            "existing causes keep their stored form"
        );
    }

    /// The stored delivery fields are compatible both ways: a routed row
    /// with obligations and custody history round-trips exactly, and a row
    /// written before them restores with none and stores none.
    #[test]
    fn the_stored_consent_delivery_is_compatible() {
        let now = Utc::now();
        let consent = ApprovalConsentRecord {
            scope: Some(consent_scope("realm-a/hosting", SessionId::new())),
            origin: Some(ApprovalConsentOrigin {
                retained_work: origin_work("runtime-a"),
            }),
            deliveries: vec![
                ApprovalConsentDelivery {
                    transition: ApprovalConsentTransition::Approved,
                    committed_at: now,
                    state: ApprovalConsentDeliveryState::Admitted {
                        session_id: SessionId::new(),
                        input_id: "input-1".to_string(),
                    },
                },
                ApprovalConsentDelivery {
                    transition: ApprovalConsentTransition::Withdrawn,
                    committed_at: now,
                    state: ApprovalConsentDeliveryState::Owed,
                },
            ],
            custody: Some(ApprovalConsentCustody::Transferred {
                input_id: "input-1".to_string(),
            }),
            ..ApprovalConsentRecord::default()
        };
        let stored = serde_json::to_value(&consent).expect("serialize");
        assert_eq!(stored["deliveries"][0]["state"]["status"], "admitted");
        assert_eq!(stored["deliveries"][1]["transition"], "withdrawn");
        assert_eq!(stored["custody"]["state"], "transferred");
        assert_eq!(
            serde_json::from_value::<ApprovalConsentRecord>(stored).expect("deserialize"),
            consent,
            "a routed row round-trips exactly"
        );
        let predating = serde_json::json!({});
        assert_eq!(
            serde_json::from_value::<ApprovalConsentRecord>(predating).expect("deserialize"),
            ApprovalConsentRecord::default()
        );
        let stored_default =
            serde_json::to_value(ApprovalConsentRecord::default()).expect("serialize");
        for field in ["origin", "deliveries", "custody"] {
            assert!(stored_default.get(field).is_none(), "{field} is not stored");
        }
    }

    /// A consent notice's job id names exactly one approval and transition.
    #[test]
    fn a_consent_delivery_job_id_names_one_transition() {
        let id = ApprovalId::new();
        for transition in [
            ApprovalConsentTransition::Approved,
            ApprovalConsentTransition::Denied,
            ApprovalConsentTransition::Withdrawn,
        ] {
            let job_id = consent_delivery_job_id(&id, transition);
            assert_eq!(
                parse_consent_delivery_job_id(&job_id),
                Some((id.clone(), transition))
            );
        }
        for invalid in [
            "approved",
            "not-an-id/approved",
            &format!("{id}/used"),
            &format!("{id}/approved/extra"),
            &format!("{id}/Approved"),
        ] {
            assert_eq!(parse_consent_delivery_job_id(invalid), None, "{invalid}");
        }
    }

    /// The host eligibility owner resolves the exact login of the decision
    /// event. An event from a revoked login of a person is refused while a
    /// live login of the same person decides; a login revoked between
    /// resolution and commit refuses with nothing committed or notified. The
    /// event entry binds the approval, the decision and that login.
    #[tokio::test]
    async fn eligibility_resolves_the_exact_login_of_the_decision_event() {
        let w = world();
        let id = w.request(&w.check()).await;
        let event = |login: &str| {
            AuthenticatedConsentEvent::new(
                id.clone(),
                AuthenticatedConsentDecision {
                    approver: approver("lead-1")
                        .with_authentication_scope(HostAuthenticationScope::new(login)),
                    decision: ApprovalDecision::Approve,
                    reason: None,
                },
            )
        };
        let not_eligible = || {
            Err(ApprovalError::NotEligible {
                approval_id: id.clone(),
            })
        };

        w.route.revoked_logins.lock().push("login-a".to_owned());
        assert_eq!(
            w.approvals.decide_consent_event(event("login-a")),
            not_eligible(),
            "a revoked login of an eligible person is not eligible"
        );
        assert_eq!(w.record(&id).status, ApprovalStatus::Pending);

        *w.route.revoke_login_after_resolve.lock() = Some("login-b".to_owned());
        assert_eq!(
            w.approvals.decide_consent_event(event("login-b")),
            not_eligible(),
            "revoked between resolution and commit"
        );
        assert_eq!(w.record(&id).status, ApprovalStatus::Pending);
        assert_eq!(w.notices(&id), vec![ConsentNoticeKind::Requested]);

        let decided = w
            .approvals
            .decide_consent_event(event("login-c"))
            .expect("a live login of the same person decides");
        assert_eq!(decided.status, ApprovalStatus::Approved);
        assert_eq!(w.spend(&id), Ok(()));
    }
}
