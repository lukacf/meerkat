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
//! This is a review-path checkpoint. Qualified human review (R3 and R2
//! escalation routes), durable consent consumption, closed batches and a typed
//! host action reference are not implemented; R3 and escalation settle locally.
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
use crate::approval::{RefusedReviewCommit, SettledReports};
use crate::authorization::{
    OperationAuthorizationFacts, OperationObservationError, PolicyPublicationObservation,
    PreparedAuthorizationBinding, PreparedOperationCheck, WorkAuthorizationContext,
};

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
            OperationReviewRefusal::Unsatisfied { kind } => Self::ReviewUnsatisfied { kind },
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
}

impl<'a> ReviewCandidate<'a> {
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
/// outside the approval owner; the raw id never leaves core.
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

    fn attribution(&self) -> ReviewOperationAttribution {
        ReviewOperationAttribution::from_attempt(self)
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
    AuthorityChanged(crate::ToolError),
    ContextChanged,
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
}

impl fmt::Debug for BoundOperationReview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundOperationReview")
            .field("deadline", &self.deadline)
            .field("observer", &self.observer.is_some())
            .finish_non_exhaustive()
    }
}

fn unsatisfied(kind: ReviewUnsatisfiedKind) -> crate::ToolError {
    crate::ToolError::ReviewUnsatisfied { kind }
}

fn unavailable(kind: ReviewUnavailableKind) -> crate::ToolError {
    crate::ToolError::ReviewUnavailable { kind }
}

/// Observations and settled reports of unrelated disposals, collected under
/// a lock and delivered after it is released: observations first, then the
/// reports (arbitrary observer code, so never under a lock).
#[derive(Default)]
#[must_use = "deliver once every lock is released"]
struct PendingObservations {
    observations: Vec<ReviewObservation>,
    reports: SettledReports,
}

impl PendingObservations {
    fn one(observation: ReviewObservation) -> Self {
        Self {
            observations: vec![observation],
            reports: SettledReports::default(),
        }
    }

    fn with_reports(mut self, reports: SettledReports) -> Self {
        self.reports.absorb(reports);
        self
    }

    fn extend(&mut self, other: Self) {
        self.observations.extend(other.observations);
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
        }
    }

    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn OperationReviewObserver>) -> Self {
        self.observer = Some(observer);
        self
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
            reports,
        } = pending;
        if let Some(observer) = self.observer.as_ref() {
            for observation in observations {
                observer.observe(observation);
            }
        }
        reports.run();
    }
}

/// Admission at the common prepared entry, before entry staging.
///
/// R1 proceeds. R3 settles locally. R2 without a composed reviewer, or for a
/// dispatcher that cannot carry review to the tool's entry, is local
/// unavailable feedback before any attempt. Otherwise one bounded review
/// runs; expiry and currentness are validated before a verdict is accepted,
/// and an allow is retained (not spent) for the leaf.
pub(crate) async fn admit_operation_review(
    review: Option<&Arc<BoundOperationReview>>,
    support: ReviewEntrySupport,
    call: ToolCallView<'_>,
    prepared: &PreparedOperationCheck,
    work: &WorkAuthorizationContext,
) -> Result<ReviewEntry, crate::ToolError> {
    match prepared.review_tier() {
        OperationReviewTier::R1 => return Ok(ReviewEntry::not_required()),
        // No qualified human route exists in this checkpoint, and a model
        // allow can never satisfy R3: settle locally with zero entry.
        OperationReviewTier::R3 => {
            return Err(unsatisfied(ReviewUnsatisfiedKind::HumanConsentRequired));
        }
        OperationReviewTier::R2 => {}
    }
    if !prepared.work_authorization().same_context(work) {
        return Err(crate::ToolError::AuthorizationRefused {
            refusal: crate::authorization::OperationRefused::new(
                crate::authorization::OperationRefusalKind::MalformedFacts,
            ),
        });
    }
    let Some(review) = review else {
        return Err(unavailable(ReviewUnavailableKind::ReviewerMissing));
    };
    if support == ReviewEntrySupport::Unsupported {
        return Err(unavailable(ReviewUnavailableKind::UnsupportedEntry));
    }
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
    };

    // Stage the candidate-to-attempt join in the existing protected input
    // audit before the reviewer can read a source or send a model request.
    // Staging failure is infrastructure failure, never a model denial.
    if let Err(error) = prepared.observe_review_attempt_started(attribution.attempt_ref().clone()) {
        let (error, pending) = retained.retire(ReviewRetirementReason::Abandoned, error.into());
        review.deliver(pending);
        return Err(error);
    }

    let candidate = ReviewCandidate {
        call,
        check: prepared,
        attribution,
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
            return Err(error);
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
            return Err(error);
        }
    };

    // Validate before accepting any verdict: a result that is ready only at
    // or after the absolute deadline never wins, and a stale result is
    // retired rather than recorded.
    if retained.expired() {
        let (error, pending) = retained.retire(
            ReviewRetirementReason::DeadlineExpired,
            unavailable(ReviewUnavailableKind::DeadlineExpired),
        );
        review.deliver(pending);
        return Err(error);
    }
    match prepared.current() {
        Err(error) => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::AuthorityChanged,
                crate::ToolError::from(error),
            );
            review.deliver(pending);
            return Err(error);
        }
        Ok(current) if !current.same_check(prepared) => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::ContextChanged,
                unsatisfied(ReviewUnsatisfiedKind::ContextChanged),
            );
            review.deliver(pending);
            return Err(error);
        }
        Ok(_) => {}
    }

    // Every verdict is accepted only under the owner's commit reservation,
    // after the same deadline/currentness checks are repeated there.
    let settled = match verdict {
        Some(ReviewVerdict::Allow) => {
            return match retained.record_verdict(ReviewVerdict::Allow) {
                Ok(pending) => {
                    review.deliver(pending);
                    Ok(ReviewEntry::retained(retained))
                }
                Err((error, pending)) => {
                    review.deliver(pending);
                    Err(error)
                }
            };
        }
        Some(ReviewVerdict::Deny) => retained
            .record_verdict(ReviewVerdict::Deny)
            .map(|pending| (pending, unsatisfied(ReviewUnsatisfiedKind::Denied))),
        Some(ReviewVerdict::Escalate) => retained
            .record_verdict(ReviewVerdict::Escalate)
            .map(|pending| (pending, unsatisfied(ReviewUnsatisfiedKind::Escalated))),
        None => retained
            .record_unavailable()
            .map(|pending| (pending, unavailable(ReviewUnavailableKind::ReviewerFailed))),
    };
    match settled {
        Ok((mut pending, error)) => {
            pending.extend(retained.release());
            review.deliver(pending);
            Err(error)
        }
        Err((error, pending)) => {
            review.deliver(pending);
            Err(error)
        }
    }
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
    Consumed,
    Settled,
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
            ReviewEntryState::Consumed => "ReviewEntry(Consumed)",
            ReviewEntryState::Settled => "ReviewEntry(Settled)",
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

    /// Non-consuming, repeatable check point, run by every entry staging after
    /// its final authorization check with the decision that staging returned.
    pub(crate) fn check(&self, entering: &PreparedOperationCheck) -> Result<(), crate::ToolError> {
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
    pub(crate) fn consume_held(
        &self,
        entering: &PreparedOperationCheck,
    ) -> Result<ReviewedEntryCustody, crate::ToolError> {
        let (result, pending, review) = {
            let mut inner = self.0.lock();
            let review = inner.review.clone();
            let current = std::mem::replace(&mut inner.state, ReviewEntryState::Settled);
            let fresh_spend = matches!(current, ReviewEntryState::Retained(_));
            // `used` carries the spend's `Used` projection and the reports its
            // reservation settled: both run after the effect.
            let (next, result, pending, used) = consume_state(current, entering);
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
        result
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

type Transition = (
    ReviewEntryState,
    Result<(), crate::ToolError>,
    PendingObservations,
);

fn check_state(current: ReviewEntryState, entering: &PreparedOperationCheck) -> Transition {
    use ReviewEntryState::{Consumed, NotRequired, Retained, Settled};
    match (entering.review_tier(), current) {
        (OperationReviewTier::R3, Retained(mut retained)) => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::ContextChanged,
                unsatisfied(ReviewUnsatisfiedKind::HumanConsentRequired),
            );
            (Settled, Err(error), pending)
        }
        (OperationReviewTier::R3, _) => (
            Settled,
            Err(unsatisfied(ReviewUnsatisfiedKind::HumanConsentRequired)),
            PendingObservations::default(),
        ),
        // A decision re-prepared after review (or a changed tier, which only
        // a re-prepared decision can carry) means the reviewed context changed.
        (_, Retained(mut retained)) if !entering.same_check(&retained.admitted) => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::ContextChanged,
                unsatisfied(ReviewUnsatisfiedKind::ContextChanged),
            );
            (Settled, Err(error), pending)
        }
        (OperationReviewTier::R2, Retained(mut retained)) if retained.expired() => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::DeadlineExpired,
                unavailable(ReviewUnavailableKind::DeadlineExpired),
            );
            (Settled, Err(error), pending)
        }
        (OperationReviewTier::R2, Retained(retained)) => {
            (Retained(retained), Ok(()), PendingObservations::default())
        }
        (OperationReviewTier::R1, NotRequired) => {
            (NotRequired, Ok(()), PendingObservations::default())
        }
        (_, Consumed) => (Consumed, Ok(()), PendingObservations::default()),
        (OperationReviewTier::R1, Retained(mut retained)) => {
            let (error, pending) = retained.retire(
                ReviewRetirementReason::ContextChanged,
                unsatisfied(ReviewUnsatisfiedKind::ContextChanged),
            );
            (Settled, Err(error), pending)
        }
        (OperationReviewTier::R2, NotRequired) => (
            Settled,
            Err(unsatisfied(ReviewUnsatisfiedKind::ContextChanged)),
            PendingObservations::default(),
        ),
        (_, Settled) => (
            Settled,
            Err(unavailable(ReviewUnavailableKind::DispatchEnded)),
            PendingObservations::default(),
        ),
    }
}

fn consume_state(
    current: ReviewEntryState,
    entering: &PreparedOperationCheck,
) -> (
    ReviewEntryState,
    Result<(), crate::ToolError>,
    PendingObservations,
    PendingObservations,
) {
    use ReviewEntryState::{Consumed, NotRequired, Retained, Settled};
    match current {
        NotRequired => (
            NotRequired,
            Ok(()),
            PendingObservations::default(),
            PendingObservations::default(),
        ),
        Consumed => (
            Consumed,
            Err(unsatisfied(ReviewUnsatisfiedKind::AlreadyConsumed)),
            PendingObservations::default(),
            PendingObservations::default(),
        ),
        Settled => (
            Settled,
            Err(unavailable(ReviewUnavailableKind::DispatchEnded)),
            PendingObservations::default(),
            PendingObservations::default(),
        ),
        Retained(mut retained) => {
            // Absolute deadline at the spend, then the identity-bound spend.
            if retained.expired() {
                let (error, pending) = retained.retire(
                    ReviewRetirementReason::DeadlineExpired,
                    unavailable(ReviewUnavailableKind::DeadlineExpired),
                );
                return (Settled, Err(error), pending, PendingObservations::default());
            }
            if !entering.same_check(&retained.admitted) {
                let (error, pending) = retained.retire(
                    ReviewRetirementReason::ContextChanged,
                    unsatisfied(ReviewUnsatisfiedKind::ContextChanged),
                );
                return (Settled, Err(error), pending, PendingObservations::default());
            }
            match retained.consume(entering) {
                Ok(used) => (Consumed, Ok(()), PendingObservations::default(), used),
                Err((error, pending)) => {
                    (Settled, Err(error), pending, PendingObservations::default())
                }
            }
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
}

impl RetainedReview {
    fn expired(&self) -> bool {
        tokio::time::Instant::now() >= self.deadline
    }

    /// Run under the owner's commit reservation, immediately before the
    /// conditional commit: the absolute deadline, then the entering decision's
    /// currentness, then its identity with the reviewed decision.
    fn final_check(&self, entering: &PreparedOperationCheck) -> Result<(), FinalCheckFailure> {
        if self.expired() {
            return Err(FinalCheckFailure::DeadlineExpired);
        }
        match entering.current() {
            Err(error) => Err(FinalCheckFailure::AuthorityChanged(error.into())),
            Ok(current)
                if !current.same_check(entering) || !entering.same_check(&self.admitted) =>
            {
                Err(FinalCheckFailure::ContextChanged)
            }
            Ok(_) => Ok(()),
        }
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
    ) -> (crate::ToolError, PendingObservations) {
        let reason = Self::refused_retirement(&refused.error);
        let error = match refused.error {
            ReservedReviewError::Contended => unavailable(ReviewUnavailableKind::OwnerUnavailable),
            ReservedReviewError::FinalCheck(FinalCheckFailure::DeadlineExpired) => {
                unavailable(ReviewUnavailableKind::DeadlineExpired)
            }
            ReservedReviewError::FinalCheck(FinalCheckFailure::AuthorityChanged(error)) => error,
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
        (error, pending.with_reports(refused.reports))
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
    ) -> Result<PendingObservations, (crate::ToolError, PendingObservations)> {
        let Some(handle) = self.handle.as_ref() else {
            return Err((
                unavailable(ReviewUnavailableKind::OwnerUnavailable),
                PendingObservations::default(),
            ));
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
            Err(refused) => Err(self.refused_commit(refused)),
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

    /// Spend the allow once for the exact entering decision and its work.
    /// The spend and the attempt's disposal share one reservation, so nothing
    /// waits on the owner between them and the effect. Returns the deferred
    /// `Used` projection; the caller must not deliver it before the effect.
    fn consume(
        &mut self,
        entering: &PreparedOperationCheck,
    ) -> Result<PendingObservations, (crate::ToolError, PendingObservations)> {
        let Some(handle) = self.handle.as_ref() else {
            return Err((
                unavailable(ReviewUnavailableKind::OwnerUnavailable),
                PendingObservations::default(),
            ));
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
                // Already disposed under the spend's reservation.
                self.handle = None;
                // `Used` and the reports this reservation settled are
                // delivered together, after the effect.
                Ok(PendingObservations::one(used).with_reports(reports))
            }
            Err(refused) => Err(self.refused_commit(refused)),
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
    //! human `decide` holds it through a blocked `ApprovalStore::put`. Review
    //! paths never wait for it (they run on async threads): a held owner
    //! refuses locally at once, and refusal cleanup is queued for the
    //! owner's next commit. Deadline and currentness are re-checked after the
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
            if self.sequence == self.owner.sequence.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into())
            }
        }
    }

    struct OwnerWork(Arc<Owner>);

    impl WorkAuthorization for OwnerWork {
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

    /// Blocks the armed `put` (inside `decide`, under the owner's state
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

        fn put(&self, _record: &ApprovalRecord) -> Result<(), ApprovalStoreError> {
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
        /// it inside `ApprovalStore::put` until released (at most `BOUND`).
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

    /// A successful spend takes the owner's reservation exactly once: the
    /// spend and the attempt's disposal share it. With the owner then held,
    /// the effect, its custody and the dispatch end complete without it.
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
            "the spend and the attempt's disposal share one reservation"
        );
        let holder = h.hold_reservation();
        drop(custody);
        guard.complete();
        assert_eq!(
            observer.statuses().last().copied(),
            Some((ReviewAttemptStatus::Used, None))
        );
        h.release(holder);
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
