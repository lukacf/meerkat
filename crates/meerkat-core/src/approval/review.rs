//! Retained model review for one exact governed native operation (ADR-001).
//!
//! The required review tier is native authorization truth
//! ([`crate::authorization::OperationReviewTier`]): the policy owner resolves
//! it with the operation's permission and the exact prepared decision retains
//! it. Review never grants a permission that authorization did not grant.
//!
//! The single generated `ApprovalLifecycleMachine` owns review attempt state.
//! Attempts are process-local and memory-only: never written to an
//! `ApprovalStore`, never restored. A historical approval record therefore
//! cannot reconstruct a review allow.
//!
//! R3 with a host human route retains one human attempt for the exact
//! candidate and settles locally; an authenticated decision and a fresh
//! explicit attempt spend it once (see [`super::consent`]). Without a route,
//! R3 settles locally as before. With a route, an R2 escalation or an
//! expired R2 deadline settles and retires the model attempt (keeping its
//! own outcome) and then requests consent from the same human owner; an
//! ordinary reviewer failure stays local feedback, because it cannot yet be
//! told apart from a protected observation failure, which must never reach
//! a human. Closed batches are not implemented.
//! The reviewer receives the exact call, authorization facts and retained work
//! owner through [`ReviewCandidate`]. Its optional `read_review_context` method
//! supplies independently source-authorized original input material; unsupported
//! owners return unavailable. Installing a reviewer alone does not establish
//! complete original input, selected account or current mandate context. Such
//! facts must come from native owners, never the transcript or model claims.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[cfg(target_arch = "wasm32")]
use crate::tokio;

use crate::ToolCallView;
use crate::approval::consent::{
    ConsentCandidate, ConsentCandidateParts, ConsentNoticeKind, ConsentPresentationCandidate,
    ConsentProjections, HumanConsent, HumanConsentRoute, SpendRechecks, consent_request,
};
use crate::approval::{ApprovalId, RefusedReviewCommit, SettledReports};
use crate::authorization::{
    OperationAuthorizationError, OperationAuthorizationFacts, OperationObservationError,
    OwnerChangeChurn, PolicyPublicationObservation, PreparedAuthorizationBinding,
    PreparedOperationCheck, ReviewOperationContext, ReviewOperationContextResolution,
    WorkAuthorizationContext,
};
use crate::lifecycle::identifiers::RunId;

mod context;
pub use context::{MAX_REVIEW_CONTEXT_BYTES, ReviewContextFuture, ReviewContextMaterial};
mod attribution;
pub use attribution::{ReviewChildAttribution, ReviewOperationAttribution, ReviewOperationRole};

pub use crate::authorization::OperationReviewTier;
pub use crate::generated::approval_lifecycle::{
    ReviewAttemptStatus, ReviewRetirementReason, ReviewVerdict,
};

/// Audience-safe reason an otherwise permitted operation lacks a satisfied
/// review. No reviewer rationale or private policy fact is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewUnsatisfiedKind {
    /// The bound reviewer refused this exact candidate.
    Denied,
    /// The reviewer escalated; no qualified human route settles it here.
    Escalated,
    /// Authorization was re-prepared while the review was retained, so the
    /// reviewed context is no longer current. A fresh attempt is reviewed anew.
    /// When only the owner publication moved (the same operation is still
    /// permitted at R2, with the same review material) before any dispatcher
    /// code ran, the fenced dispatch reviews it once more itself, as a second
    /// attempt for the same tool call; the first attempt's retirement is then
    /// not the call's outcome.
    ContextChanged,
    /// R3 requires a fresh qualified human decision; a model allow cannot
    /// satisfy it.
    HumanConsentRequired,
    /// The review allow for this operation was already spent at its entry.
    AlreadyConsumed,
}

impl fmt::Display for ReviewUnsatisfiedKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Denied => "operation review refused this action",
            Self::Escalated => "operation review escalated this action",
            Self::ContextChanged => "operation review is no longer current",
            Self::HumanConsentRequired => "this action requires human consent",
            Self::AlreadyConsumed => "operation review was already used for this action",
        })
    }
}

/// Infrastructure reason required review could not be completed. Distinct
/// from a refusal; the operation never runs unchecked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewUnavailableKind {
    /// The finite owner deadline elapsed and the attempt was retired.
    DeadlineExpired,
    /// The bound reviewer failed to produce a verdict.
    ReviewerFailed,
    /// Authorization requires review but this composition installs no reviewer.
    ReviewerMissing,
    /// The dispatcher cannot carry review to this tool's physical entry, so a
    /// required review cannot be honored. The tool never runs unchecked.
    UnsupportedEntry,
    /// The review attempt owner could not record the attempt.
    OwnerUnavailable,
    /// The dispatch that admitted this review already ended (cancelled,
    /// timed out or settled); a retained clone can no longer enter.
    DispatchEnded,
    /// Another owner of the approval store changed the consent first; the
    /// retained consent was retired and the approval is reconciled.
    StoreConflict,
    /// The approval store cannot confirm whether the consent spend
    /// committed. Nothing entered and nothing is refunded.
    StoreCommitUnknown,
    /// The owner requires review material for this operation (its selected
    /// action account or current mandate) and could not supply it. No review
    /// attempt or approval exists and nothing entered.
    ContextUnavailable,
}

impl fmt::Display for ReviewUnavailableKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DeadlineExpired => "operation review deadline expired",
            Self::ReviewerFailed => "operation reviewer failed",
            Self::ReviewerMissing => "operation reviewer is not installed",
            Self::UnsupportedEntry => "this tool cannot honor required operation review",
            Self::OwnerUnavailable => "operation review owner unavailable",
            Self::DispatchEnded => "the operation's dispatch already ended",
            Self::StoreConflict => "the approval changed elsewhere",
            Self::StoreCommitUnknown => "the approval store could not confirm the commit",
            Self::ContextUnavailable => "the operation's review context is unavailable",
        })
    }
}

/// Typed local settlement of a required review at an entry. It is review
/// feedback, never a permission refusal or an observation failure: the
/// operation was otherwise permitted and current, and it did not enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "review", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationReviewRefusal {
    #[error("{kind}")]
    Unsatisfied { kind: ReviewUnsatisfiedKind },
    #[error("{kind}")]
    Unavailable { kind: ReviewUnavailableKind },
}

impl OperationReviewRefusal {
    /// Stable wire code shared with the tool feedback codes.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsatisfied { .. } => "REVIEW_UNSATISFIED",
            Self::Unavailable { .. } => "REVIEW_UNAVAILABLE",
        }
    }

    /// The exact R1-only settlement for an entry that has no review seam.
    /// R2 cannot be honored there (`UnsupportedEntry`), and no model allow
    /// can satisfy R3 (`HumanConsentRequired`); neither enters.
    pub fn for_unreviewed_entry(tier: OperationReviewTier) -> Result<(), Self> {
        match tier {
            OperationReviewTier::R1 => Ok(()),
            OperationReviewTier::R2 => Err(Self::Unavailable {
                kind: ReviewUnavailableKind::UnsupportedEntry,
            }),
            OperationReviewTier::R3 => Err(Self::Unsatisfied {
                kind: ReviewUnsatisfiedKind::HumanConsentRequired,
            }),
        }
    }
}

impl From<OperationReviewRefusal> for crate::ToolError {
    fn from(refusal: OperationReviewRefusal) -> Self {
        match refusal {
            OperationReviewRefusal::Unsatisfied { kind } => Self::ReviewUnsatisfied {
                kind,
                approval_id: None,
            },
            OperationReviewRefusal::Unavailable { kind } => Self::ReviewUnavailable { kind },
        }
    }
}

/// An unavailable review is local feedback; a failed protected observation
/// retains the engine's infrastructure failure and deferred-sibling behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReviewerFailure {
    #[error("operation reviewer failed")]
    Unavailable,
    #[error("operation observation unavailable")]
    ObservationUnavailable(#[from] OperationObservationError),
}

impl From<crate::authorization::OperationAuthorizationError> for ReviewerFailure {
    fn from(error: crate::authorization::OperationAuthorizationError) -> Self {
        match error {
            crate::authorization::OperationAuthorizationError::ObservationUnavailable(error) => {
                Self::ObservationUnavailable(error)
            }
            crate::authorization::OperationAuthorizationError::Unavailable
            | crate::authorization::OperationAuthorizationError::Refused(_) => Self::Unavailable,
        }
    }
}

/// The exact native candidate under review: the binding-owned call and the
/// native owner's admitted operation facts. Model statements and forwarded
/// text cannot add identity, account or work facts.
pub struct ReviewCandidate<'a> {
    pub(super) call: ToolCallView<'a>,
    pub(super) check: &'a PreparedOperationCheck,
    attribution: ReviewOperationAttribution,
    operation_context: Option<&'a ReviewOperationContext>,
}

impl<'a> ReviewCandidate<'a> {
    /// The owners' passive material for this operation (selected action
    /// account, current mandate), read once for this attempt. `None` when the
    /// owners stated it is not applicable. It grants nothing.
    pub fn operation_context(&self) -> Option<&'a ReviewOperationContext> {
        self.operation_context
    }

    /// Exact origin for this attempt's source reads and reviewer inference.
    /// This diagnostic association never grants either operation permission.
    pub fn attribution(&self) -> &ReviewOperationAttribution {
        &self.attribution
    }

    pub fn tool_name(&self) -> &'a str {
        self.call.name
    }

    pub fn tool_call_id(&self) -> &'a str {
        self.call.id
    }

    pub fn arguments(&self) -> &'a serde_json::value::RawValue {
        self.call.args
    }

    pub fn facts(&self) -> &'a OperationAuthorizationFacts {
        self.check.binding().facts()
    }

    /// The exact native operation retained by this candidate. Equal facts or
    /// serialized arguments cannot reconstruct this binding. It is not an
    /// allowance for a reviewer model request or a context-source read.
    pub fn binding(&self) -> &'a PreparedAuthorizationBinding {
        self.check.binding()
    }

    /// The actual work owner retained by the prepared check, without looking
    /// it up again by session or run coordinates. A reviewer uses this owner
    /// for its own governed operations; the reviewed tool's permission never
    /// grants a model route, processor or source read.
    ///
    /// This does not provide authenticated request content, account facts or
    /// mandates. Those still require an owner-authorized native context read;
    /// an adapter must report unavailable when it cannot obtain that context.
    pub fn work_authorization(&self) -> &'a WorkAuthorizationContext {
        self.check.work_authorization()
    }

    /// Historical attribution of the preparation under review. Never a
    /// currentness proof; staleness is decided only by current owners.
    pub fn policy_observation(&self) -> Option<PolicyPublicationObservation> {
        self.check.policy_observation()
    }
}

impl fmt::Debug for ReviewCandidate<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReviewCandidate([REDACTED])")
    }
}

/// Restricted reviewer judging one retained candidate once.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait OperationReviewer: Send + Sync {
    async fn review(
        &self,
        candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure>;
}

/// Audience-safe owner reference of one review attempt. It is not an
/// authority handle: it cannot be converted back into an attempt, decide it
/// or authorize entry.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ReviewAttemptRef(Arc<str>);

impl fmt::Debug for ReviewAttemptRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ReviewAttemptRef({})", self.0)
    }
}

impl fmt::Display for ReviewAttemptRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Projection of one generated review transition. Projections never
/// authorize entry. The `Used` projection is delivered only after the effect.
///
/// One tool call can have two attempts: an attempt retired as
/// `ContextChanged` before any dispatcher code ran can be followed by a
/// second attempt (another `attempt`, the same `tool_call_id`) when the
/// fenced dispatch reviews the operation once more under its current
/// decision. Only the last attempt's projections settle the call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewObservation {
    pub attempt: ReviewAttemptRef,
    pub tool_call_id: Arc<str>,
    pub status: ReviewAttemptStatus,
    pub retirement: Option<ReviewRetirementReason>,
}

pub trait OperationReviewObserver: Send + Sync {
    fn observe(&self, observation: ReviewObservation);
}

/// Owner-issued attempt handle bound to the exact operation binding and the
/// original admitted work. Not cloneable, not serializable, not constructible
/// outside the approval owner; the raw id never leaves core. Only the
/// approval owner duplicates one (`twin`), for the entered state of a spent
/// attempt.
pub(crate) struct ReviewAttemptHandle {
    id: Arc<str>,
    binding: PreparedAuthorizationBinding,
    work: WorkAuthorizationContext,
}

impl ReviewAttemptHandle {
    pub(super) fn new(
        id: Arc<str>,
        binding: PreparedAuthorizationBinding,
        work: WorkAuthorizationContext,
    ) -> Self {
        Self { id, binding, work }
    }

    pub(super) fn id(&self) -> &str {
        &self.id
    }

    /// Identity, never equality: equal-looking facts or a copied id from
    /// another operation or work association do not match.
    pub(super) fn bound_to(
        &self,
        binding: &PreparedAuthorizationBinding,
        work: &WorkAuthorizationContext,
    ) -> bool {
        self.binding.same_operation(binding) && self.work.same_context(work)
    }

    pub(super) fn attempt_ref(&self) -> ReviewAttemptRef {
        ReviewAttemptRef(Arc::clone(&self.id))
    }

    /// The same attempt, bound to the same operation and work. Only the
    /// approval owner calls it: the consent owner, for the entered state of a
    /// spent consent attempt (the spend removed the candidate, and its own
    /// handle, from the owner's live custody), kept until the attempt is
    /// released.
    pub(super) fn twin(&self) -> Self {
        Self {
            id: Arc::clone(&self.id),
            binding: self.binding.clone(),
            work: self.work.clone(),
        }
    }

    fn attribution(&self) -> ReviewOperationAttribution {
        ReviewOperationAttribution::from_attempt(self)
    }

    /// The same attempt (its id and generated status) bound to another exact
    /// operation and work: only the consent owner moves parked custody into
    /// a resumed run this way.
    pub(super) fn rebound(
        &self,
        binding: PreparedAuthorizationBinding,
        work: WorkAuthorizationContext,
    ) -> Self {
        Self {
            id: Arc::clone(&self.id),
            binding,
            work,
        }
    }

    /// Whether the attempt was issued under this exact work association.
    pub(super) fn admitted_under(&self, work: &WorkAuthorizationContext) -> bool {
        self.work.same_context(work)
    }
}

/// Typed review owner failure, internal to core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewOwnerError {
    Unavailable,
    Mismatch,
    Rejected(crate::generated::approval_lifecycle::ApprovalLifecycleRejectionReason),
}

/// Refused review commit that admits an effect (verdict or spend).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReservedReviewError<E> {
    /// The owner's commit reservation was held; review paths never wait
    /// for it.
    Contended,
    /// The final deadline/currentness check under the reservation refused.
    FinalCheck(E),
    Owner(ReviewOwnerError),
}

/// Why the final check under the commit reservation refused.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FinalCheckFailure {
    DeadlineExpired,
    /// The prepared owner's coherent current check refused; its failure is
    /// observed only after every lock is released.
    AuthorityChanged(crate::authorization::OperationAuthorizationError),
    ContextChanged,
}

/// Classify the retained decision's callback-free current check, run under a
/// commit reservation. A re-preparation request means the reviewed context is
/// no longer current; every other failure changed the authority.
pub(crate) fn retained_current_failure(
    check: &PreparedOperationCheck,
) -> Option<FinalCheckFailure> {
    match check.check_retained_current() {
        Ok(()) => None,
        Err(crate::authorization::OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == crate::authorization::OperationRefusalKind::ReprepareRequired =>
        {
            Some(FinalCheckFailure::ContextChanged)
        }
        Err(error) => Some(FinalCheckFailure::AuthorityChanged(error)),
    }
}

/// A refusal decided under an owner lock. An authority failure is observed
/// (arbitrary owner code) only by [`Self::resolve`], once every lock is
/// released.
#[derive(Debug)]
#[must_use = "resolve once every lock is released"]
pub(crate) enum EntryRefusal {
    Settled(crate::ToolError),
    Authority {
        check: PreparedOperationCheck,
        error: crate::authorization::OperationAuthorizationError,
    },
}

impl EntryRefusal {
    pub(crate) fn resolve(self) -> crate::ToolError {
        match self {
            Self::Settled(error) => error,
            Self::Authority { check, error } => check.observe_retained_check_failure(error).into(),
        }
    }
}

impl From<crate::ToolError> for EntryRefusal {
    fn from(error: crate::ToolError) -> Self {
        Self::Settled(error)
    }
}

/// A refused verdict record: the attempt was retired under the reservation,
/// or queued without waiting.
struct RefusedVerdict {
    error: crate::ToolError,
    pending: PendingObservations,
    /// The final check under the reservation found the reviewed decision
    /// no longer current (a re-preparation is required): the one refusal a
    /// superseding decision can follow.
    stale: bool,
}

/// A refused spend. When the spend already committed (`spent`), the entry
/// stays consumed: a later attempt refuses as already consumed, and nothing
/// is refunded.
struct SpendRefused {
    refusal: EntryRefusal,
    pending: PendingObservations,
    spent: bool,
}

impl SpendRefused {
    fn into_transition(
        self,
    ) -> (
        ReviewEntryState,
        Result<(), EntryRefusal>,
        PendingObservations,
        PendingObservations,
    ) {
        let state = if self.spent {
            ReviewEntryState::Consumed
        } else {
            ReviewEntryState::Settled
        };
        (
            state,
            Err(self.refusal),
            self.pending,
            PendingObservations::default(),
        )
    }
}

// ---------------------------------------------------------------------------
// Seam: admission at the common prepared entry, a non-consuming check point,
// and exactly one consuming step at the physical-effect leaf.
// ---------------------------------------------------------------------------

/// Whether a dispatcher can carry review to a tool's physical entry.
///
/// A leaf that declares `ConsumesAtEntry` calls
/// [`crate::agent::ToolDispatchContext::enter_reviewed_effect`] exactly once,
/// after its argument and preparation work and any waits, immediately before
/// the body or handoff. Wrappers forward the owning leaf's declaration. A
/// dispatcher that drops the dispatch context must stay `Unsupported`: a
/// required review then settles as local unavailable feedback and the tool
/// never runs unchecked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewEntrySupport {
    #[default]
    Unsupported,
    ConsumesAtEntry,
}

/// Trusted host review composition: the reviewer, the approval owner that
/// holds attempts, and the finite owner deadline. It never selects a tier.
pub struct BoundOperationReview {
    reviewer: Arc<dyn OperationReviewer>,
    approvals: super::ApprovalService,
    deadline: std::time::Duration,
    observer: Option<Arc<dyn OperationReviewObserver>>,
    human_consent: Option<HumanConsent>,
}

impl fmt::Debug for BoundOperationReview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundOperationReview")
            .field("deadline", &self.deadline)
            .field("observer", &self.observer.is_some())
            .field(
                "human_consent_validity",
                &self.human_consent.as_ref().map(|consent| consent.validity),
            )
            .finish_non_exhaustive()
    }
}

fn unsatisfied(kind: ReviewUnsatisfiedKind) -> crate::ToolError {
    crate::ToolError::ReviewUnsatisfied {
        kind,
        approval_id: None,
    }
}

fn unavailable(kind: ReviewUnavailableKind) -> crate::ToolError {
    crate::ToolError::ReviewUnavailable { kind }
}

/// Observations and settled reports of unrelated disposals, collected under
/// a lock and delivered after it is released: observations first, then
/// consent projections, then the reports (arbitrary observer code, so never
/// under a lock).
#[derive(Default)]
#[must_use = "deliver once every lock is released"]
struct PendingObservations {
    observations: Vec<ReviewObservation>,
    /// Boxed: every review refusal carries this beside its error.
    consent: Option<Box<ConsentProjections>>,
    reports: SettledReports,
}

impl PendingObservations {
    fn one(observation: ReviewObservation) -> Self {
        Self {
            observations: vec![observation],
            ..Self::default()
        }
    }

    fn consent(consent: ConsentProjections) -> Self {
        Self {
            consent: Some(Box::new(consent)),
            ..Self::default()
        }
    }

    fn with_reports(mut self, reports: SettledReports) -> Self {
        self.reports.absorb(reports);
        self
    }

    fn extend(&mut self, other: Self) {
        self.observations.extend(other.observations);
        match (&mut self.consent, other.consent) {
            (Some(consent), Some(more)) => consent.extend(*more),
            (consent @ None, more) => *consent = more,
            (Some(_), None) => {}
        }
        self.reports.absorb(other.reports);
    }
}

impl BoundOperationReview {
    /// `deadline` bounds one attempt, measured as an absolute monotonic
    /// instant from its start. The retained allow is valid only until then.
    /// Review waits hold the dispatch permit and the tool batch, so keep it
    /// below the tool timeout; an enclosing timeout that fires first abandons
    /// the attempt.
    pub fn new(
        reviewer: Arc<dyn OperationReviewer>,
        approvals: super::ApprovalService,
        deadline: std::time::Duration,
    ) -> Self {
        Self {
            reviewer,
            approvals,
            deadline,
            observer: None,
            human_consent: None,
        }
    }

    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn OperationReviewObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Route R3 to qualified human consent through `route`: each candidate
    /// gets one approval valid for `validity` (finite, chosen by the host)
    /// from its request. Without a route R3 settles locally and nothing is
    /// retained. This never selects a tier.
    #[must_use]
    pub fn with_human_consent(
        mut self,
        route: Arc<dyn HumanConsentRoute>,
        validity: std::time::Duration,
    ) -> Self {
        self.human_consent = Some(HumanConsent { route, validity });
        self
    }

    /// The approval owner that issues this review's human consent, as the
    /// runtime's owner-loss settlement of that consent
    /// ([`super::ConsentOwnerLossSettlement`]): present exactly when R3 routes
    /// to human consent. Without a route no consent is issued here, so none
    /// is owed.
    pub fn consent_owner_loss_settlement(
        &self,
    ) -> Option<Arc<dyn super::ConsentOwnerLossSettlement>> {
        self.human_consent
            .as_ref()
            .map(|_| Arc::new(self.approvals.clone()) as Arc<dyn super::ConsentOwnerLossSettlement>)
    }

    /// The approval owner that issues this review's human consent, as the
    /// issuer of the consent notices it owes the hosts of the consented
    /// sessions (a host's delivery owner projects them, through the `meerkat`
    /// crate's `ConsentNoticeProjector`): present exactly when R3 routes to
    /// human consent. Without a route no consent is issued here, so no
    /// notice is owed.
    pub fn consent_notice_issuer(&self) -> Option<Arc<super::ApprovalService>> {
        self.human_consent
            .as_ref()
            .map(|_| Arc::new(self.approvals.clone()))
    }

    /// The run that admitted `work` closed: park the unspent human consent it
    /// admitted for a resumed run of that work where it can be transferred,
    /// and withdraw the rest, without waiting for the approval owner.
    pub(crate) fn close_consent_for_work(&self, work: &WorkAuthorizationContext) {
        if self.human_consent.is_some() {
            self.approvals.close_consent_for_work(work);
        }
    }

    /// A run opened under `work` as `run_id`: when it resumes retained work
    /// through staged notice inputs, the parked consent their committed
    /// receipts name moves into it.
    pub(crate) fn open_consent_for_run(&self, work: &WorkAuthorizationContext, run_id: &RunId) {
        if self.human_consent.is_some() {
            self.approvals.open_consent_for_run(work, run_id);
        }
    }

    /// The decided human consent among `requested` that the run `run_id` of
    /// `work` can spend now (see
    /// [`super::ApprovalService::decided_consent_for_run`]); none without a
    /// human route.
    pub(crate) fn decided_consent_for_run(
        &self,
        work: &WorkAuthorizationContext,
        run_id: Option<&RunId>,
        requested: &[crate::ApprovalId],
    ) -> crate::approval::consent::DecidedConsentScan {
        if self.human_consent.is_none() {
            return crate::approval::consent::DecidedConsentScan::Decided(Vec::new());
        }
        self.approvals
            .decided_consent_for_run(work, run_id, requested)
    }

    /// At a boundary of the resumed run `run_id` of `work`: move into it, without
    /// waiting, the parked consent its staged inputs' committed receipts name
    /// that an earlier move left parked (see
    /// [`super::ApprovalService::retry_parked_consent_for_run`]); nothing
    /// without a human route.
    pub(crate) fn retry_parked_consent_for_run(
        &self,
        work: &WorkAuthorizationContext,
        run_id: &RunId,
    ) {
        if self.human_consent.is_some() {
            self.approvals.retry_parked_consent_for_run(work, run_id);
        }
    }

    /// The human consent the approval owner moved into the resumed run
    /// `run_id` of `work` by the committed receipts of its staged notice
    /// inputs (see [`super::ApprovalService::consent_moved_into_run`]); none
    /// without a human route.
    pub(crate) fn consent_moved_into_run(
        &self,
        work: &WorkAuthorizationContext,
        run_id: Option<&RunId>,
    ) -> crate::approval::consent::MovedConsentScan {
        if self.human_consent.is_none() {
            return crate::approval::consent::MovedConsentScan::Moved(Vec::new());
        }
        self.approvals.consent_moved_into_run(work, run_id)
    }

    /// Rebind the live consent custody of `approval_id` to its retained call
    /// re-resolved as `fresh` by the session's agent this process rebuilt
    /// (see [`super::ApprovalService::rebind_consent_to_reresolved_plan`]);
    /// nothing to rebind without a human route.
    pub(crate) fn rebind_consent_to_reresolved_plan(
        &self,
        approval_id: &ApprovalId,
        work: &WorkAuthorizationContext,
        run_id: Option<&RunId>,
        old: &PreparedOperationCheck,
        fresh: crate::ResolvedToolExecutionPlan,
    ) -> crate::approval::consent::ConsentRebind {
        if self.human_consent.is_none() {
            return crate::approval::consent::ConsentRebind::Refused(
                crate::approval::consent::ConsentRefusal::NoConsent,
            );
        }
        self.approvals
            .rebind_consent_to_reresolved_plan(approval_id, work, run_id, old, fresh)
    }

    fn observation(
        handle: &ReviewAttemptHandle,
        tool_call_id: &Arc<str>,
        status: ReviewAttemptStatus,
        retirement: Option<ReviewRetirementReason>,
    ) -> ReviewObservation {
        ReviewObservation {
            attempt: handle.attempt_ref(),
            tool_call_id: Arc::clone(tool_call_id),
            status,
            retirement,
        }
    }

    fn deliver(&self, pending: PendingObservations) {
        let PendingObservations {
            observations,
            consent,
            reports,
        } = pending;
        if let Some(observer) = self.observer.as_ref() {
            for observation in observations {
                observer.observe(observation);
            }
        }
        if let Some(consent) = consent {
            consent.deliver();
        }
        reports.run();
    }
}

// ---------------------------------------------------------------------------
// Supersession: one more model review when only the owner publication moved.
// ---------------------------------------------------------------------------

/// What a model review attempt produced before a publication change
/// superseded the decision it reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SupersededOutcome {
    /// The reviewer allowed the stale candidate.
    Allowed,
    /// The reviewer produced no verdict (an ordinary reviewer failure).
    NoVerdict,
    /// The reviewer denied or escalated.
    Negative,
}

impl SupersededOutcome {
    fn of(verdict: Option<ReviewVerdict>) -> Self {
        match verdict {
            Some(ReviewVerdict::Allow) => Self::Allowed,
            None => Self::NoVerdict,
            Some(ReviewVerdict::Deny | ReviewVerdict::Escalate) => Self::Negative,
        }
    }
}

/// A model review attempt retired because a completed owner publication
/// change superseded the decision it reviewed, before any dispatcher code
/// ran. Built only here, from typed facts: the current decision that the
/// canonical re-prepare of the same binding returned
/// ([`PreparedOperationCheck::current`]), never an error or a retirement
/// reason. Only [`ReviewSupersession::readmit`] consumes it.
#[derive(Debug)]
pub(crate) struct SupersededReview {
    /// The retired attempt's own settlement, which stands when nothing is
    /// admitted again.
    settled: crate::ToolError,
    /// The current decision of the same binding and work.
    successor: PreparedOperationCheck,
    /// The tier of the decision the retired attempt reviewed.
    reviewed_tier: OperationReviewTier,
    outcome: SupersededOutcome,
    /// The owner material the retired attempt was shown.
    material: ReviewOperationContextResolution,
}

/// Why admission retained no review entry.
#[derive(Debug)]
pub(crate) enum ReviewAdmissionRefusal {
    /// Settled locally, exactly as without supersession.
    Settled(crate::ToolError),
    /// The attempt was retired as superseded; what follows is
    /// [`ReviewSupersession::readmit`]'s decision.
    Superseded(Box<SupersededReview>),
}

impl From<crate::ToolError> for ReviewAdmissionRefusal {
    fn from(error: crate::ToolError) -> Self {
        Self::Settled(error)
    }
}

impl ReviewAdmissionRefusal {
    fn superseded(
        settled: crate::ToolError,
        successor: PreparedOperationCheck,
        reviewed: &PreparedOperationCheck,
        outcome: SupersededOutcome,
        material: ReviewOperationContextResolution,
    ) -> Self {
        Self::Superseded(Box::new(SupersededReview {
            settled,
            successor,
            reviewed_tier: reviewed.review_tier(),
            outcome,
            material,
        }))
    }

    /// The settlement that stands when nothing is admitted again.
    pub(crate) fn into_settled(self) -> crate::ToolError {
        match self {
            Self::Settled(error) => error,
            Self::Superseded(superseded) => superseded.into_settled(),
        }
    }
}

impl SupersededReview {
    /// The retired attempt's own settlement, when nothing is admitted again.
    pub(crate) fn into_settled(self) -> crate::ToolError {
        self.settled
    }
}

/// The one supersession decision of one fenced dispatch. A model review
/// superseded by a completed owner publication change before any dispatcher
/// code ran is admitted again once, as a new attempt reviewed from the start
/// under the current decision; otherwise the retired attempt's own
/// settlement stands. A verdict is never reused.
///
/// Every rule must hold:
/// - the dispatch's budget of one is unspent;
/// - the reviewed decision and its successor are both R2: human consent,
///   a consent attempt and a changed tier never re-admit;
/// - the retired attempt allowed or produced no verdict: a denial or an
///   escalation is never asked again, and an expired attempt settles;
/// - the tool time left fits a whole review deadline;
/// - the successor's owner material equals what the retired attempt was
///   shown: changed material reaches the model as the retired attempt's
///   feedback, and the model decides.
///
/// With the owner-change waits this bounds one fenced dispatch: at most
/// three wait states (its dispatch-entry preparation and each of at most two
/// review attempts), so at most 9 free rebuilds (3 x
/// [`OWNER_CHANGE_FREE_REBUILDS`](crate::authorization::OWNER_CHANGE_FREE_REBUILDS))
/// and 6 s of waiting (3 x
/// [`OWNER_CHANGE_WAIT_BOUND`](crate::authorization::OWNER_CHANGE_WAIT_BOUND)),
/// all inside the plan's tool timeout.
#[derive(Debug, Default)]
pub(crate) struct ReviewSupersession {
    spent: bool,
}

/// A successor admitted for one new model review attempt, with the owner
/// material that attempt is shown (read for it once, by the decision).
pub(crate) struct Readmission {
    pub(crate) successor: PreparedOperationCheck,
    pub(crate) material: ReviewOperationContextResolution,
}

impl ReviewSupersession {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Decide what follows `superseded`. The typed rules come first; the
    /// successor's material is read only when they all hold, and is then the
    /// new attempt's own single read.
    pub(crate) fn readmit(
        &mut self,
        review: Option<&Arc<BoundOperationReview>>,
        superseded: SupersededReview,
        remaining_tool_time: Option<std::time::Duration>,
    ) -> Result<Readmission, crate::ToolError> {
        let SupersededReview {
            settled,
            successor,
            reviewed_tier,
            outcome,
            material,
        } = superseded;
        let eligible = review.is_some_and(|review| {
            !self.spent
                && reviewed_tier == OperationReviewTier::R2
                && successor.review_tier() == OperationReviewTier::R2
                && matches!(
                    outcome,
                    SupersededOutcome::Allowed | SupersededOutcome::NoVerdict
                )
                && remaining_tool_time.is_none_or(|remaining| remaining >= review.deadline)
        });
        if !eligible {
            return Err(settled);
        }
        match resolve_operation_context(&successor) {
            Ok(current) if current == material => {
                self.spent = true;
                Ok(Readmission {
                    successor,
                    material: current,
                })
            }
            // Changed, newly required or unreadable material: the model is
            // told the review is no longer current, and decides.
            Ok(_) | Err(_) => Err(settled),
        }
    }
}

/// How a reviewed decision stands once its review window closed.
enum CurrentDecision {
    /// Still current: the verdict may be recorded.
    Unchanged,
    /// A completed owner publication change superseded it with this current
    /// decision of the same binding and work.
    Superseded(PreparedOperationCheck),
    /// The owner refused or failed, or a change in progress did not complete
    /// within the wait: the authority changed.
    Failed(OperationAuthorizationError),
    /// The attempt's own deadline passed (possibly while waiting).
    Expired,
}

/// Classify the reviewed decision through the canonical current/reprepare
/// path ([`PreparedOperationCheck::current`]). A re-preparation refused while
/// an owner change is still in progress on the work's own publication waits
/// for that change, within the attempt's own deadline and `churn`'s bounds,
/// and classifies again: a fact read while a change is half published is
/// never used, and waiting weakens no refusal.
async fn current_decision(
    prepared: &PreparedOperationCheck,
    work: &WorkAuthorizationContext,
    retained: &RetainedReview,
    churn: &mut OwnerChangeChurn,
) -> CurrentDecision {
    loop {
        if retained.expired() {
            return CurrentDecision::Expired;
        }
        let error = match prepared.current() {
            Ok(current) if current.same_check(prepared) => return CurrentDecision::Unchanged,
            Ok(successor) => return CurrentDecision::Superseded(successor),
            Err(error) => error,
        };
        if let OperationAuthorizationError::Refused(refusal) = &error
            && refusal.kind() == crate::authorization::OperationRefusalKind::ReprepareRequired
            && let Some(wait) = churn
                .wait_for_settled_change(
                    work.authorization().as_ref(),
                    Some(retained.remaining()),
                    "review_currentness",
                    Some(&prepared.binding().facts().operation_id),
                )
                .await
            && wait.settled()
        {
            continue;
        }
        return CurrentDecision::Failed(error);
    }
}

/// Admission at the common prepared entry, before entry staging.
///
/// R1 proceeds. R3 settles locally. R2 without a composed reviewer, or for a
/// dispatcher that cannot carry review to the tool's entry, is local
/// unavailable feedback before any attempt. Otherwise one bounded review
/// runs; expiry and currentness are validated before a verdict is accepted,
/// and an allow is retained (not spent) for the leaf.
///
/// A decision that a completed owner publication change superseded during
/// the review, or at the verdict's commit, retires the attempt and comes back
/// typed ([`ReviewAdmissionRefusal::Superseded`]) with its current successor,
/// for the fenced dispatch's [`ReviewSupersession`] (as does the retained
/// allow's check point at that dispatch's own root staging; see
/// [`ReviewEntry::check_superseding`]). An owner change still in
/// progress on the work's own publication is first waited out through
/// `churn`, this attempt's own wait state, within the attempt's own deadline.
/// `successor_material` is the owner material a re-admitted successor was
/// already read with ([`Readmission`]); without it the material is read
/// here, once.
pub(crate) async fn admit_operation_review_attempt(
    review: Option<&Arc<BoundOperationReview>>,
    support: ReviewEntrySupport,
    call: ToolCallView<'_>,
    prepared: &PreparedOperationCheck,
    work: &WorkAuthorizationContext,
    successor_material: Option<ReviewOperationContextResolution>,
    churn: &mut OwnerChangeChurn,
) -> Result<ReviewEntry, ReviewAdmissionRefusal> {
    match prepared.review_tier() {
        OperationReviewTier::R1 => return Ok(ReviewEntry::not_required()),
        // A model allow can never satisfy R3. Without a host human route it
        // settles locally with zero entry; with one, one human attempt is
        // retained for this exact candidate and it still settles locally:
        // nothing parks, and only a fresh explicit attempt can spend it.
        OperationReviewTier::R3 => {
            return Err(ReviewAdmissionRefusal::Settled(
                match review.and_then(|review| {
                    review
                        .human_consent
                        .as_ref()
                        .map(|consent| (review, consent))
                }) {
                    None => unsatisfied(ReviewUnsatisfiedKind::HumanConsentRequired),
                    Some(_) if support == ReviewEntrySupport::Unsupported => {
                        unavailable(ReviewUnavailableKind::UnsupportedEntry)
                    }
                    Some((review, consent)) => match resolve_operation_context(prepared) {
                        Ok(context) => {
                            request_human_consent(review, consent, call, prepared, &context)
                        }
                        Err(error) => error,
                    },
                },
            ));
        }
        OperationReviewTier::R2 => {}
    }
    if !prepared.work_authorization().same_context(work) {
        return Err(crate::ToolError::AuthorizationRefused {
            refusal: crate::authorization::OperationRefused::new(
                crate::authorization::OperationRefusalKind::MalformedFacts,
            ),
        }
        .into());
    }
    let Some(review) = review else {
        return Err(unavailable(ReviewUnavailableKind::ReviewerMissing).into());
    };
    if support == ReviewEntrySupport::Unsupported {
        return Err(unavailable(ReviewUnavailableKind::UnsupportedEntry).into());
    }
    // Read the owners' passive material once, before any attempt exists, so
    // a missing required context or an owner denial leaves no attempt. A
    // re-admitted successor brings the material its decision read for it.
    let operation_context = match successor_material {
        Some(successor_material) => successor_material,
        None => resolve_operation_context(prepared)?,
    };
    // The deadline starts before the first access to the owner. Beginning
    // the attempt takes the owner's reservation without waiting (this runs
    // on an async thread): a held owner refuses locally at once, typed, with
    // no attempt created.
    let deadline = tokio::time::Instant::now() + review.deadline;
    let handle = review
        .approvals
        .try_begin_review(prepared.binding(), work)
        .map_err(|_| unavailable(ReviewUnavailableKind::OwnerUnavailable))?;
    let attribution = handle.attribution();
    let tool_call_id: Arc<str> = Arc::from(call.id);
    review.deliver(PendingObservations::one(BoundOperationReview::observation(
        &handle,
        &tool_call_id,
        ReviewAttemptStatus::Pending,
        None,
    )));
    let mut retained = RetainedReview {
        review: Arc::clone(review),
        handle: Some(handle),
        admitted: prepared.clone(),
        deadline,
        tool_call_id,
        material: operation_context.clone(),
    };

    // Stage the candidate-to-attempt join in the existing protected input
    // audit before the reviewer can read a source or send a model request.
    // Staging failure is infrastructure failure, never a model denial.
    if let Err(error) = prepared.observe_review_attempt_started(attribution.attempt_ref().clone()) {
        let (error, pending) = retained.retire(ReviewRetirementReason::Abandoned, error.into());
        review.deliver(pending);
        return Err(error.into());
    }

    let candidate = ReviewCandidate {
        call,
        check: prepared,
        attribution,
        operation_context: material(&operation_context),
    };
    let verdict = match tokio::time::timeout_at(deadline, review.reviewer.review(&candidate)).await
    {
        Ok(verdict) => verdict,
        Err(_) => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::DeadlineExpired,
                unavailable(ReviewUnavailableKind::DeadlineExpired),
            );
            review.deliver(pending);
            return Err(human_fallback(review, call, prepared, &operation_context)
                .unwrap_or(error)
                .into());
        }
    };

    let verdict = match verdict {
        Ok(verdict) => Some(verdict),
        Err(ReviewerFailure::Unavailable) => None,
        Err(ReviewerFailure::ObservationUnavailable(error)) => {
            // Preserve a known audit failure before any later deadline or
            // currentness check. Disposal cannot turn it into a review verdict.
            let (error, pending) = retained.retire(ReviewRetirementReason::Abandoned, error.into());
            review.deliver(pending);
            return Err(error.into());
        }
    };
    let outcome = SupersededOutcome::of(verdict);

    // Validate before accepting any verdict: a result that is ready only at
    // or after the absolute deadline never wins, and a stale result is
    // retired rather than recorded. A decision that a completed owner change
    // superseded comes back typed, with its successor.
    match current_decision(prepared, work, &retained, churn).await {
        CurrentDecision::Unchanged => {}
        CurrentDecision::Expired => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::DeadlineExpired,
                unavailable(ReviewUnavailableKind::DeadlineExpired),
            );
            review.deliver(pending);
            return Err(human_fallback(review, call, prepared, &operation_context)
                .unwrap_or(error)
                .into());
        }
        CurrentDecision::Failed(error) => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::AuthorityChanged,
                crate::ToolError::from(error),
            );
            review.deliver(pending);
            return Err(error.into());
        }
        CurrentDecision::Superseded(successor) => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::ContextChanged,
                unsatisfied(ReviewUnsatisfiedKind::ContextChanged),
            );
            review.deliver(pending);
            return Err(ReviewAdmissionRefusal::superseded(
                error,
                successor,
                prepared,
                outcome,
                operation_context,
            ));
        }
    }

    // Every verdict is accepted only under the owner's commit reservation,
    // after the same deadline/currentness checks are repeated there.
    let escalated = matches!(verdict, Some(ReviewVerdict::Escalate));
    let settled = match verdict {
        Some(ReviewVerdict::Allow) => match retained.record_verdict(ReviewVerdict::Allow) {
            Ok(pending) => {
                review.deliver(pending);
                return Ok(ReviewEntry::retained(retained));
            }
            Err(refused) => Err(refused),
        },
        Some(ReviewVerdict::Deny) => retained
            .record_verdict(ReviewVerdict::Deny)
            .map(|pending| (pending, unsatisfied(ReviewUnsatisfiedKind::Denied))),
        Some(ReviewVerdict::Escalate) => retained
            .record_verdict(ReviewVerdict::Escalate)
            .map(|pending| (pending, unsatisfied(ReviewUnsatisfiedKind::Escalated))),
        None => retained
            .record_unavailable()
            .map(|pending| (pending, unavailable(ReviewUnavailableKind::ReviewerFailed)))
            .map_err(|(error, pending)| {
                Box::new(RefusedVerdict {
                    error,
                    pending,
                    stale: false,
                })
            }),
    };
    match settled {
        Ok((mut pending, error)) => {
            pending.extend(retained.release());
            review.deliver(pending);
            // The model attempt is settled (as escalated) and released
            // first; only then does the same human owner take it over.
            Err(escalated
                .then(|| human_fallback(review, call, prepared, &operation_context))
                .flatten()
                .unwrap_or(error)
                .into())
        }
        Err(refused) => {
            let RefusedVerdict {
                error,
                pending,
                stale,
            } = *refused;
            review.deliver(pending);
            if !stale {
                return Err(error.into());
            }
            // The decision went stale at the commit itself. The attempt was
            // retired there; its successor is searched for after the
            // reservation was released, as after the verdict.
            match current_decision(prepared, work, &retained, churn).await {
                CurrentDecision::Superseded(successor) => Err(ReviewAdmissionRefusal::superseded(
                    error,
                    successor,
                    prepared,
                    outcome,
                    operation_context,
                )),
                CurrentDecision::Unchanged
                | CurrentDecision::Expired
                | CurrentDecision::Failed(_) => Err(error.into()),
            }
        }
    }
}

/// [`admit_operation_review_attempt`] with no supersession owner after it:
/// the attempt reads its own material and has its own wait state, and a
/// superseded attempt settles with its own settlement.
#[cfg(any(test, feature = "test-support"))]
pub(crate) async fn admit_operation_review(
    review: Option<&Arc<BoundOperationReview>>,
    support: ReviewEntrySupport,
    call: ToolCallView<'_>,
    prepared: &PreparedOperationCheck,
    work: &WorkAuthorizationContext,
) -> Result<ReviewEntry, crate::ToolError> {
    admit_operation_review_attempt(
        review,
        support,
        call,
        prepared,
        work,
        None,
        &mut OwnerChangeChurn::new(),
    )
    .await
    .map_err(ReviewAdmissionRefusal::into_settled)
}

/// R2 human fallback after the model attempt was already settled and
/// retired: when a human route is configured, request consent for the same
/// candidate from it. `None` without a route (local feedback stands).
fn human_fallback(
    review: &Arc<BoundOperationReview>,
    call: ToolCallView<'_>,
    prepared: &PreparedOperationCheck,
    operation_context: &ReviewOperationContextResolution,
) -> Option<crate::ToolError> {
    review
        .human_consent
        .as_ref()
        .map(|consent| request_human_consent(review, consent, call, prepared, operation_context))
}

/// Read the owners' passive review material once for this review attempt,
/// after R1 is ruled out (a retained human consent's re-adjudication reads it
/// again, to compare; ADR-001 D9). A required context the owner cannot supply
/// is typed unavailable, an owner denial stays a refusal and an observation
/// failure stays infrastructure.
fn resolve_operation_context(
    prepared: &PreparedOperationCheck,
) -> Result<ReviewOperationContextResolution, crate::ToolError> {
    prepared
        .work_authorization()
        .authorization()
        .review_operation_context(prepared.binding())
        .map_err(|error| match error {
            OperationAuthorizationError::Unavailable => {
                unavailable(ReviewUnavailableKind::ContextUnavailable)
            }
            refused_or_observation => crate::ToolError::from(refused_or_observation),
        })
}

fn material(resolution: &ReviewOperationContextResolution) -> Option<&ReviewOperationContext> {
    match resolution {
        ReviewOperationContextResolution::NotApplicable => None,
        ReviewOperationContextResolution::Material(context) => Some(context),
    }
}

/// R3 with a host human route (or R2 taken over by it): issue one consent
/// approval for this exact candidate and retain its human attempt, then
/// settle locally with
/// action-required feedback. The decision never executes the action; a fresh
/// explicit attempt does ([`admit_consent_attempt_reporting`]).
fn request_human_consent(
    review: &Arc<BoundOperationReview>,
    consent: &HumanConsent,
    call: ToolCallView<'_>,
    prepared: &PreparedOperationCheck,
    operation_context: &ReviewOperationContextResolution,
) -> crate::ToolError {
    let owner_unavailable = || unavailable(ReviewUnavailableKind::OwnerUnavailable);
    // Present only material that is still current with the retained
    // decision. A deadline fallback can run long after the material was read;
    // any owner change since the preparation makes the retained check stale,
    // and then nothing is presented or requested as current.
    if let Err(error) = prepared.check_retained_current() {
        return match prepared.observe_retained_check_failure(error) {
            OperationAuthorizationError::Refused(refusal)
                if refusal.kind()
                    == crate::authorization::OperationRefusalKind::ReprepareRequired =>
            {
                unsatisfied(ReviewUnsatisfiedKind::ContextChanged)
            }
            error => crate::ToolError::from(error),
        };
    }
    // Host presentation, outside every owner lock. A candidate the route
    // cannot present readably gets no approval.
    let Ok(presentation) = consent.route.present(&ConsentPresentationCandidate {
        call,
        check: prepared,
        operation_context: material(operation_context),
    }) else {
        return owner_unavailable();
    };
    let Some(request) = consent_request(call, presentation, consent.validity) else {
        return owner_unavailable();
    };
    let Some(deadline) = tokio::time::Instant::now().checked_add(consent.validity) else {
        return owner_unavailable();
    };
    match review.approvals.try_request_consent(
        request,
        ConsentCandidateParts {
            admitted: prepared.clone(),
            material: operation_context.clone(),
            deadline,
            tool_call_id: Arc::from(call.id),
            route: Arc::clone(&consent.route),
            observer: review.observer.clone(),
        },
    ) {
        // The reference is the owner's own committed request, never one taken
        // from the call, its metadata or its arguments.
        Ok((approval_id, projections)) => {
            projections.deliver();
            crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::HumanConsentRequired,
                approval_id: Some(approval_id),
            }
        }
        Err(_) => owner_unavailable(),
    }
}

/// Admission of a fresh explicit attempt of one retained R3 candidate after
/// an authenticated human decision. The approval owner resolves the
/// candidate only for the presenting context's own admitted work and run, and
/// returns its exact retained decision: the caller dispatches the retained
/// binding-owned call through the common fenced tail, whose leaf spends the
/// decision once at entry. The dispatch itself admits through
/// [`admit_consent_attempt_reporting`].
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn admit_consent_attempt(
    review: Option<&Arc<BoundOperationReview>>,
    approval_id: &ApprovalId,
    work: &WorkAuthorizationContext,
    run_id: Option<&RunId>,
) -> Result<(ReviewEntry, PreparedOperationCheck), crate::ToolError> {
    admit_consent_attempt_reporting(review, approval_id, work, run_id)
        .map_err(|refused| refused.error)
}

/// A fresh attempt of one retained R3 candidate that its admission refused,
/// with zero entry.
pub(crate) struct ConsentAttemptRefusal {
    pub(crate) error: crate::ToolError,
    /// The candidate's held decision when the candidate resolved before the
    /// refusal: its retained call is the one the refusal is reported for.
    pub(crate) retained: Option<PreparedOperationCheck>,
}

/// Admission of a fresh explicit attempt of one retained R3 candidate, as
/// the retained dispatch admits it: a refusal after the candidate resolved
/// names the retained call it belongs to. The held decision is settled before
/// the attempt is handed it (ADR-001 D9;
/// [`super::ApprovalService::settle_consent_for_attempt`]): one that is not
/// current is decided afresh by its owners, and the attempt goes on only
/// with a current decision. A related change or the owners' refusal
/// withdraws the consent and refuses the attempt (`ContextChanged`, or that
/// refusal); while nothing can be settled now the attempt refuses as
/// `owner_unavailable` and the consent is kept.
pub(crate) fn admit_consent_attempt_reporting(
    review: Option<&Arc<BoundOperationReview>>,
    approval_id: &ApprovalId,
    work: &WorkAuthorizationContext,
    run_id: Option<&RunId>,
) -> Result<(ReviewEntry, PreparedOperationCheck), ConsentAttemptRefusal> {
    let unresolved = |error| ConsentAttemptRefusal {
        error,
        retained: None,
    };
    let Some(review) = review.filter(|review| review.human_consent.is_some()) else {
        return Err(unresolved(unsatisfied(
            ReviewUnsatisfiedKind::HumanConsentRequired,
        )));
    };
    let candidate = review
        .approvals
        .resolve_consent_attempt(approval_id, work, run_id)
        .map_err(|refusal| unresolved(refusal.into_tool_error()))?;
    if let Err(refusal) = review
        .approvals
        .settle_consent_for_attempt(approval_id, &candidate)
    {
        // Every owner lock is released: a refused authority is observed now.
        return Err(ConsentAttemptRefusal {
            retained: Some(candidate.admitted()),
            error: refusal.into_tool_error(),
        });
    }
    let admitted = candidate.admitted();
    let retained = RetainedConsent {
        review: Arc::clone(review),
        approval_id: approval_id.clone(),
        candidate,
    };
    Ok((
        ReviewEntry::new(
            ReviewEntryState::Consent(retained),
            Some(Arc::clone(review)),
        ),
        admitted,
    ))
}

/// Per-operation review entry, shared by clones of one operation's dispatch
/// context so the check point can run repeatedly and the leaf spends once.
#[derive(Clone)]
pub(crate) struct ReviewEntry(Arc<parking_lot::Mutex<ReviewEntryInner>>);

struct ReviewEntryInner {
    state: ReviewEntryState,
    review: Option<Arc<BoundOperationReview>>,
    /// The `Used` projection, delivered only after the effect returned:
    /// once the admitting dispatch ended and no entered custody is held.
    deferred: PendingObservations,

    /// Entered effects still running (a worker may outlive its dispatch).
    held: usize,
    dispatch_ended: bool,
}

enum ReviewEntryState {
    NotRequired,
    Retained(RetainedReview),
    /// A fresh attempt of a retained R3 candidate holding an authenticated
    /// human approval; the leaf spends it once.
    Consent(RetainedConsent),
    /// Spent, and its attempt handed back: the admitting dispatch ended, or
    /// the spend's final check refused after its commit, so nothing entered.
    /// A second spend refuses as already consumed.
    Consumed,
    Settled,
    /// The effect entered with this entry's one spend. The spent attempt
    /// stays registered with its owner as `Used`, the entered state, until
    /// the admitting dispatch ends and hands it back, leaving `Consumed`
    /// (the owner keeps it `Used` while an admitted step is in flight). A
    /// second spend refuses as already consumed.
    Entered(EnteredAttempt),
}

impl Drop for ReviewEntryInner {
    fn drop(&mut self) {
        // A retained allow drops (and retires as Abandoned) with its state;
        // a deferred `Used` projection is delivered after that point.
        self.state = ReviewEntryState::Settled;
        if let Some(review) = self.review.as_ref() {
            review.deliver(std::mem::take(&mut self.deferred));
        }
    }
}

impl fmt::Debug for ReviewEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match &self.0.lock().state {
            ReviewEntryState::NotRequired => "ReviewEntry(NotRequired)",
            ReviewEntryState::Retained(_) => "ReviewEntry(Retained)",
            ReviewEntryState::Consent(_) => "ReviewEntry(Consent)",
            ReviewEntryState::Consumed => "ReviewEntry(Consumed)",
            ReviewEntryState::Settled => "ReviewEntry(Settled)",
            ReviewEntryState::Entered(_) => "ReviewEntry(Entered)",
        })
    }
}

impl ReviewEntry {
    fn not_required() -> Self {
        Self::new(ReviewEntryState::NotRequired, None)
    }

    fn retained(retained: RetainedReview) -> Self {
        let review = Arc::clone(&retained.review);
        Self::new(ReviewEntryState::Retained(retained), Some(review))
    }

    fn new(state: ReviewEntryState, review: Option<Arc<BoundOperationReview>>) -> Self {
        Self(Arc::new(parking_lot::Mutex::new(ReviewEntryInner {
            state,
            review,
            deferred: PendingObservations::default(),
            held: 0,
            dispatch_ended: false,
        })))
    }

    pub(crate) fn same_entry(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// What this entry says about the effect its dispatch entered: the basis
    /// of the dispatch's entered witness. Reads the entry only; it spends,
    /// releases and observes nothing.
    pub(crate) fn entered_basis(&self) -> EnteredBasis {
        let inner = self.0.lock();
        match &inner.state {
            ReviewEntryState::NotRequired => EnteredBasis::Unreviewed,
            ReviewEntryState::Entered(entered) => {
                match (entered.handle.as_ref(), inner.review.as_ref()) {
                    (Some(handle), Some(review)) => EnteredBasis::Entered {
                        spent: entered.spent.clone(),
                        steps: EnteredSteps {
                            entry: self.clone(),
                            review: Arc::clone(review),
                            attempt: Arc::new(handle.twin()),
                        },
                    },
                    // Handed back already: nothing is entered any more.
                    _ => EnteredBasis::Ended,
                }
            }
            ReviewEntryState::Retained(_) | ReviewEntryState::Consent(_) => EnteredBasis::NotSpent,
            ReviewEntryState::Consumed | ReviewEntryState::Settled => EnteredBasis::Ended,
        }
    }

    /// Whether no spend of this entry committed: nothing entered on it and,
    /// for a consent attempt, its consent was not spent here (a spend that
    /// committed leaves the entry entered, then consumed). Reads the entry
    /// only.
    pub(crate) fn spent_nothing(&self) -> bool {
        !matches!(
            self.0.lock().state,
            ReviewEntryState::Entered(_) | ReviewEntryState::Consumed
        )
    }

    /// Reports settled by a later step of the entered effect, deferred with
    /// the spend's own `Used` projection: they run once the admitting
    /// dispatch ended and no entered custody is held, never inside a step
    /// (which may run under a store's lock).
    fn defer(&self, reports: SettledReports) {
        self.0
            .lock()
            .deferred
            .extend(PendingObservations::default().with_reports(reports));
    }

    /// Test adapter: the check point's settled projection.
    #[cfg(test)]
    pub(crate) fn check(&self, entering: &PreparedOperationCheck) -> Result<(), crate::ToolError> {
        self.check_superseding(entering)
            .map_err(ReviewAdmissionRefusal::into_settled)
    }

    /// Non-consuming, repeatable check point, run by every entry staging after
    /// its final authorization check with the decision that staging returned.
    /// A retained model review whose decision a completed owner publication
    /// change superseded (the entering decision is the canonical re-prepare
    /// of the same binding) is retired and comes back typed; only the fenced
    /// dispatch's own root staging acts on that, every other staging settles.
    pub(crate) fn check_superseding(
        &self,
        entering: &PreparedOperationCheck,
    ) -> Result<(), ReviewAdmissionRefusal> {
        let (result, pending, review) = {
            let mut inner = self.0.lock();
            let review = inner.review.clone();
            let current = std::mem::replace(&mut inner.state, ReviewEntryState::Settled);
            let (next, result, pending) = check_state(current, entering);
            inner.state = next;
            (result, pending, review)
        };
        if let Some(review) = review {
            review.deliver(pending);
        }
        result
    }

    /// Test adapter: the spend with its custody released at once.
    #[cfg(test)]
    pub(crate) fn consume(
        &self,
        entering: &PreparedOperationCheck,
    ) -> Result<(), crate::ToolError> {
        self.consume_held(entering).map(drop)
    }

    /// The single consuming step, at the physical-effect leaf, after the
    /// check point passed for the same decision. No external callback runs
    /// between this spend and the leaf's body: the `Used` projection is
    /// deferred until after the effect.
    ///
    /// The spend, returning custody of the entered effect: while it is held
    /// the `Used` projection stays deferred, even after the admitting
    /// dispatch ended (a blocking worker outliving a cancelled dispatch).
    ///
    /// A consent attempt whose entering decision is its staging's
    /// re-preparation of the consented one is re-adjudicated first, and the
    /// approver's eligibility evidence that went stale is decided afresh by
    /// its owner, outside this entry's lock and every owner lock, since both
    /// call into the host's owners (ADR-001 D9).
    pub(crate) fn consume_held(
        &self,
        entering: &PreparedOperationCheck,
    ) -> Result<ReviewedEntryCustody, crate::ToolError> {
        let rechecks = self.spend_rechecks(entering);
        let (result, pending, review) = {
            let mut inner = self.0.lock();
            let review = inner.review.clone();
            let current = std::mem::replace(&mut inner.state, ReviewEntryState::Settled);
            let fresh_spend = matches!(
                current,
                ReviewEntryState::Retained(_) | ReviewEntryState::Consent(_)
            );
            // `used` carries the spend's `Used` projection and the reports its
            // reservation settled: both run after the effect.
            let (next, result, pending, used) = consume_state(current, entering, &rechecks);
            inner.state = next;
            inner.deferred.extend(used);
            let result = result.map(|()| {
                if fresh_spend {
                    inner.held += 1;
                    ReviewedEntryCustody(Some(self.clone()))
                } else {
                    ReviewedEntryCustody::none()
                }
            });
            (result, pending, review)
        };
        if let Some(review) = review {
            // Only refusal-path projections; never the spend's `Used`.
            review.deliver(pending);
        }
        // A refused authority is observed only now, with every lock released.
        result.map_err(EntryRefusal::resolve)
    }

    /// What a consent attempt's spend decides afresh before its
    /// reservation, outside every lock (none for any other entry): the
    /// re-adjudication of `entering` when it is not the consented decision
    /// but one of its binding and work
    /// ([`ConsentCandidate::readjudicate_entering`]), and the eligibility
    /// owner's answer when the approver's evidence went stale
    /// ([`ConsentCandidate::recheck_eligibility`]).
    fn spend_rechecks(&self, entering: &PreparedOperationCheck) -> SpendRechecks {
        let candidate = match &self.0.lock().state {
            ReviewEntryState::Consent(consent) => Arc::clone(&consent.candidate),
            _ => return SpendRechecks::default(),
        };
        SpendRechecks {
            readjudication: candidate.readjudicate_entering(entering),
            eligibility: candidate.recheck_eligibility(),
        }
    }

    fn release_custody(&self) {
        let (pending, review) = {
            let mut inner = self.0.lock();
            inner.held = inner.held.saturating_sub(1);
            if inner.held == 0 && inner.dispatch_ended {
                (std::mem::take(&mut inner.deferred), inner.review.clone())
            } else {
                (PendingObservations::default(), None)
            }
        };
        match review {
            Some(review) => review.deliver(pending),
            None => pending.reports.run(),
        }
    }

    /// The admitting dispatch ended. On normal completion a still-retained
    /// allow was never spent (a failure before entry) and is released
    /// unspent; on cancellation or an enclosing timeout it is retired as
    /// `Abandoned`. Either way every clone (worker, transport) is closed and
    /// can no longer spend, and the deferred `Used` projection is delivered.
    fn end_dispatch(&self, completed: bool) {
        let (pending, review) = {
            let mut inner = self.0.lock();
            let mut pending = PendingObservations::default();
            match std::mem::replace(&mut inner.state, ReviewEntryState::Settled) {
                ReviewEntryState::Retained(mut retained) if completed => {
                    pending.extend(retained.release());
                }
                ReviewEntryState::Retained(mut retained) => {
                    let (_, retired) = retained.retire(
                        ReviewRetirementReason::Abandoned,
                        unavailable(ReviewUnavailableKind::DispatchEnded),
                    );
                    pending.extend(retired);
                }
                // An unspent consent (a failure before entry, a cancelled or
                // timed-out attempt) stays with its owner, still spendable
                // by a later fresh attempt; only this dispatch is closed.
                ReviewEntryState::Consent(_) => {}
                // The entered effect's dispatch ended: its spent attempt is
                // handed back (never waiting), so no later step is admitted;
                // the owner releases it once no admitted step is in flight.
                // A second spend still refuses as already consumed.
                ReviewEntryState::Entered(mut entered) => {
                    pending.extend(entered.release());
                    inner.state = ReviewEntryState::Consumed;
                }
                // A spent or not-required entry keeps its state: nested or
                // late R1 staging remains a plain authorization check, and a
                // second spend still refuses as already consumed.
                other @ (ReviewEntryState::NotRequired | ReviewEntryState::Consumed) => {
                    inner.state = other;
                }
                ReviewEntryState::Settled => {}
            }
            inner.dispatch_ended = true;
            // An entered worker still holding custody delivers `Used` when its
            // effect ends, not when the awaiting dispatch was cancelled.
            if inner.held == 0 {
                pending.extend(std::mem::take(&mut inner.deferred));
            }
            (pending, inner.review.clone())
        };
        match review {
            Some(review) => review.deliver(pending),
            None => pending.reports.run(),
        }
    }
}

/// Custody of one entered reviewed effect. Hold it for the whole effect
/// body; dropping it marks the effect ended. The review `Used` projection is
/// delivered once the admitting dispatch ended and no custody is held, so a
/// worker that outlives a cancelled dispatch is never reported used before
/// its body completed. Unreviewed and R1 entries carry an inert custody.
#[must_use = "hold the custody until the entered effect completes"]
pub struct ReviewedEntryCustody(Option<ReviewEntry>);

impl ReviewedEntryCustody {
    pub(crate) fn none() -> Self {
        Self(None)
    }

    /// The entry this custody holds, `None` for an inert custody (an
    /// unreviewed entry, which names no dispatch).
    pub(crate) fn entry(&self) -> Option<&ReviewEntry> {
        self.0.as_ref()
    }
}

impl fmt::Debug for ReviewedEntryCustody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() {
            "ReviewedEntryCustody(Held)"
        } else {
            "ReviewedEntryCustody(Inert)"
        })
    }
}

impl Drop for ReviewedEntryCustody {
    fn drop(&mut self) {
        if let Some(entry) = self.0.take() {
            entry.release_custody();
        }
    }
}

/// Owned by the admitting dispatch for exactly its lifetime. Completing it
/// settles normally; dropping it (cancellation, enclosing tool timeout)
/// abandons. In both cases every outstanding clone of the entry is closed, so
/// nothing can spend after the dispatch ended, independent of when the last
/// clone is dropped.
pub(crate) struct ReviewDispatchGuard(Option<ReviewEntry>);

impl ReviewDispatchGuard {
    pub(crate) fn new(entry: ReviewEntry) -> Self {
        Self(Some(entry))
    }

    pub(crate) fn complete(mut self) {
        if let Some(entry) = self.0.take() {
            entry.end_dispatch(true);
        }
    }
}

impl Drop for ReviewDispatchGuard {
    fn drop(&mut self) {
        if let Some(entry) = self.0.take() {
            entry.end_dispatch(false);
        }
    }
}

/// What a dispatch's own review entry says about the effect it entered.
pub(crate) enum EnteredBasis {
    /// Not reviewed (R1): nothing was spent and no attempt exists.
    Unreviewed,
    /// Entered with the entry's one spend: the decision spent, and the
    /// generated owner's custody of the spent attempt for the effect's later
    /// steps.
    Entered {
        spent: PreparedOperationCheck,
        steps: EnteredSteps,
    },
    /// Not spent: a retained review allow or consent.
    NotSpent,
    /// The admitting dispatch ended, or the spend entered nothing.
    Ended,
}

/// Custody of one entered effect's spent attempt for the effect's later
/// steps: each is admitted by the generated owner (`AdmitEnteredStep`) and
/// finished when its guard drops (`FinishEnteredStep`). Neither spends, and
/// this custody keeps nothing registered: the attempt stays `Used` only
/// while the owner says so (the admitting dispatch is live or an admitted
/// step is in flight).
#[derive(Clone)]
pub(crate) struct EnteredSteps {
    entry: ReviewEntry,
    review: Arc<BoundOperationReview>,
    attempt: Arc<ReviewAttemptHandle>,
}

impl EnteredSteps {
    /// Admit one later step of the entered effect for exactly `binding` and
    /// `work`, never waiting: a held owner is `Unavailable` at once, and the
    /// review inputs queued behind an earlier holder are settled into the
    /// owner before it decides. The step is in flight until the returned
    /// guard drops. Every report this settles is deferred with the entry's
    /// `Used` projection, so none runs inside the step.
    pub(crate) fn admit(
        &self,
        binding: &crate::authorization::PreparedAuthorizationBinding,
        work: &WorkAuthorizationContext,
    ) -> Result<EnteredStep, ReviewOwnerError> {
        let (admitted, reports) =
            self.review
                .approvals
                .admit_entered_step(&self.attempt, binding, work);
        self.entry.defer(reports);
        admitted.map(|()| EnteredStep(self.clone()))
    }
}

/// One admitted step of an entered effect, in flight until dropped. Its drop
/// finishes the step (`FinishEnteredStep`) on every exit path, an unwind
/// included, without waiting: a held owner gets the finish queued for its
/// next holder, so no finish is lost. The last finish of a closing effect
/// releases its attempt.
#[must_use = "the step is in flight until the guard drops"]
pub(crate) struct EnteredStep(EnteredSteps);

impl Drop for EnteredStep {
    fn drop(&mut self) {
        let reports = self
            .0
            .review
            .approvals
            .finish_entered_step(self.0.attempt.id());
        self.0.entry.defer(reports);
    }
}

type Transition = (
    ReviewEntryState,
    Result<(), ReviewAdmissionRefusal>,
    PendingObservations,
);

fn check_state(current: ReviewEntryState, entering: &PreparedOperationCheck) -> Transition {
    use ReviewEntryState::{Consent, Consumed, Entered, NotRequired, Retained, Settled};
    match (entering.review_tier(), current) {
        // Identity, not tier: the entering decision is the consented one (an
        // R3 candidate, or an R2 one a human took over), or one of its
        // binding and work that the staging prepared afresh after an owner
        // publication change, which the spend re-adjudicates (ADR-001 D9).
        (_, Consent(consent)) if entering.same_operation_and_work(&consent.admitted()) => {
            (Consent(consent), Ok(()), PendingObservations::default())
        }
        // The entering decision is another operation's: fresh consent.
        (_, Consent(consent)) => {
            let (error, pending) = consent.retire_changed();
            (Settled, Err(error.into()), pending)
        }
        (OperationReviewTier::R3, Retained(mut retained)) => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::ContextChanged,
                unsatisfied(ReviewUnsatisfiedKind::HumanConsentRequired),
            );
            (Settled, Err(error.into()), pending)
        }
        // Terminal states keep their disposition for every tier, before the
        // R3 catch-all: a spent clone stays spent (its spend refuses as
        // already consumed) and an ended dispatch's clone stays ended.
        (_, Consumed) => (Consumed, Ok(()), PendingObservations::default()),
        (_, Entered(entered)) => (Entered(entered), Ok(()), PendingObservations::default()),
        (_, Settled) => (
            Settled,
            Err(unavailable(ReviewUnavailableKind::DispatchEnded).into()),
            PendingObservations::default(),
        ),
        (OperationReviewTier::R3, _) => (
            Settled,
            Err(unsatisfied(ReviewUnsatisfiedKind::HumanConsentRequired).into()),
            PendingObservations::default(),
        ),
        // A decision re-prepared after review (or a changed tier, which only
        // a re-prepared decision can carry) means the reviewed context
        // changed. The entering decision is the staging's canonical re-prepare
        // of the same binding: it is handed back as the successor, and the
        // supersession decision alone says whether it is reviewed once more.
        (_, Retained(mut retained)) if !entering.same_check(&retained.admitted) => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::ContextChanged,
                unsatisfied(ReviewUnsatisfiedKind::ContextChanged),
            );
            let superseded = ReviewAdmissionRefusal::superseded(
                error,
                entering.clone(),
                &retained.admitted,
                SupersededOutcome::Allowed,
                retained.material.clone(),
            );
            (Settled, Err(superseded), pending)
        }
        (OperationReviewTier::R2, Retained(mut retained)) if retained.expired() => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::DeadlineExpired,
                unavailable(ReviewUnavailableKind::DeadlineExpired),
            );
            (Settled, Err(error.into()), pending)
        }
        (OperationReviewTier::R2, Retained(retained)) => {
            (Retained(retained), Ok(()), PendingObservations::default())
        }
        (OperationReviewTier::R1, NotRequired) => {
            (NotRequired, Ok(()), PendingObservations::default())
        }
        (OperationReviewTier::R1, Retained(mut retained)) => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::ContextChanged,
                unsatisfied(ReviewUnsatisfiedKind::ContextChanged),
            );
            (Settled, Err(error.into()), pending)
        }
        (OperationReviewTier::R2, NotRequired) => (
            Settled,
            Err(unsatisfied(ReviewUnsatisfiedKind::ContextChanged).into()),
            PendingObservations::default(),
        ),
    }
}

fn consume_state(
    current: ReviewEntryState,
    entering: &PreparedOperationCheck,
    rechecks: &SpendRechecks,
) -> (
    ReviewEntryState,
    Result<(), EntryRefusal>,
    PendingObservations,
    PendingObservations,
) {
    use ReviewEntryState::{Consent, Consumed, Entered, NotRequired, Retained, Settled};
    let none = PendingObservations::default;
    match current {
        Consent(consent) => match consent.consume(entering, rechecks) {
            Ok((used, entered)) => (Entered(entered), Ok(()), none(), used),
            Err(refused) => (*refused).into_transition(),
        },
        NotRequired => (NotRequired, Ok(()), none(), none()),
        Consumed => (
            Consumed,
            Err(unsatisfied(ReviewUnsatisfiedKind::AlreadyConsumed).into()),
            none(),
            none(),
        ),
        Entered(entered) => (
            Entered(entered),
            Err(unsatisfied(ReviewUnsatisfiedKind::AlreadyConsumed).into()),
            none(),
            none(),
        ),
        Settled => (
            Settled,
            Err(unavailable(ReviewUnavailableKind::DispatchEnded).into()),
            none(),
            none(),
        ),
        Retained(mut retained) => {
            // Absolute deadline at the spend, then the identity-bound spend.
            if retained.expired() {
                let (error, pending) = retained.retire(
                    ReviewRetirementReason::DeadlineExpired,
                    unavailable(ReviewUnavailableKind::DeadlineExpired),
                );
                return (Settled, Err(error.into()), pending, none());
            }
            if !entering.same_check(&retained.admitted) {
                let (error, pending) = retained.retire(
                    ReviewRetirementReason::ContextChanged,
                    unsatisfied(ReviewUnsatisfiedKind::ContextChanged),
                );
                return (Settled, Err(error.into()), pending, none());
            }
            match retained.consume(entering) {
                Ok((used, entered)) => (Entered(entered), Ok(()), none(), used),
                Err(refused) => (*refused).into_transition(),
            }
        }
    }
}

/// A fresh attempt's hold on one retained R3 candidate. The candidate's
/// custody stays with the approval owner: dropping this (a failure before
/// entry, cancellation) spends nothing and retires nothing.
pub(crate) struct RetainedConsent {
    review: Arc<BoundOperationReview>,
    approval_id: ApprovalId,
    candidate: Arc<ConsentCandidate>,
}

impl RetainedConsent {
    /// The decision the consent's custody holds now: the consented one, or
    /// one a re-adjudication found unchanged since (ADR-001 D9).
    fn admitted(&self) -> PreparedOperationCheck {
        self.candidate.admitted()
    }

    /// The entering decision is another operation's: retire the candidate
    /// (fresh consent is required) and settle this attempt.
    fn retire_changed(&self) -> (crate::ToolError, PendingObservations) {
        let reports = self.review.approvals.try_retire_consent(
            &self.approval_id,
            &self.candidate,
            ReviewRetirementReason::ContextChanged,
        );
        (
            unsatisfied(ReviewUnsatisfiedKind::ContextChanged),
            PendingObservations::default().with_reports(reports),
        )
    }

    /// Spend the decision once for the exact entering decision, with what
    /// the attempt decided afresh before it (`rechecks`: the re-adjudication
    /// of an entering decision its staging prepared afresh, the eligibility
    /// owner's answer for stale evidence). Returns the deferred `Used`
    /// projection and notice, which the caller must not deliver before the
    /// effect, and the attempt the spend left registered (`Used`) for the
    /// entered effect, until its dispatch ends.
    fn consume(
        &self,
        entering: &PreparedOperationCheck,
        rechecks: &SpendRechecks,
    ) -> Result<(PendingObservations, EnteredAttempt), Box<SpendRefused>> {
        let (entered, spent, mut projections) = self.review.approvals.consume_consent_for_entry(
            &self.approval_id,
            &self.candidate,
            entering,
            rechecks,
        );
        if entered.is_ok() || spent {
            // The spend committed: `Used` is its truth. On success it is
            // delivered after the effect; after a final-check refusal it goes
            // with the refusal (zero effect, nothing refunded).
            projections.observe(&self.candidate, ReviewAttemptStatus::Used, None);
            projections.notify(&self.candidate, &self.approval_id, ConsentNoticeKind::Used);
        }
        match entered {
            Ok(()) => Ok((
                PendingObservations::consent(projections),
                EnteredAttempt::new(
                    Arc::clone(&self.review),
                    self.candidate.entered_attempt(),
                    entering.clone(),
                ),
            )),
            Err(refusal) => {
                let mut pending = PendingObservations::consent(projections);
                if spent {
                    // Committed, but nothing entered: its attempt stays
                    // registered until released, so release it now.
                    pending.extend(
                        EnteredAttempt::new(
                            Arc::clone(&self.review),
                            self.candidate.entered_attempt(),
                            entering.clone(),
                        )
                        .release(),
                    );
                }
                Err(Box::new(SpendRefused {
                    refusal: refusal.into_entry_refusal(),
                    pending,
                    spent,
                }))
            }
        }
    }
}

/// The attempt one entry's spend left registered with its owner as `Used`:
/// the entered state of that effect. The admitting dispatch's end hands it
/// back (`ReleaseReview`, never waiting): the owner releases the attempt
/// when no admitted step of the effect is in flight, and otherwise closes
/// the effect, so no new step is admitted and the last admitted step's
/// finish releases it. It is handed back at once when the spend's final
/// check refused after its commit, so nothing entered. Dropped unreleased
/// (its entry dropped with no dispatch guard), it is handed back then.
pub(crate) struct EnteredAttempt {
    review: Arc<BoundOperationReview>,
    /// `None` once released.
    handle: Option<ReviewAttemptHandle>,
    /// The decision this entry's one spend entered: its review tier is the
    /// entered tier and its validity bounds every later step of the effect.
    spent: PreparedOperationCheck,
}

impl EnteredAttempt {
    fn new(
        review: Arc<BoundOperationReview>,
        handle: ReviewAttemptHandle,
        spent: PreparedOperationCheck,
    ) -> Self {
        Self {
            review,
            handle: Some(handle),
            spent,
        }
    }

    /// Hand the spent attempt back to its owner (`ReleaseReview`) without
    /// waiting: settled now when the owner is free, otherwise queued for its
    /// next commit. The owner releases it, or closes its effect while
    /// admitted steps are in flight (the last one's finish releases it).
    /// Returns the reports of other disposals settled here, which the caller
    /// delivers outside every lock.
    fn release(&mut self) -> PendingObservations {
        let Some(handle) = self.handle.take() else {
            return PendingObservations::default();
        };
        // A released `Used` attempt has no projection of its own: its `Used`
        // was observed at the spend.
        let (_, reports) = self.review.approvals.dispose_review(handle.id(), None, || {
            Box::new(|_: Result<ReviewAttemptStatus, ReviewOwnerError>| {})
        });
        PendingObservations::default().with_reports(reports)
    }
}

impl Drop for EnteredAttempt {
    fn drop(&mut self) {
        if self.handle.is_some() {
            let pending = self.release();
            self.review.deliver(pending);
        }
    }
}

/// One retained review attempt. Dropping it before settlement (caller
/// interrupt, enclosing deadline or any other drop of the dispatch) retires
/// the attempt as `Abandoned`; the review owner never claims caller
/// cancellation, which remains the turn owner's typed outcome.
pub(crate) struct RetainedReview {
    review: Arc<BoundOperationReview>,
    handle: Option<ReviewAttemptHandle>,
    admitted: PreparedOperationCheck,
    deadline: tokio::time::Instant,
    tool_call_id: Arc<str>,
    /// The owner material this attempt's reviewer was shown.
    material: ReviewOperationContextResolution,
}

impl RetainedReview {
    fn expired(&self) -> bool {
        tokio::time::Instant::now() >= self.deadline
    }

    /// Time left before the attempt's absolute deadline.
    fn remaining(&self) -> std::time::Duration {
        self.deadline
            .saturating_duration_since(tokio::time::Instant::now())
    }

    /// Run under the owner's commit reservation, immediately before the
    /// conditional commit: the absolute deadline, the entering decision's
    /// identity with the reviewed one, then the prepared owner's coherent
    /// current check of that retained decision. Nothing here re-prepares or
    /// observes: the entering decision's full current check already ran at
    /// its staging, outside every lock, and a failure here is observed only
    /// after the reservation is released.
    fn final_check(&self, entering: &PreparedOperationCheck) -> Result<(), FinalCheckFailure> {
        if self.expired() {
            return Err(FinalCheckFailure::DeadlineExpired);
        }
        if !entering.same_check(&self.admitted) {
            return Err(FinalCheckFailure::ContextChanged);
        }
        retained_current_failure(&self.admitted).map_or(Ok(()), Err)
    }

    /// Retirement reason for a refused reserved commit. Contention (a held
    /// owner, never waited for) abandons the attempt.
    fn refused_retirement(
        error: &ReservedReviewError<FinalCheckFailure>,
    ) -> ReviewRetirementReason {
        match error {
            ReservedReviewError::FinalCheck(FinalCheckFailure::DeadlineExpired) => {
                ReviewRetirementReason::DeadlineExpired
            }
            ReservedReviewError::Contended => ReviewRetirementReason::Abandoned,
            ReservedReviewError::FinalCheck(FinalCheckFailure::AuthorityChanged(_))
            | ReservedReviewError::Owner(ReviewOwnerError::Mismatch) => {
                ReviewRetirementReason::AuthorityChanged
            }
            ReservedReviewError::FinalCheck(FinalCheckFailure::ContextChanged) => {
                ReviewRetirementReason::ContextChanged
            }
            ReservedReviewError::Owner(_) => ReviewRetirementReason::Abandoned,
        }
    }

    /// Typed refusal of a reserved commit. The attempt was already retired
    /// and disposed under the same reservation, or queued without waiting
    /// (its observation then follows from the owner's settlement).
    fn refused_commit(
        &mut self,
        refused: RefusedReviewCommit<FinalCheckFailure>,
    ) -> (EntryRefusal, PendingObservations) {
        let reason = Self::refused_retirement(&refused.error);
        if let Some(status) = refused.committed {
            // A committed spend refused by its final check after the owner
            // exit: the attempt stays spent (its `Used` is reported now, as
            // nothing will enter) and nothing is refunded. Nothing entered,
            // so the spent attempt is released now (never waiting).
            let mut pending = match self.handle.as_ref() {
                Some(handle) => PendingObservations::one(BoundOperationReview::observation(
                    handle,
                    &self.tool_call_id,
                    status,
                    None,
                )),
                None => PendingObservations::default(),
            };
            pending.extend(self.dispose(None));
            let pending = pending.with_reports(refused.reports);
            let refusal = match refused.error {
                ReservedReviewError::FinalCheck(FinalCheckFailure::AuthorityChanged(error)) => {
                    EntryRefusal::Authority {
                        check: self.admitted.clone(),
                        error,
                    }
                }
                ReservedReviewError::FinalCheck(FinalCheckFailure::DeadlineExpired) => {
                    unavailable(ReviewUnavailableKind::DeadlineExpired).into()
                }
                _ => unsatisfied(ReviewUnsatisfiedKind::ContextChanged).into(),
            };
            return (refusal, pending);
        }
        let error = match refused.error {
            ReservedReviewError::Contended => unavailable(ReviewUnavailableKind::OwnerUnavailable),
            ReservedReviewError::FinalCheck(FinalCheckFailure::DeadlineExpired) => {
                unavailable(ReviewUnavailableKind::DeadlineExpired)
            }
            ReservedReviewError::FinalCheck(FinalCheckFailure::AuthorityChanged(error)) => {
                let pending = self.refused_disposal(reason, refused.disposal, refused.reports);
                return (
                    EntryRefusal::Authority {
                        check: self.admitted.clone(),
                        error,
                    },
                    pending,
                );
            }
            ReservedReviewError::FinalCheck(FinalCheckFailure::ContextChanged) => {
                unsatisfied(ReviewUnsatisfiedKind::ContextChanged)
            }
            ReservedReviewError::Owner(ReviewOwnerError::Mismatch) => {
                crate::ToolError::AuthorizationRefused {
                    refusal: crate::authorization::OperationRefused::new(
                        crate::authorization::OperationRefusalKind::MalformedFacts,
                    ),
                }
            }
            ReservedReviewError::Owner(error) => {
                if let ReviewOwnerError::Rejected(reason) = error {
                    tracing::warn!(?reason, "review owner rejected a review commit");
                }
                unavailable(ReviewUnavailableKind::OwnerUnavailable)
            }
        };
        let pending = self.refused_disposal(reason, refused.disposal, refused.reports);
        (EntryRefusal::Settled(error), pending)
    }

    /// Projection of the attempt's disposal under a refused reservation.
    fn refused_disposal(
        &mut self,
        reason: ReviewRetirementReason,
        disposal: Option<Result<ReviewAttemptStatus, ReviewOwnerError>>,
        reports: SettledReports,
    ) -> PendingObservations {
        let pending = match (self.handle.take(), disposal) {
            (Some(handle), Some(Ok(status))) => {
                PendingObservations::one(BoundOperationReview::observation(
                    &handle,
                    &self.tool_call_id,
                    status,
                    Some(reason),
                ))
            }
            _ => PendingObservations::default(),
        };
        pending.with_reports(reports)
    }

    /// Report for a disposal the owner settles later: delivers the settled
    /// terminal projection (never a claimed one) once it happens.
    fn queued_report(
        &self,
        attempt: ReviewAttemptRef,
        retirement: Option<ReviewRetirementReason>,
    ) -> impl FnOnce() -> crate::approval::QueuedDisposalReport + use<> {
        let review = Arc::clone(&self.review);
        let tool_call_id = Arc::clone(&self.tool_call_id);
        move || {
            Box::new(
                move |settled: Result<ReviewAttemptStatus, ReviewOwnerError>| {
                    if let (Ok(status), Some(_)) = (settled, retirement) {
                        review.deliver(PendingObservations::one(ReviewObservation {
                            attempt,
                            tool_call_id,
                            status,
                            retirement,
                        }));
                    }
                },
            )
        }
    }

    /// Retire and dispose without waiting for the owner. Returns the
    /// authorized terminal projection when settled now, for the caller to
    /// deliver outside any lock; a queued disposal reports when settled.
    fn retire(
        &mut self,
        reason: ReviewRetirementReason,
        error: crate::ToolError,
    ) -> (crate::ToolError, PendingObservations) {
        let pending = self.dispose(Some(reason));
        (error, pending)
    }

    fn dispose(&mut self, retirement: Option<ReviewRetirementReason>) -> PendingObservations {
        let Some(handle) = self.handle.take() else {
            return PendingObservations::default();
        };
        let report = self.queued_report(handle.attempt_ref(), retirement);
        let (disposal, reports) =
            self.review
                .approvals
                .dispose_review(handle.id(), retirement, report);
        let pending = match disposal {
            Some(Ok(status)) if retirement.is_some() => PendingObservations::one(
                BoundOperationReview::observation(&handle, &self.tool_call_id, status, retirement),
            ),
            _ => PendingObservations::default(),
        };
        // Other disposals' reports: delivered by the caller outside its locks.
        pending.with_reports(reports)
    }

    /// Accept the verdict under the owner's single commit reservation, after
    /// the final check of the reviewed decision. A refusal retires the
    /// attempt there, or queues it without waiting on contention.
    fn record_verdict(
        &mut self,
        verdict: ReviewVerdict,
    ) -> Result<PendingObservations, Box<RefusedVerdict>> {
        let Some(handle) = self.handle.as_ref() else {
            return Err(Box::new(RefusedVerdict {
                error: unavailable(ReviewUnavailableKind::OwnerUnavailable),
                pending: PendingObservations::default(),
                stale: false,
            }));
        };
        let recorded = self.review.approvals.record_review_verdict(
            handle,
            verdict,
            || self.final_check(&self.admitted),
            Self::refused_retirement,
            self.queued_report(
                handle.attempt_ref(),
                Some(ReviewRetirementReason::Abandoned),
            ),
        );
        match recorded {
            Ok((status, reports)) => Ok(PendingObservations::one(
                BoundOperationReview::observation(handle, &self.tool_call_id, status, None),
            )
            .with_reports(reports)),
            // Called outside every entry lock, and the owner's reservation
            // is already released: observe a refused authority now.
            Err(refused) => {
                let stale = matches!(
                    refused.error,
                    ReservedReviewError::FinalCheck(FinalCheckFailure::ContextChanged)
                );
                let (refusal, pending) = self.refused_commit(refused);
                Err(Box::new(RefusedVerdict {
                    error: refusal.resolve(),
                    pending,
                    stale,
                }))
            }
        }
    }

    /// Record a reviewer failure under the non-blocking reservation. A contended
    /// or refused record retires the attempt there (or queues it) and still
    /// settles as the reviewer failure.
    fn record_unavailable(
        &mut self,
    ) -> Result<PendingObservations, (crate::ToolError, PendingObservations)> {
        let Some(handle) = self.handle.as_ref() else {
            return Err((
                unavailable(ReviewUnavailableKind::OwnerUnavailable),
                PendingObservations::default(),
            ));
        };
        let recorded = self.review.approvals.record_review_unavailable(
            handle,
            |_| ReviewRetirementReason::Abandoned,
            self.queued_report(
                handle.attempt_ref(),
                Some(ReviewRetirementReason::Abandoned),
            ),
        );
        match recorded {
            Ok((status, reports)) => Ok(PendingObservations::one(
                BoundOperationReview::observation(handle, &self.tool_call_id, status, None),
            )
            .with_reports(reports)),
            Err(refused) => {
                let reason = ReviewRetirementReason::Abandoned;
                let pending = match (self.handle.take(), refused.disposal) {
                    (Some(handle), Some(Ok(status))) => {
                        PendingObservations::one(BoundOperationReview::observation(
                            &handle,
                            &self.tool_call_id,
                            status,
                            Some(reason),
                        ))
                    }
                    _ => PendingObservations::default(),
                };
                Err((
                    unavailable(ReviewUnavailableKind::ReviewerFailed),
                    pending.with_reports(refused.reports),
                ))
            }
        }
    }

    /// Dispose an attempt that already reached a settled or unspent state,
    /// without waiting for the owner.
    fn release(&mut self) -> PendingObservations {
        self.dispose(None)
    }

    /// Spend the allow once for the exact entering decision and its work,
    /// under one reservation, so nothing waits on the owner between the
    /// spend and the effect. Returns the deferred `Used` projection, which
    /// the caller must not deliver before the effect, and the attempt the
    /// spend left registered (`Used`) for the entered effect, until its
    /// dispatch ends.
    fn consume(
        &mut self,
        entering: &PreparedOperationCheck,
    ) -> Result<(PendingObservations, EnteredAttempt), Box<SpendRefused>> {
        let Some(handle) = self.handle.as_ref() else {
            return Err(Box::new(SpendRefused {
                refusal: unavailable(ReviewUnavailableKind::OwnerUnavailable).into(),
                pending: PendingObservations::default(),
                spent: false,
            }));
        };
        let spent = self.review.approvals.consume_review_for_entry(
            handle,
            entering,
            entering.work_authorization(),
            || self.final_check(entering),
            Self::refused_retirement,
            self.queued_report(
                handle.attempt_ref(),
                Some(ReviewRetirementReason::Abandoned),
            ),
        );
        match spent {
            Ok((status, reports)) => {
                let used =
                    BoundOperationReview::observation(handle, &self.tool_call_id, status, None);
                // `Used` and the reports this reservation settled are
                // delivered together, after the effect.
                let pending = PendingObservations::one(used).with_reports(reports);
                // The attempt now belongs to the entered effect: registered
                // as `Used` until its dispatch ends and releases it.
                let entered = EnteredAttempt {
                    review: Arc::clone(&self.review),
                    handle: self.handle.take(),
                    spent: entering.clone(),
                };
                Ok((pending, entered))
            }
            Err(refused) => {
                let spent = refused.committed.is_some();
                let (refusal, pending) = self.refused_commit(refused);
                Err(Box::new(SpendRefused {
                    refusal,
                    pending,
                    spent,
                }))
            }
        }
    }
}

impl Drop for RetainedReview {
    /// Never waits on the owner: an abandoned attempt is retired now when the
    /// owner is free, otherwise queued for its next commit.
    fn drop(&mut self) {
        let pending = self.dispose(Some(ReviewRetirementReason::Abandoned));
        self.review.deliver(pending);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod seam_tests {
    //! The dispatch-lifetime guard closes every clone of a review entry, so a
    //! worker or transport clone held past the dispatch can never spend.

    use super::*;
    use crate::authorization::{
        AuthorizationOperation, OperationAuthorizationError, OperationAuthorizationFacts,
        PreparedAuthorizationBinding, PreparedOperationAuthorization, SourceAuthorizationFacts,
        SourceAuthorizationTarget, SourceAuthorizationUse, WorkAuthorization,
    };
    use crate::exact_operation::OperationExecutionScope;
    use crate::memory::MemorySearchScope;
    use crate::ops::OperationId;
    use crate::types::SessionId;

    struct R2;

    impl PreparedOperationAuthorization for R2 {
        fn review_tier(&self) -> OperationReviewTier {
            OperationReviewTier::R2
        }

        fn check_current(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            Ok(())
        }
    }

    struct R2Work;

    impl WorkAuthorization for R2Work {
        /// This test owner has no review material: explicitly not applicable.
        fn review_operation_context(
            &self,
            _binding: &crate::authorization::PreparedAuthorizationBinding,
        ) -> Result<
            crate::authorization::ReviewOperationContextResolution,
            crate::authorization::OperationAuthorizationError,
        > {
            Ok(crate::authorization::ReviewOperationContextResolution::NotApplicable)
        }

        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Ok(Arc::new(R2))
        }
    }

    struct Allow {
        expected_work: WorkAuthorizationContext,
        expected_binding: PreparedAuthorizationBinding,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl OperationReviewer for Allow {
        async fn review(
            &self,
            candidate: &ReviewCandidate<'_>,
        ) -> Result<ReviewVerdict, ReviewerFailure> {
            assert!(
                candidate
                    .work_authorization()
                    .same_context(&self.expected_work),
                "review must retain the actual admitted work owner"
            );
            assert!(
                candidate.binding().same_operation(&self.expected_binding),
                "review must retain the exact prepared operation"
            );
            let lookalike = WorkAuthorizationContext::new(
                Arc::clone(self.expected_work.authorization()),
                self.expected_work.execution_scope().clone(),
            );
            assert!(
                !candidate.work_authorization().same_context(&lookalike),
                "equal coordinates cannot reconstruct the retained work context"
            );
            Ok(ReviewVerdict::Allow)
        }
    }

    #[derive(Default)]
    struct Recording(
        parking_lot::Mutex<Vec<(ReviewAttemptStatus, Option<ReviewRetirementReason>)>>,
    );

    impl OperationReviewObserver for Recording {
        fn observe(&self, observation: ReviewObservation) {
            self.0
                .lock()
                .push((observation.status, observation.retirement));
        }
    }

    async fn admitted() -> (ReviewEntry, PreparedOperationCheck, Arc<Recording>) {
        let work = WorkAuthorizationContext::new(Arc::new(R2Work), OperationExecutionScope::Domain);
        let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: OperationId(uuid::Uuid::nil()),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(
                    SessionId::from_uuid(uuid::Uuid::nil()),
                )),
                usage: SourceAuthorizationUse::Read,
            }),
        });
        let check = PreparedOperationCheck::prepare(work.clone(), binding).expect("prepared");
        let observer = Arc::new(Recording::default());
        let review = Arc::new(
            BoundOperationReview::new(
                Arc::new(Allow {
                    expected_work: work.clone(),
                    expected_binding: check.binding().clone(),
                }),
                super::super::ApprovalService::new(),
                std::time::Duration::from_secs(30),
            )
            .with_observer(observer.clone()),
        );
        let args = serde_json::value::RawValue::from_string("{}".into()).expect("args");
        let call = ToolCallView {
            id: "call",
            name: "tool",
            args: &args,
        };
        let entry = admit_operation_review(
            Some(&review),
            ReviewEntrySupport::ConsumesAtEntry,
            call,
            &check,
            &work,
        )
        .await
        .expect("allowed review is retained");
        (entry, check, observer)
    }

    fn statuses(
        observer: &Recording,
    ) -> Vec<(ReviewAttemptStatus, Option<ReviewRetirementReason>)> {
        observer.0.lock().clone()
    }

    #[tokio::test]
    async fn a_clone_held_after_the_dispatch_was_cancelled_cannot_spend() {
        let (entry, check, observer) = admitted().await;
        let worker_clone = entry.clone();
        // The enclosing dispatch is cancelled or times out: its guard drops.
        drop(ReviewDispatchGuard::new(entry));
        assert_eq!(
            worker_clone.consume(&check),
            Err(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::DispatchEnded,
            })
        );
        assert_eq!(
            statuses(&observer),
            vec![
                (ReviewAttemptStatus::Pending, None),
                (ReviewAttemptStatus::Allowed, None),
                (
                    ReviewAttemptStatus::Retired,
                    Some(ReviewRetirementReason::Abandoned)
                ),
            ]
        );
    }

    #[tokio::test]
    async fn a_completed_dispatch_releases_unspent_and_closes_its_clones() {
        let (entry, check, observer) = admitted().await;
        let worker_clone = entry.clone();
        ReviewDispatchGuard::new(entry).complete();
        assert_eq!(
            worker_clone.consume(&check),
            Err(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::DispatchEnded,
            })
        );
        assert_eq!(
            statuses(&observer),
            vec![
                (ReviewAttemptStatus::Pending, None),
                (ReviewAttemptStatus::Allowed, None),
            ],
            "released unspent: never Used, never relabelled"
        );
    }

    #[tokio::test]
    async fn a_spent_entry_defers_used_until_the_dispatch_completes() {
        let (entry, check, observer) = admitted().await;
        let guard = ReviewDispatchGuard::new(entry.clone());
        entry.consume(&check).expect("single spend");
        assert!(
            !statuses(&observer).contains(&(ReviewAttemptStatus::Used, None)),
            "Used is not delivered between the spend and the effect"
        );
        assert_eq!(
            entry.consume(&check),
            Err(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                approval_id: None,
            })
        );
        guard.complete();
        assert_eq!(
            statuses(&observer).last(),
            Some(&(ReviewAttemptStatus::Used, None))
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod reservation_tests {
    //! The approval owner's commit reservation under real contention: a
    //! human `decide` holds it through a blocked store write. Review paths
    //! never wait for it (they run on async threads): a held owner refuses
    //! locally at once, and refusal cleanup is queued for the owner's next
    //! commit. Deadline and currentness are re-checked after the
    //! reservation is taken, and no queued report runs between a successful
    //! spend and the effect.

    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::approval::{
        ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
        ApprovalProposedAction, ApprovalRecord, ApprovalRequest, ApprovalResourceId,
        ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalService, ApprovalStore,
        ApprovalStoreError,
    };
    use crate::authorization::{
        AuthorizationOperation, OperationAuthorizationError, OperationAuthorizationFacts,
        OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
        PreparedOperationAuthorization, SourceAuthorizationFacts, SourceAuthorizationTarget,
        SourceAuthorizationUse, WorkAuthorization,
    };
    use crate::exact_operation::OperationExecutionScope;
    use crate::memory::MemorySearchScope;
    use crate::ops::OperationId;
    use crate::types::SessionId;

    const DEADLINE: Duration = Duration::from_secs(30);
    /// Real-time hang guard: no harness wait or held reservation outlives it.
    const BOUND: Duration = Duration::from_secs(10);

    /// Republishable R2 owner: a publication change makes a retained
    /// decision stale, and re-preparation yields a different decision.
    #[derive(Default)]
    struct Owner {
        sequence: AtomicU64,
        /// Runs once inside the next currentness check (test injection).
        on_check: parking_lot::Mutex<Option<Box<dyn FnOnce() + Send>>>,
        /// The current permission is withdrawn: every check refuses.
        denied: AtomicBool,
        /// Arbitrary owner code run while a refusal is observed.
        on_observe: parking_lot::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    }

    struct Decision {
        owner: Arc<Owner>,
        sequence: u64,
    }

    impl PreparedOperationAuthorization for Decision {
        fn review_tier(&self) -> OperationReviewTier {
            OperationReviewTier::R2
        }

        fn check_current(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            let hook = self.owner.on_check.lock().take();
            if let Some(hook) = hook {
                hook();
            }
            if self.owner.denied.load(Ordering::Acquire) {
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
        /// This test owner has no review material: explicitly not applicable.
        fn review_operation_context(
            &self,
            _binding: &crate::authorization::PreparedAuthorizationBinding,
        ) -> Result<
            crate::authorization::ReviewOperationContextResolution,
            crate::authorization::OperationAuthorizationError,
        > {
            Ok(crate::authorization::ReviewOperationContextResolution::NotApplicable)
        }

        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Ok(Arc::new(Decision {
                owner: Arc::clone(&self.0),
                sequence: self.0.sequence.load(Ordering::Acquire),
            }))
        }
    }

    /// Blocks the armed store write (inside `decide`, under the owner's state
    /// lock) until released, for at most `BOUND`.
    struct GatedStore {
        armed: AtomicBool,
        held: parking_lot::Mutex<mpsc::Sender<()>>,
        release: parking_lot::Mutex<mpsc::Receiver<()>>,
    }

    impl ApprovalStore for GatedStore {
        fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
            Ok(Vec::new())
        }

        fn get(
            &self,
            _approval_id: &crate::approval::ApprovalId,
        ) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
            Ok(None)
        }

        fn put_if_current(
            &self,
            _expected: Option<&ApprovalRecord>,
            _record: &ApprovalRecord,
        ) -> Result<(), ApprovalStoreError> {
            if self.armed.swap(false, Ordering::AcqRel) {
                let _ = self.held.lock().send(());
                let _ = self.release.lock().recv_timeout(BOUND);
            }
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            true
        }
    }

    struct ImmediateAllow;

    #[async_trait]
    impl OperationReviewer for ImmediateAllow {
        async fn review(
            &self,
            _candidate: &ReviewCandidate<'_>,
        ) -> Result<ReviewVerdict, ReviewerFailure> {
            Ok(ReviewVerdict::Allow)
        }
    }

    /// Allows once the test lets it, after signalling that review began.
    struct GatedAllow {
        reviewing: tokio::sync::mpsc::UnboundedSender<()>,
        proceed: parking_lot::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    #[async_trait]
    impl OperationReviewer for GatedAllow {
        async fn review(
            &self,
            _candidate: &ReviewCandidate<'_>,
        ) -> Result<ReviewVerdict, ReviewerFailure> {
            let _ = self.reviewing.send(());
            let proceed = self.proceed.lock().take();
            if let Some(proceed) = proceed {
                let _ = proceed.await;
            }
            Ok(ReviewVerdict::Allow)
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

    impl Recording {
        fn statuses(&self) -> Statuses {
            self.0.lock().clone()
        }

        fn used(&self) -> bool {
            self.statuses()
                .iter()
                .any(|(status, _)| *status == ReviewAttemptStatus::Used)
        }
    }

    type ProbeAction = Arc<parking_lot::Mutex<Option<Box<dyn FnOnce() + Send>>>>;

    struct Harness {
        approvals: ApprovalService,
        store: Arc<GatedStore>,
        held: mpsc::Receiver<()>,
        release: mpsc::Sender<()>,
        /// Runs once at the next reserved commit, after its pre-checks and
        /// before it takes the reservation.
        probe_action: ProbeAction,
        owner: Arc<Owner>,
        work: WorkAuthorizationContext,
        check: PreparedOperationCheck,
    }

    fn harness() -> Harness {
        let (held_tx, held) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let store = Arc::new(GatedStore {
            armed: AtomicBool::new(false),
            held: parking_lot::Mutex::new(held_tx),
            release: parking_lot::Mutex::new(release_rx),
        });
        let probe_action: ProbeAction = Arc::default();
        let action = Arc::clone(&probe_action);
        let approvals = ApprovalService::with_store(store.clone())
            .expect("service")
            .with_reservation_probe(Arc::new(move || {
                let action = action.lock().take();
                if let Some(action) = action {
                    action();
                }
            }));
        let owner = Arc::new(Owner::default());
        let work = WorkAuthorizationContext::new(
            Arc::new(OwnerWork(Arc::clone(&owner))),
            OperationExecutionScope::Domain,
        );
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
        let check = PreparedOperationCheck::prepare(work.clone(), binding).expect("prepared");
        Harness {
            approvals,
            store,
            held,
            release,
            probe_action,
            owner,
            work,
            check,
        }
    }

    fn approval_request() -> ApprovalRequest {
        ApprovalRequest {
            requester: ApprovalPrincipalId::new("human:alice").expect("principal"),
            owner: ApprovalOwnerRef::Session {
                session_id: SessionId::new(),
            },
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::ShellCommand,
                id: ApprovalResourceId::new("shell:rm"),
            },
            proposed_action: ApprovalProposedAction {
                kind: ApprovalActionKind::ShellCommand,
                summary: "run destructive command".to_string(),
                body: None,
            },
            risk: ApprovalRisk::High,
            request_body: None,
            allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
                .into_iter()
                .collect(),
            expires_at: None,
            metadata: crate::SurfaceMetadata::default(),
            request_provenance: None,
        }
    }

    impl Harness {
        fn review(
            &self,
            reviewer: Arc<dyn OperationReviewer>,
            deadline: Duration,
        ) -> (Arc<BoundOperationReview>, Arc<Recording>) {
            let observer = Arc::new(Recording::default());
            let review = Arc::new(
                BoundOperationReview::new(reviewer, self.approvals.clone(), deadline)
                    .with_observer(observer.clone()),
            );
            (review, observer)
        }

        fn on_probe(&self, action: impl FnOnce() + Send + 'static) {
            *self.probe_action.lock() = Some(Box::new(action));
        }

        /// A human decision takes the owner's commit reservation and holds
        /// it inside its store write until released (at most `BOUND`).
        fn hold_reservation(&self) -> std::thread::JoinHandle<()> {
            let id = self
                .approvals
                .request(approval_request())
                .expect("approval request")
                .approval_id;
            self.store.armed.store(true, Ordering::Release);
            let approvals = self.approvals.clone();
            let holder = std::thread::spawn(move || {
                approvals
                    .decide(
                        &id,
                        ApprovalDecision::Approve,
                        ApprovalPrincipalId::new("human:bob").expect("principal"),
                        None,
                        None,
                    )
                    .expect("decided");
            });
            self.held
                .recv_timeout(BOUND)
                .expect("decide holds the reservation");
            holder
        }

        fn release(&self, holder: std::thread::JoinHandle<()>) {
            let _ = self.release.send(());
            holder.join().expect("holder thread");
        }
    }

    async fn admit(
        review: &Arc<BoundOperationReview>,
        check: &PreparedOperationCheck,
        work: &WorkAuthorizationContext,
    ) -> Result<ReviewEntry, crate::ToolError> {
        let args = serde_json::value::RawValue::from_string("{}".into()).expect("args");
        let call = ToolCallView {
            id: "call",
            name: "tool",
            args: &args,
        };
        admit_operation_review(
            Some(review),
            ReviewEntrySupport::ConsumesAtEntry,
            call,
            check,
            work,
        )
        .await
    }

    fn owner_unavailable() -> crate::ToolError {
        crate::ToolError::ReviewUnavailable {
            kind: ReviewUnavailableKind::OwnerUnavailable,
        }
    }

    /// The final check runs after the reservation is taken: an owner change
    /// at the reservation point refuses the spend (no Used), and the
    /// unchanged control spends.
    #[tokio::test]
    async fn spend_rechecks_currentness_after_taking_the_reservation() {
        for change in [false, true] {
            let h = harness();
            let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
            let entry = admit(&review, &h.check, &h.work)
                .await
                .expect("allowed and retained");
            let guard = ReviewDispatchGuard::new(entry.clone());
            if change {
                let owner = Arc::clone(&h.owner);
                h.on_probe(move || {
                    owner.sequence.fetch_add(1, Ordering::AcqRel);
                });
            }
            let result = entry.consume(&h.check);
            guard.complete();
            if change {
                assert_eq!(
                    result,
                    Err(crate::ToolError::ReviewUnsatisfied {
                        kind: ReviewUnsatisfiedKind::ContextChanged,
                        approval_id: None,
                    })
                );
                assert_eq!(
                    observer.statuses().last().copied(),
                    Some((
                        ReviewAttemptStatus::Retired,
                        Some(ReviewRetirementReason::ContextChanged)
                    ))
                );
                assert!(!observer.used());
            } else {
                assert_eq!(result, Ok(()));
                assert_eq!(
                    observer.statuses().last().copied(),
                    Some((ReviewAttemptStatus::Used, None))
                );
            }
        }
    }

    /// R2's final check under the reservation is the retained decision's
    /// coherent current check only: a permission withdrawn between staging
    /// and the spend refuses there, and its observation (owner code that
    /// re-enters the approval owner) runs only after every lock is released.
    /// The unchanged control spends.
    #[tokio::test]
    async fn a_refused_spend_is_observed_only_after_the_reservation() {
        for withdrawn in [true, false] {
            let h = harness();
            let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
            let entry = admit(&review, &h.check, &h.work)
                .await
                .expect("allowed and retained");
            let reentered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let approvals = h.approvals.clone();
            let count = Arc::clone(&reentered);
            *h.owner.on_observe.lock() = Some(Arc::new(move || {
                approvals
                    .list(crate::approval::ApprovalListFilter::default())
                    .expect("the owner is free while a refusal is observed");
                count.fetch_add(1, Ordering::SeqCst);
            }));
            h.owner.denied.store(withdrawn, Ordering::Release);

            let (done, spent) = mpsc::channel();
            let check = h.check.clone();
            let spender = std::thread::spawn(move || {
                let guard = ReviewDispatchGuard::new(entry.clone());
                let result = entry.consume(&check);
                guard.complete();
                let _ = done.send(result);
            });
            let result = spent
                .recv_timeout(BOUND)
                .expect("no deadlock: the observation runs outside every lock");
            spender.join().expect("spender");
            if withdrawn {
                assert_eq!(
                    result,
                    Err(crate::ToolError::AuthorizationRefused {
                        refusal: OperationRefused::new(OperationRefusalKind::Denied),
                    })
                );
                assert_eq!(reentered.load(Ordering::SeqCst), 1, "observed once");
                assert_eq!(
                    observer.statuses().last().copied(),
                    Some((
                        ReviewAttemptStatus::Retired,
                        Some(ReviewRetirementReason::AuthorityChanged)
                    ))
                );
                assert!(!observer.used());
            } else {
                assert_eq!(result, Ok(()));
                assert_eq!(reentered.load(Ordering::SeqCst), 0);
                assert!(observer.used());
            }
        }
    }

    /// The deadline is re-checked after the reservation is taken: a deadline
    /// that passes at the reservation point refuses (no Used).
    #[tokio::test]
    async fn spend_rechecks_the_deadline_after_taking_the_reservation() {
        let h = harness();
        let (review, observer) = h.review(Arc::new(ImmediateAllow), Duration::from_millis(200));
        let entry = admit(&review, &h.check, &h.work)
            .await
            .expect("allowed and retained");
        let guard = ReviewDispatchGuard::new(entry.clone());
        h.on_probe(|| std::thread::sleep(Duration::from_millis(300)));
        let result = entry.consume(&h.check);
        guard.complete();
        assert_eq!(
            result,
            Err(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::DeadlineExpired,
            })
        );
        assert!(!observer.used());
    }

    /// Verdict admission re-checks currentness after taking the reservation:
    /// an owner change at that point is never recorded as an allow.
    #[tokio::test]
    async fn verdict_rechecks_currentness_after_taking_the_reservation() {
        let h = harness();
        let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let owner = Arc::clone(&h.owner);
        h.on_probe(move || {
            owner.sequence.fetch_add(1, Ordering::AcqRel);
        });
        let result = admit(&review, &h.check, &h.work).await;
        assert_eq!(
            result.err(),
            Some(crate::ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::ContextChanged,
                approval_id: None,
            })
        );
        assert_eq!(
            observer.statuses(),
            vec![
                (ReviewAttemptStatus::Pending, None),
                (
                    ReviewAttemptStatus::Retired,
                    Some(ReviewRetirementReason::ContextChanged)
                ),
            ]
        );
    }

    /// A decision that went stale at the verdict's own commit is superseded,
    /// not settled, once the owner's re-prepare returns it current: the
    /// dispatch's one supersession decision admits the successor, whose new
    /// attempt is reviewed from the start and allowed.
    #[tokio::test]
    async fn a_verdict_commit_superseded_by_a_completed_change_is_reviewed_once_more() {
        let h = harness();
        let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let owner = Arc::clone(&h.owner);
        h.on_probe(move || {
            owner.sequence.fetch_add(1, Ordering::AcqRel);
        });
        let args = serde_json::value::RawValue::from_string("{}".into()).expect("args");
        let call = ToolCallView {
            id: "call",
            name: "tool",
            args: &args,
        };
        let superseded = match admit_operation_review_attempt(
            Some(&review),
            ReviewEntrySupport::ConsumesAtEntry,
            call,
            &h.check,
            &h.work,
            None,
            &mut OwnerChangeChurn::new(),
        )
        .await
        {
            Err(ReviewAdmissionRefusal::Superseded(superseded)) => superseded,
            other => unreachable!("the stale verdict commit is superseded: {other:?}"),
        };
        assert_eq!(superseded.outcome, SupersededOutcome::Allowed);
        assert!(!superseded.successor.same_check(&h.check));
        let readmission = ReviewSupersession::new()
            .readmit(Some(&review), *superseded, None)
            .expect("one more review");
        let entry = admit_operation_review_attempt(
            Some(&review),
            ReviewEntrySupport::ConsumesAtEntry,
            call,
            &readmission.successor,
            &h.work,
            Some(readmission.material),
            &mut OwnerChangeChurn::new(),
        )
        .await
        .expect("the successor is reviewed and its allow retained");
        assert_eq!(
            observer.statuses(),
            vec![
                (ReviewAttemptStatus::Pending, None),
                (
                    ReviewAttemptStatus::Retired,
                    Some(ReviewRetirementReason::ContextChanged)
                ),
                (ReviewAttemptStatus::Pending, None),
                (ReviewAttemptStatus::Allowed, None),
            ]
        );
        drop(entry);
    }

    /// A held owner is never waited for: the spend refuses locally at once
    /// (typed OwnerUnavailable) while the unrelated decide still holds it.
    /// The disposal is queued and the holder's release settles it.
    #[tokio::test]
    async fn contended_spend_refuses_at_once_while_the_owner_is_held() {
        let h = harness();
        let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let entry = admit(&review, &h.check, &h.work)
            .await
            .expect("allowed and retained");
        let guard = ReviewDispatchGuard::new(entry.clone());
        let holder = h.hold_reservation();
        let result = entry.consume(&h.check);
        assert!(!holder.is_finished(), "refused while the owner was held");
        assert_eq!(result, Err(owner_unavailable()));
        guard.complete();
        assert!(!observer.used());
        // The holder's own release settles the queued disposal and reports
        // it: no later owner call is needed.
        h.release(holder);
        assert_eq!(
            observer.statuses().last().copied(),
            Some((
                ReviewAttemptStatus::Retired,
                Some(ReviewRetirementReason::Abandoned)
            ))
        );
    }

    /// Verdict admission against a held owner refuses at once too; the
    /// allow is never recorded.
    #[tokio::test]
    async fn contended_verdict_refuses_at_once_while_the_owner_is_held() {
        let h = harness();
        let (reviewing_tx, mut reviewing) = tokio::sync::mpsc::unbounded_channel();
        let (proceed, proceed_rx) = tokio::sync::oneshot::channel();
        let (review, observer) = h.review(
            Arc::new(GatedAllow {
                reviewing: reviewing_tx,
                proceed: parking_lot::Mutex::new(Some(proceed_rx)),
            }),
            DEADLINE,
        );
        let (check, work) = (h.check.clone(), h.work.clone());
        let admission = tokio::spawn(async move { admit(&review, &check, &work).await });
        reviewing.recv().await.expect("reviewer invoked");
        let holder = h.hold_reservation();
        proceed.send(()).expect("proceed");
        let result = admission.await.expect("admission task");
        assert!(!holder.is_finished(), "refused while the owner was held");
        assert_eq!(result.err(), Some(owner_unavailable()));
        assert_eq!(
            observer.statuses(),
            vec![(ReviewAttemptStatus::Pending, None)],
            "the allow was never recorded"
        );
        h.release(holder);
        assert_eq!(
            observer.statuses().last().copied(),
            Some((
                ReviewAttemptStatus::Retired,
                Some(ReviewRetirementReason::Abandoned)
            ))
        );
    }

    /// Beginning a review against a held owner refuses at once (typed),
    /// creates no attempt (the owner lock was never taken) and observes no
    /// Pending.
    #[tokio::test]
    async fn begin_refuses_at_once_while_the_owner_is_held() {
        let h = harness();
        let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let holder = h.hold_reservation();
        let before = h.approvals.review_lock_acquisitions.load(Ordering::SeqCst);
        let result = admit(&review, &h.check, &h.work).await;
        assert!(!holder.is_finished(), "refused while the owner was held");
        assert!(
            matches!(&result, Err(error) if *error == owner_unavailable()),
            "{result:?}"
        );
        assert_eq!(
            h.approvals.review_lock_acquisitions.load(Ordering::SeqCst),
            before,
            "begin never took the owner: no attempt was created"
        );
        assert!(observer.statuses().is_empty(), "no attempt, no Pending");
        h.release(holder);
    }

    /// On a current-thread runtime with the owner held, a reviewed admission
    /// never blocks the thread: a healthy sibling completes and a shorter
    /// enclosing timeout is never outlived.
    #[tokio::test(flavor = "current_thread")]
    async fn a_held_owner_never_blocks_the_current_thread_runtime() {
        let h = harness();
        let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let holder = h.hold_reservation();
        let started = std::time::Instant::now();
        let (admission, sibling) = tokio::join!(
            tokio::time::timeout(
                Duration::from_millis(200),
                admit(&review, &h.check, &h.work)
            ),
            async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                "sibling completed"
            }
        );
        assert_eq!(sibling, "sibling completed");
        assert!(started.elapsed() < Duration::from_secs(2), "never blocked");
        match admission {
            Ok(result) => assert!(
                matches!(&result, Err(error) if *error == owner_unavailable()),
                "{result:?}"
            ),
            Err(_elapsed) => {} // the enclosing timeout fired: also never blocked
        }
        assert!(!holder.is_finished(), "the owner was held throughout");
        assert!(!observer.used());
        h.release(holder);
    }

    /// The spent attempt this entry holds as its entered state, if any.
    fn entered_attempt_id(entry: &ReviewEntry) -> Option<String> {
        match &entry.0.lock().state {
            ReviewEntryState::Entered(entered) => {
                entered.handle.as_ref().map(|handle| handle.id().to_owned())
            }
            _ => None,
        }
    }

    /// The generated owner's answer for one later step of `attempt`'s
    /// effect: `Ok(Used)` while it is registered and open. An admitted probe
    /// step is finished at once, which leaves the owner as it was.
    fn step_answer(
        approvals: &ApprovalService,
        attempt: &str,
    ) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
        let admitted =
            approvals.apply_review(|authority| authority.admit_entered_step(attempt.to_owned()));
        if admitted.is_ok() {
            approvals
                .apply_review(|authority| authority.finish_entered_step(attempt.to_owned()))
                .expect("the probe step finishes");
        }
        admitted
    }

    /// A successful spend takes the owner's reservation exactly once. It no
    /// longer disposes the attempt there: the attempt stays registered as
    /// `Used`, the entered state of the effect, and the admitting dispatch's
    /// end releases it. With the owner then held, the effect, its custody
    /// and the dispatch end complete without it: the release is queued, and
    /// applied once the owner is free.
    #[tokio::test]
    async fn successful_spend_takes_the_owner_lock_once_before_the_effect() {
        let h = harness();
        let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let entry = admit(&review, &h.check, &h.work)
            .await
            .expect("allowed and retained");
        let guard = ReviewDispatchGuard::new(entry.clone());
        let before = h.approvals.review_lock_acquisitions.load(Ordering::SeqCst);
        let custody = entry.consume_held(&h.check).expect("spend");
        assert_eq!(
            h.approvals.review_lock_acquisitions.load(Ordering::SeqCst) - before,
            1,
            "the spend takes the owner's reservation once"
        );
        let attempt = entered_attempt_id(&entry).expect("the spend leaves its attempt entered");
        assert_eq!(
            step_answer(&h.approvals, &attempt),
            Ok(ReviewAttemptStatus::Used),
            "registered as Used until the dispatch ends"
        );
        let holder = h.hold_reservation();
        drop(custody);
        guard.complete();
        assert_eq!(
            observer.statuses().last().copied(),
            Some((ReviewAttemptStatus::Used, None))
        );
        assert_eq!(
            entered_attempt_id(&entry),
            None,
            "the dispatch end handed the release to the owner"
        );
        h.release(holder);
        assert_eq!(
            step_answer(&h.approvals, &attempt),
            Err(ReviewOwnerError::Rejected(
                crate::generated::approval_lifecycle::ApprovalLifecycleRejectionReason::NotFound
            )),
            "the queued release applied once the owner was free"
        );
    }

    /// This entry's custody of its spent attempt for the effect's later steps.
    fn entered_steps(entry: &ReviewEntry) -> EnteredSteps {
        match entry.entered_basis() {
            EnteredBasis::Entered { steps, .. } => Some(steps),
            EnteredBasis::Unreviewed | EnteredBasis::NotSpent | EnteredBasis::Ended => None,
        }
        .expect("the spend left the effect entered")
    }

    /// At the owner: the admitting dispatch is cancelled
    /// while a step admitted under the entered effect is still in flight (a
    /// store commit blocked inside it). The attempt stays registered as
    /// `Used`, closing: a new step and a second spend are refused, and `Used`
    /// is not observed. The blocked step then completes and its finish
    /// releases the attempt; `Used` is delivered once the effect's custody
    /// is dropped.
    #[tokio::test]
    async fn a_dispatch_cancelled_during_a_step_keeps_the_attempt_until_the_step_ends() {
        use crate::generated::approval_lifecycle::ApprovalLifecycleRejectionReason as Reason;
        let h = harness();
        let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let entry = admit(&review, &h.check, &h.work)
            .await
            .expect("allowed and retained");
        let guard = ReviewDispatchGuard::new(entry.clone());
        let custody = entry.consume_held(&h.check).expect("spend");
        let attempt = entered_attempt_id(&entry).expect("the spend leaves its attempt entered");
        let steps = entered_steps(&entry);
        let blocked = steps
            .admit(h.check.binding(), &h.work)
            .expect("a step while the dispatch is live");

        drop(guard);
        assert_eq!(
            step_answer(&h.approvals, &attempt),
            Err(ReviewOwnerError::Rejected(Reason::Expired)),
            "still registered as Used and closing"
        );
        assert_eq!(
            steps.admit(h.check.binding(), &h.work).err(),
            Some(ReviewOwnerError::Rejected(Reason::Expired)),
            "a step after the dispatch end refuses as ended"
        );
        assert_eq!(
            h.approvals
                .apply_review(|authority| authority.consume_review_for_entry(attempt.clone())),
            Err(ReviewOwnerError::Rejected(Reason::AlreadyDecided)),
            "the review is still in use: a second spend refuses"
        );
        assert!(!observer.used(), "Used waits for the work");

        drop(blocked);
        assert_eq!(
            step_answer(&h.approvals, &attempt),
            Err(ReviewOwnerError::Rejected(Reason::NotFound)),
            "the last step's finish released the attempt"
        );
        assert!(!observer.used(), "the effect's custody is still held");
        drop(custody);
        assert_eq!(
            observer
                .statuses()
                .iter()
                .filter(|(status, _)| *status == ReviewAttemptStatus::Used)
                .count(),
            1,
            "Used once, after the work"
        );
    }

    /// The dispatch end's hand-back, queued behind a held owner and not yet
    /// settled when the owner frees (seeded under a raw guard, as a reader's
    /// race leaves it), is settled into the owner by the next step's own
    /// reservation before the admission: the generated owner alone decides,
    /// and no step is admitted after the dispatch ended.
    #[tokio::test]
    async fn a_queued_dispatch_end_is_settled_before_a_step_is_admitted() {
        use crate::generated::approval_lifecycle::ApprovalLifecycleRejectionReason as Reason;
        let h = harness();
        let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let entry = admit(&review, &h.check, &h.work)
            .await
            .expect("allowed and retained");
        let guard = ReviewDispatchGuard::new(entry.clone());
        let custody = entry.consume_held(&h.check).expect("spend");
        let steps = entered_steps(&entry);
        {
            let state = h.approvals.state.write();
            guard.complete();
            assert_eq!(
                h.approvals.queued_disposals.lock().len(),
                1,
                "the hand-back is queued behind the held owner"
            );
            drop(state);
        }
        assert_eq!(
            h.approvals.queued_disposals.lock().len(),
            1,
            "nothing settled it yet"
        );
        assert_eq!(
            steps.admit(h.check.binding(), &h.work).err(),
            Some(ReviewOwnerError::Rejected(Reason::NotFound)),
            "the queued hand-back released the attempt first"
        );
        assert!(h.approvals.queued_disposals.lock().is_empty());
        drop(custody);
        assert!(observer.used());
    }

    /// While the owner is held a step is unavailable at once (never
    /// admitted, never waiting); once the queued dispatch end settles, a
    /// step is refused as ended.
    #[tokio::test]
    async fn a_step_after_a_queued_dispatch_end_is_never_admitted() {
        use crate::generated::approval_lifecycle::ApprovalLifecycleRejectionReason as Reason;
        let h = harness();
        let (review, _observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let entry = admit(&review, &h.check, &h.work)
            .await
            .expect("allowed and retained");
        let guard = ReviewDispatchGuard::new(entry.clone());
        let _custody = entry.consume_held(&h.check).expect("spend");
        let steps = entered_steps(&entry);
        let holder = h.hold_reservation();
        guard.complete();
        assert_eq!(
            steps.admit(h.check.binding(), &h.work).err(),
            Some(ReviewOwnerError::Unavailable),
            "a held owner admits nothing"
        );
        assert!(!holder.is_finished(), "refused while the owner was held");
        h.release(holder);
        assert_eq!(
            steps.admit(h.check.binding(), &h.work).err(),
            Some(ReviewOwnerError::Rejected(Reason::NotFound)),
            "released by the queued hand-back: never admitted after the end"
        );
    }

    /// A step's finish behind a held owner is queued, never lost: with the
    /// dispatch's hand-back queued before or after it, the holder settles
    /// both in order and the attempt is released exactly once, never before
    /// its step finished.
    #[tokio::test]
    async fn a_finish_behind_a_held_owner_is_queued_and_releases_once() {
        use crate::generated::approval_lifecycle::ApprovalLifecycleRejectionReason as Reason;
        for end_first in [false, true] {
            let h = harness();
            let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
            let entry = admit(&review, &h.check, &h.work)
                .await
                .expect("allowed and retained");
            let guard = ReviewDispatchGuard::new(entry.clone());
            let custody = entry.consume_held(&h.check).expect("spend");
            let attempt = entered_attempt_id(&entry).expect("entered");
            let steps = entered_steps(&entry);
            let step = steps
                .admit(h.check.binding(), &h.work)
                .expect("a step while the dispatch is live");
            let holder = h.hold_reservation();
            if end_first {
                guard.complete();
                drop(step);
            } else {
                drop(step);
                guard.complete();
            }
            assert_eq!(
                h.approvals.queued_disposals.lock().len(),
                2,
                "both queued behind the held owner (end first: {end_first})"
            );
            h.release(holder);
            assert!(h.approvals.queued_disposals.lock().is_empty());
            assert_eq!(
                step_answer(&h.approvals, &attempt),
                Err(ReviewOwnerError::Rejected(Reason::NotFound)),
                "released exactly once (end first: {end_first})"
            );
            drop(custody);
            assert!(observer.used());
        }
    }

    /// A step that unwinds still finishes: its guard's drop is the finish,
    /// on every exit path, so the dispatch end then releases the attempt.
    #[tokio::test]
    async fn an_unwinding_step_still_finishes() {
        use crate::generated::approval_lifecycle::ApprovalLifecycleRejectionReason as Reason;
        let h = harness();
        let (review, _observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let entry = admit(&review, &h.check, &h.work)
            .await
            .expect("allowed and retained");
        let guard = ReviewDispatchGuard::new(entry.clone());
        let _custody = entry.consume_held(&h.check).expect("spend");
        let attempt = entered_attempt_id(&entry).expect("entered");
        let steps = entered_steps(&entry);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _step = steps
                .admit(h.check.binding(), &h.work)
                .expect("a step while the dispatch is live");
            std::panic::resume_unwind(Box::new("the step's work failed"));
        }));
        assert!(unwound.is_err());
        guard.complete();
        assert_eq!(
            step_answer(&h.approvals, &attempt),
            Err(ReviewOwnerError::Rejected(Reason::NotFound)),
            "the unwound step finished, so the dispatch end released the attempt"
        );
    }

    /// Reports a step settles with the owner (here a queued disposal's
    /// observer report, settled by the step's own reservation) never run
    /// inside the step or at its finish, which may hold a store's lock: they
    /// run with `Used`, once the dispatch ended and the custody dropped.
    #[tokio::test]
    async fn reports_a_step_settles_run_only_after_the_effect() {
        let h = harness();
        let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let entry = admit(&review, &h.check, &h.work)
            .await
            .expect("allowed and retained");
        let guard = ReviewDispatchGuard::new(entry.clone());
        let custody = entry.consume_held(&h.check).expect("spend");
        let steps = entered_steps(&entry);
        let unrelated = h
            .approvals
            .begin_review(h.check.binding(), &h.work)
            .expect("unrelated attempt");
        let (report_tx, report_rx) = mpsc::channel();
        {
            let state = h.approvals.state.write();
            let (queued, reports) = h.approvals.dispose_review(
                unrelated.id(),
                Some(ReviewRetirementReason::Abandoned),
                move || -> crate::approval::QueuedDisposalReport {
                    Box::new(move |settled| {
                        let _ = report_tx.send(settled);
                    })
                },
            );
            assert!(queued.is_none(), "queued behind the held owner");
            reports.run();
            drop(state);
        }
        let step = steps
            .admit(h.check.binding(), &h.work)
            .expect("a step while the dispatch is live");
        assert!(
            h.approvals.queued_disposals.lock().is_empty(),
            "the step's reservation settled the queue"
        );
        assert!(report_rx.try_recv().is_err(), "no report inside the step");
        drop(step);
        assert!(report_rx.try_recv().is_err(), "nor at its finish");
        guard.complete();
        assert!(report_rx.try_recv().is_err(), "the custody is still held");
        drop(custody);
        assert_eq!(
            report_rx.recv_timeout(BOUND).expect("after the effect"),
            Ok(ReviewAttemptStatus::Retired)
        );
        assert!(observer.used());
    }

    /// A disposal queued DURING a successful spend's own reservation is
    /// settled as that reservation is released, but its report (arbitrary
    /// observer code) runs only after the effect, with `Used`: never between
    /// the spend and the effect.
    #[tokio::test]
    async fn a_report_settled_by_the_spend_runs_only_after_the_effect() {
        let h = harness();
        let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let entry = admit(&review, &h.check, &h.work)
            .await
            .expect("allowed and retained");
        let guard = ReviewDispatchGuard::new(entry.clone());
        let unrelated = h
            .approvals
            .begin_review(h.check.binding(), &h.work)
            .expect("unrelated attempt")
            .id()
            .to_owned();
        let (report_tx, report_rx) = mpsc::channel();
        // At the reservation point, arm a hook in the final currentness check
        // (which runs under the spend's reservation): another thread then
        // disposes the unrelated attempt, which must queue.
        // The hook never joins under the reservation: it waits for a
        // bounded completion handshake and keeps the JoinHandle, joined only
        // after the spend released the reservation.
        let approvals = h.approvals.clone();
        let owner = Arc::clone(&h.owner);
        let disposer: Arc<parking_lot::Mutex<Option<std::thread::JoinHandle<bool>>>> =
            Arc::default();
        let handshake_timed_out = Arc::new(AtomicBool::new(false));
        {
            let disposer = Arc::clone(&disposer);
            let handshake_timed_out = Arc::clone(&handshake_timed_out);
            h.on_probe(move || {
                *owner.on_check.lock() = Some(Box::new(move || {
                    let (done_tx, done_rx) = mpsc::channel();
                    let handle = std::thread::spawn(move || {
                        let (queued, reports) = approvals.dispose_review(
                            &unrelated,
                            Some(ReviewRetirementReason::Abandoned),
                            move || -> crate::approval::QueuedDisposalReport {
                                Box::new(move |settled| {
                                    let _ = report_tx.send(settled);
                                })
                            },
                        );
                        reports.run();
                        let _ = done_tx.send(());
                        queued.is_none()
                    });
                    if done_rx.recv_timeout(BOUND).is_err() {
                        handshake_timed_out.store(true, Ordering::Release);
                    }
                    *disposer.lock() = Some(handle);
                }));
            });
        }
        let spent = entry.consume_held(&h.check);
        // The reservation is released: now join, bounded by the handshake.
        let joined = disposer.lock().take().map(|handle| handle.join());
        assert!(
            !handshake_timed_out.load(Ordering::Acquire),
            "disposal never waits for the held reservation"
        );
        assert!(
            matches!(joined, Some(Ok(true))),
            "queued under the spend's reservation: {joined:?}"
        );
        let custody = spent.expect("spend");
        assert!(
            report_rx.try_recv().is_err(),
            "no report between the spend and the effect"
        );
        assert!(!observer.used());
        // The effect runs here; then it ends and the dispatch completes.
        drop(custody);
        guard.complete();
        assert_eq!(
            report_rx
                .recv_timeout(BOUND)
                .expect("report after the effect"),
            Ok(ReviewAttemptStatus::Retired)
        );
        assert_eq!(
            observer.statuses().last().copied(),
            Some((ReviewAttemptStatus::Used, None))
        );
    }

    /// No lost exit: an entry queued while a `decide` holds the owner inside
    /// its store put is settled as that writer releases the lock, and its
    /// terminal report arrives with NO subsequent owner call.
    #[tokio::test]
    async fn an_entry_queued_during_a_held_decide_settles_when_it_releases() {
        let h = harness();
        let attempt = h
            .approvals
            .begin_review(h.check.binding(), &h.work)
            .expect("attempt");
        let holder = h.hold_reservation();
        let (report_tx, report_rx) = mpsc::channel();
        let (queued, reports) = h.approvals.dispose_review(
            attempt.id(),
            Some(ReviewRetirementReason::Abandoned),
            move || -> crate::approval::QueuedDisposalReport {
                Box::new(move |settled| {
                    let _ = report_tx.send(settled);
                })
            },
        );
        assert!(queued.is_none(), "queued while the owner was held");
        reports.run();
        assert!(report_rx.try_recv().is_err(), "not settled while held");
        h.release(holder);
        assert_eq!(
            report_rx.recv_timeout(BOUND).expect("settled on release"),
            Ok(ReviewAttemptStatus::Retired)
        );
    }

    /// An expiry refresh (from `get`) is an owner commit too: an entry queued
    /// while its expiry store put holds the owner settles when it releases,
    /// with no request or decide.
    #[tokio::test]
    async fn an_entry_queued_during_an_expiry_put_settles_when_it_releases() {
        let h = harness();
        let mut expiring = approval_request();
        expiring.expires_at = Some(chrono::Utc::now() + chrono::Duration::milliseconds(50));
        let id = h
            .approvals
            .request(expiring)
            .expect("expiring approval")
            .approval_id;
        let attempt = h
            .approvals
            .begin_review(h.check.binding(), &h.work)
            .expect("attempt");
        std::thread::sleep(Duration::from_millis(100));
        h.store.armed.store(true, Ordering::Release);
        let approvals = h.approvals.clone();
        let holder = std::thread::spawn(move || {
            approvals.get(&id).expect("expiry refresh");
        });
        h.held
            .recv_timeout(BOUND)
            .expect("the expiry put holds the owner");
        let (report_tx, report_rx) = mpsc::channel();
        let (queued, reports) = h.approvals.dispose_review(
            attempt.id(),
            Some(ReviewRetirementReason::Abandoned),
            move || -> crate::approval::QueuedDisposalReport {
                Box::new(move |settled| {
                    let _ = report_tx.send(settled);
                })
            },
        );
        assert!(queued.is_none(), "queued while the owner was held");
        reports.run();
        assert!(report_rx.try_recv().is_err(), "not settled while held");
        h.release(holder);
        assert_eq!(
            report_rx.recv_timeout(BOUND).expect("settled on release"),
            Ok(ReviewAttemptStatus::Retired)
        );
    }

    /// Seed a real queued retirement under a test-held owner guard, then let
    /// the next spend drain it. This isolates callback lock ownership; the
    /// writer/reader interleavings are covered by the adjacent drain tests.
    #[tokio::test]
    async fn queued_disposal_observer_runs_outside_the_entering_review_lock() {
        struct EntryLockProbe {
            entry: std::sync::Weak<parking_lot::Mutex<ReviewEntryInner>>,
            observations: Arc<parking_lot::Mutex<Vec<(ReviewAttemptStatus, bool)>>>,
        }

        impl OperationReviewObserver for EntryLockProbe {
            fn observe(&self, observation: ReviewObservation) {
                if observation.retirement != Some(ReviewRetirementReason::Abandoned) {
                    return;
                }
                let entry = self
                    .entry
                    .upgrade()
                    .expect("entering review still retained");
                // Do not call Debug or take a blocking lock: the old source
                // must record false and fail an assertion instead of hanging.
                let unlocked = entry.try_lock().is_some();
                self.observations
                    .lock()
                    .push((observation.status, unlocked));
            }
        }

        let h = harness();
        let (review, observer) = h.review(Arc::new(ImmediateAllow), DEADLINE);
        let entry = admit(&review, &h.check, &h.work)
            .await
            .expect("allowed and retained");
        let guard = ReviewDispatchGuard::new(entry.clone());
        let observations = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let unrelated_review = Arc::new(
            BoundOperationReview::new(Arc::new(ImmediateAllow), h.approvals.clone(), DEADLINE)
                .with_observer(Arc::new(EntryLockProbe {
                    entry: Arc::downgrade(&entry.0),
                    observations: Arc::clone(&observations),
                })),
        );
        let unrelated = admit(&unrelated_review, &h.check, &h.work)
            .await
            .expect("unrelated allowed review");
        let unrelated_guard = ReviewDispatchGuard::new(unrelated);
        {
            let state = h.approvals.state.write();
            // Actual dispatch abandonment creates the typed queued disposal
            // and its real observer report. Bypass the normal release helper
            // only to seed this unit fixture's next-drain precondition.
            drop(unrelated_guard);
            assert_eq!(h.approvals.queued_disposals.lock().len(), 1);
            assert!(observations.lock().is_empty(), "not reported while queued");
            drop(state);
        }

        let custody = entry
            .consume_held(&h.check)
            .expect("unchanged entry allowed");
        // An unchanged operation may enter. Reports may run before the final
        // check or be deferred through custody; neither timing is required.
        drop(custody);
        guard.complete();
        assert_eq!(
            *observations.lock(),
            vec![(ReviewAttemptStatus::Retired, true)],
            "the real queued observer runs once, outside the entering review lock"
        );
        assert!(observer.used(), "the unchanged entered control completed");
        assert!(h.approvals.queued_disposals.lock().is_empty());
    }

    struct FailedReviewer {
        error: ReviewerFailure,
        owner: Arc<Owner>,
        change_owner: bool,
        calls: AtomicU64,
    }

    #[async_trait]
    impl OperationReviewer for FailedReviewer {
        async fn review(
            &self,
            _candidate: &ReviewCandidate<'_>,
        ) -> Result<ReviewVerdict, ReviewerFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.change_owner {
                self.owner.sequence.fetch_add(1, Ordering::Release);
            }
            Err(self.error)
        }
    }

    #[tokio::test]
    async fn known_observation_failure_precedes_expiry_and_changed_authority() {
        for (deadline, change_owner) in [
            (DEADLINE, false),
            (DEADLINE, true),
            (Duration::ZERO, false),
            (Duration::ZERO, true),
        ] {
            let h = harness();
            let reviewer = Arc::new(FailedReviewer {
                error: ReviewerFailure::ObservationUnavailable(OperationObservationError),
                owner: Arc::clone(&h.owner),
                change_owner,
                calls: AtomicU64::new(0),
            });
            let (review, observer) = h.review(reviewer.clone(), deadline);
            let result = admit(&review, &h.check, &h.work).await;
            // The ready reviewer returns the known error in its first poll.
            // ZERO makes the post-return deadline expired without a sleep.
            assert_eq!(reviewer.calls.load(Ordering::SeqCst), 1);
            assert!(matches!(
                result,
                Err(crate::ToolError::OperationObservationUnavailable)
            ));
            assert_eq!(
                observer.statuses(),
                vec![
                    (ReviewAttemptStatus::Pending, None),
                    (
                        ReviewAttemptStatus::Retired,
                        Some(ReviewRetirementReason::Abandoned)
                    ),
                ]
            );
            assert!(!observer.used());
            assert!(h.approvals.queued_disposals.lock().is_empty());
        }
    }

    #[tokio::test]
    async fn ordinary_reviewer_failure_remains_local_unavailable() {
        let h = harness();
        let reviewer = Arc::new(FailedReviewer {
            error: ReviewerFailure::Unavailable,
            owner: Arc::clone(&h.owner),
            change_owner: false,
            calls: AtomicU64::new(0),
        });
        let (review, observer) = h.review(reviewer.clone(), DEADLINE);
        let result = admit(&review, &h.check, &h.work).await;
        assert!(matches!(
            result,
            Err(crate::ToolError::ReviewUnavailable {
                kind: ReviewUnavailableKind::ReviewerFailed,
            })
        ));
        assert_eq!(reviewer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            observer.statuses().last().copied(),
            Some((ReviewAttemptStatus::Unavailable, None))
        );
        assert!(!observer.used());
    }

    #[test]
    fn source_observation_failure_retains_its_infrastructure_type() {
        assert_eq!(
            ReviewerFailure::from(OperationAuthorizationError::ObservationUnavailable(
                OperationObservationError
            )),
            ReviewerFailure::ObservationUnavailable(OperationObservationError),
        );
        for error in [
            OperationAuthorizationError::Unavailable,
            OperationRefused::new(OperationRefusalKind::Denied).into(),
        ] {
            assert_eq!(ReviewerFailure::from(error), ReviewerFailure::Unavailable);
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod consent_notice_issuer_tests {
    //! The issuer of a review's consent notices is the review's own approval
    //! owner, and only a review that routes R3 to human consent has one.

    use std::time::Duration;

    use super::*;
    use crate::approval::consent::{
        AuthenticatedApprover, ConsentEligibility, ConsentIneligible, ConsentNotice,
        ConsentPresentation, ConsentRouteUnavailable,
    };
    use crate::approval::{ApprovalRecord, ApprovalService};

    /// R3 is never model-reviewed.
    struct NoModelReview;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl OperationReviewer for NoModelReview {
        async fn review(
            &self,
            _candidate: &ReviewCandidate<'_>,
        ) -> Result<ReviewVerdict, ReviewerFailure> {
            Err(ReviewerFailure::Unavailable)
        }
    }

    /// A human consent route this row never reaches.
    struct UnusedRoute;

    impl HumanConsentRoute for UnusedRoute {
        fn present(
            &self,
            _candidate: &ConsentPresentationCandidate<'_>,
        ) -> Result<ConsentPresentation, ConsentRouteUnavailable> {
            Err(ConsentRouteUnavailable)
        }

        fn eligibility(
            &self,
            _approver: &AuthenticatedApprover,
            _approval: &ApprovalRecord,
        ) -> Result<Arc<dyn ConsentEligibility>, ConsentIneligible> {
            Err(ConsentIneligible)
        }

        fn notice(&self, _notice: ConsentNotice) {}
    }

    /// Without a consent route a review has no issuer. With one, the issuer
    /// is the review's own approval owner: an approval that owner records is
    /// the issuer's.
    #[test]
    fn a_reviews_consent_notice_issuer_is_its_own_approval_owner() {
        let approvals = ApprovalService::new();
        let review = BoundOperationReview::new(
            Arc::new(NoModelReview),
            approvals.clone(),
            Duration::from_secs(30),
        );
        assert!(
            review.consent_notice_issuer().is_none(),
            "no consent route, no issuer"
        );
        let review = review.with_human_consent(Arc::new(UnusedRoute), Duration::from_secs(30));
        let issuer = review
            .consent_notice_issuer()
            .expect("a review with a consent route has an issuer");
        let recorded = approvals
            .request(crate::approval::tests::request())
            .expect("the review's owner records an approval");
        assert_eq!(
            issuer
                .get(&recorded.approval_id)
                .expect("the issuer holds it")
                .approval_id,
            recorded.approval_id
        );
    }
}
