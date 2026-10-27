//! Test support, hidden: one routed consent of an approval owner, parked
//! through the owner's real paths only, for compositions outside this crate
//! that need live parked custody (the consent notice delivery controls).
//!
//! The consent is issued by the review admission of one R3 operation in a
//! routed run, decided through the authenticated consent decision, and
//! parked by that run's close. No row is constructed, nothing is written to
//! the store directly and no owner state is edited. A resumed run of its
//! origin work is opened, and the consent spent in it, through the review's
//! own run open and consent attempt only.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::consent::{
    AuthenticatedApprover, AuthenticatedConsentDecision, ConsentEligibility, ConsentIneligible,
    ConsentNotice, ConsentNoticeKind, ConsentPresentation, ConsentPresentationCandidate,
    ConsentRouteUnavailable, HumanConsentRoute,
};
use super::review::{
    BoundOperationReview, OperationReviewer, ReviewCandidate, ReviewDispatchGuard,
    ReviewEntrySupport, ReviewVerdict, ReviewerFailure, admit_consent_attempt,
    admit_operation_review,
};
use super::{
    ApprovalAssurance, ApprovalConsentCustody, ApprovalConsentScope, ApprovalDecision, ApprovalId,
    ApprovalPrincipalId, ApprovalRecord, ApprovalRisk, ApprovalService,
};
use crate::authorization::{
    AuthorizationOperation, OperationAuthorizationError, OperationAuthorizationFacts,
    OperationReviewTier, PreparedAuthorizationBinding, PreparedOperationAuthorization,
    PreparedOperationCheck, ReviewOperationContextResolution, SourceAuthorizationFacts,
    SourceAuthorizationTarget, SourceAuthorizationUse, WorkAuthorization, WorkAuthorizationContext,
};
use crate::exact_operation::OperationExecutionScope;
use crate::lifecycle::{InputId, RunId};
use crate::memory::MemorySearchScope;
use crate::ops::OperationId;
use crate::retained_work::{ResumedInput, RetainedWorkIdentity};
use crate::types::SessionId;
use crate::{PrincipalKind, PrincipalRef, SurfaceMetadata, ToolCallView, TrustDomainId};

/// The only principal the route lets decide.
const APPROVER: &str = "lead-1";
/// Host-chosen validity of the consent request.
const VALIDITY: std::time::Duration = std::time::Duration::from_hours(4);
/// Model review deadline of the review composition (R3 never uses it).
const REVIEW_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// One routed consent of an approval owner, approved by an authenticated
/// human and parked at its admitting run's close, for a resumed run of its
/// origin work. It keeps the review composition and the host route the
/// parked candidate came from, and the destination and origin work it was
/// issued for.
#[doc(hidden)]
pub struct ParkedRoutedConsent {
    approval_id: ApprovalId,
    route: Arc<NoticeRoute>,
    review: Arc<BoundOperationReview>,
    scope: ApprovalConsentScope,
    origin: RetainedWorkIdentity,
}

impl std::fmt::Debug for ParkedRoutedConsent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParkedRoutedConsent")
            .field("approval_id", &self.approval_id)
            .finish_non_exhaustive()
    }
}

impl ParkedRoutedConsent {
    /// Issue one consent for an R3 operation of a run of `origin` whose
    /// notices are routed to `scope`, approve it as an authenticated eligible
    /// human, and close the run, which parks it in `approvals`.
    ///
    /// # Errors
    /// The owner path that did not take its step, described.
    pub async fn park(
        approvals: &ApprovalService,
        scope: ApprovalConsentScope,
        origin: RetainedWorkIdentity,
    ) -> Result<Self, String> {
        let route = Arc::new(NoticeRoute::default());
        let review = Arc::new(
            BoundOperationReview::new(Arc::new(NoModelReview), approvals.clone(), REVIEW_DEADLINE)
                .with_human_consent(Arc::clone(&route) as Arc<dyn HumanConsentRoute>, VALIDITY),
        );
        let work = WorkAuthorizationContext::new(Arc::new(R3Work), OperationExecutionScope::Domain)
            .with_consent_scope(scope.clone())
            .with_retained_work(origin.clone());
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
        let check = PreparedOperationCheck::prepare(work.clone(), binding)
            .map_err(|error| format!("the R3 operation was not prepared: {error:?}"))?;
        let args = serde_json::value::RawValue::from_string("{}".into())
            .map_err(|error| error.to_string())?;
        let call = ToolCallView {
            id: "parked-consent-call",
            name: "send_message",
            args: &args,
        };
        let approval_id = match admit_operation_review(
            Some(&review),
            ReviewEntrySupport::ConsumesAtEntry,
            call,
            &check,
            &work,
        )
        .await
        {
            Err(crate::ToolError::ReviewUnsatisfied {
                kind: super::review::ReviewUnsatisfiedKind::HumanConsentRequired,
                approval_id: Some(approval_id),
            }) => approval_id,
            other => {
                return Err(format!(
                    "the R3 call issued no consent approval: {:?}",
                    other.err()
                ));
            }
        };
        let approver = AuthenticatedApprover::new(
            PrincipalRef::in_domain(
                PrincipalKind::Human,
                APPROVER,
                TrustDomainId::new("team").map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?,
            ApprovalAssurance::PersonAuthenticated,
        )
        .map_err(|error| format!("{error:?}"))?;
        approvals
            .decide_consent(
                &approval_id,
                AuthenticatedConsentDecision {
                    approver,
                    decision: ApprovalDecision::Approve,
                    reason: None,
                },
            )
            .map_err(|error| format!("the consent was not approved: {error}"))?;
        review.close_consent_for_work(&work);
        let custody = approvals
            .get(&approval_id)
            .map_err(|error| error.to_string())?
            .consent
            .and_then(|consent| consent.custody);
        if custody != Some(ApprovalConsentCustody::Parked) {
            return Err(format!(
                "the run's close did not park the consent: {custody:?}"
            ));
        }
        Ok(Self {
            approval_id,
            route,
            review,
            scope,
            origin,
        })
    }

    pub fn approval_id(&self) -> &ApprovalId {
        &self.approval_id
    }

    /// The kinds of the host notices this consent's route received, in
    /// order.
    pub fn notices(&self) -> Vec<ConsentNoticeKind> {
        self.route
            .notices
            .lock()
            .iter()
            .filter(|(approval_id, _)| *approval_id == self.approval_id)
            .map(|(_, kind)| *kind)
            .collect()
    }

    /// Open a resumed run of this consent's origin work, for its destination,
    /// staged with `inputs`, each resuming that origin work, through the
    /// review's own run open (as a run opens before its first model call):
    /// parked consent whose committed approved-notice receipt names one of
    /// `inputs` moves into it. Returns the run's work and id, for
    /// [`Self::spend_in`].
    pub fn open_resumed_run(&self, inputs: &[InputId]) -> (WorkAuthorizationContext, RunId) {
        let run_id = RunId::new();
        let work = WorkAuthorizationContext::new(Arc::new(R3Work), OperationExecutionScope::Domain)
            .with_consent_scope(self.scope.clone())
            .with_retained_work(RetainedWorkIdentity::new(
                self.origin.runtime_id(),
                run_id.clone(),
                Vec::new(),
                BTreeMap::new(),
                None,
            ))
            .with_resumed_inputs(
                inputs
                    .iter()
                    .map(|input_id| ResumedInput {
                        input_id: input_id.clone(),
                        resumes: self.origin.clone(),
                    })
                    .collect(),
            );
        self.review.open_consent_for_run(&work, &run_id);
        (work, run_id)
    }

    /// One fresh explicit attempt of this consent in `run`, a resumed run
    /// [`Self::open_resumed_run`] opened, that reaches its leaf and spends
    /// there: the attempt resolves only for that run's own work and id.
    ///
    /// # Errors
    /// The attempt's typed refusal, for example consent already spent.
    pub fn spend_in(
        &self,
        run: &(WorkAuthorizationContext, RunId),
    ) -> Result<(), crate::ToolError> {
        let (work, run_id) = run;
        let (entry, admitted) =
            admit_consent_attempt(Some(&self.review), &self.approval_id, work, Some(run_id))?;
        let guard = ReviewDispatchGuard::new(entry.clone());
        let spent = entry.consume_held(&admitted).map(drop);
        guard.complete();
        spent
    }
}

/// An R3 work owner whose decisions stay current; it has no review
/// material.
struct R3Work;

impl WorkAuthorization for R3Work {
    fn review_operation_context(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<ReviewOperationContextResolution, OperationAuthorizationError> {
        Ok(ReviewOperationContextResolution::NotApplicable)
    }

    fn prepare(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        Ok(Arc::new(R3Decision))
    }
}

struct R3Decision;

impl PreparedOperationAuthorization for R3Decision {
    fn review_tier(&self) -> OperationReviewTier {
        OperationReviewTier::R3
    }

    fn check_current(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        Ok(())
    }
}

/// R3 is never model-reviewed.
struct NoModelReview;

#[async_trait::async_trait]
impl OperationReviewer for NoModelReview {
    async fn review(
        &self,
        _candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure> {
        Err(ReviewerFailure::Unavailable)
    }
}

/// Host route: presents readably, lets only [`APPROVER`] decide, and
/// records notices.
#[derive(Default)]
struct NoticeRoute {
    notices: parking_lot::Mutex<Vec<(ApprovalId, ConsentNoticeKind)>>,
}

/// The approver stays eligible.
struct Eligible;

impl ConsentEligibility for Eligible {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        Ok(())
    }
}

impl HumanConsentRoute for NoticeRoute {
    fn present(
        &self,
        _candidate: &ConsentPresentationCandidate<'_>,
    ) -> Result<ConsentPresentation, ConsentRouteUnavailable> {
        Ok(ConsentPresentation {
            requester: ApprovalPrincipalId::new("human:requester")
                .map_err(|_| ConsentRouteUnavailable)?,
            risk: ApprovalRisk::High,
            metadata: SurfaceMetadata::default(),
        })
    }

    fn eligibility(
        &self,
        approver: &AuthenticatedApprover,
        _approval: &ApprovalRecord,
    ) -> Result<Arc<dyn ConsentEligibility>, ConsentIneligible> {
        if approver.principal().id.as_str() == APPROVER {
            Ok(Arc::new(Eligible))
        } else {
            Err(ConsentIneligible)
        }
    }

    fn notice(&self, notice: ConsentNotice) {
        self.notices.lock().push((notice.approval_id, notice.kind));
    }
}
