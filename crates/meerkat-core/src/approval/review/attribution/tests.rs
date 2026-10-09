use super::*;
use crate::approval::ApprovalService;
use crate::authorization::{
    ModelAuthorizationFacts, PreparedOperationAuthorization, SourceAuthorizationFacts,
    SourceAuthorizationTarget,
};
use crate::exact_operation::OperationExecutionScope;
use crate::memory::MemorySearchScope;
use crate::{LlmRequestAuthorization, SessionId, SessionLlmIdentity};

struct Allowed;

impl PreparedOperationAuthorization for Allowed {
    fn review_tier(&self) -> crate::authorization::OperationReviewTier {
        crate::authorization::OperationReviewTier::R2
    }

    fn check_current(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        Ok(())
    }
}

struct Owner;

impl WorkAuthorization for Owner {
    fn prepare(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        Ok(Arc::new(Allowed))
    }
}

fn fixture() -> (
    ApprovalService,
    super::super::ReviewAttemptHandle,
    ReviewOperationAttribution,
) {
    let work = WorkAuthorizationContext::new(Arc::new(Owner), OperationExecutionScope::Domain);
    let binding = PreparedAuthorizationBinding::new(source_facts());
    let approvals = ApprovalService::new();
    let handle = approvals.try_begin_review(&binding, &work).unwrap();
    let attribution = handle.attribution();
    (approvals, handle, attribution)
}

fn source_facts() -> OperationAuthorizationFacts {
    OperationAuthorizationFacts {
        operation_id: crate::OperationId::new(),
        execution_scope: OperationExecutionScope::Domain,
        run_id: None,
        context_revision: None,
        operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(
                SessionId::new(),
            )),
            usage: SourceAuthorizationUse::Read,
        }),
    }
}

fn model_facts() -> ModelAuthorizationFacts {
    ModelAuthorizationFacts {
        identity: Arc::new(SessionLlmIdentity {
            model: "reviewer".into(),
            provider: crate::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }),
        wire_model: Arc::from("reviewer"),
        hosted_capabilities: Arc::from([]),
        backend_profile_id: None,
        backend_kind: Arc::from("openai"),
        endpoint: Arc::from("https://review.invalid/responses"),
        credential: None,
        usage: ModelAuthorizationUse::ControllerInference,
        live_channel: None,
    }
}

#[test]
fn child_links_retain_the_exact_candidate_and_attempt_without_retagging_it() {
    let (_approvals, handle, attribution) = fixture();
    let original = attribution.candidate_binding().clone();
    let child = attribution
        .context_read_binding(
            &original,
            attribution.work_authorization().authorization().as_ref(),
            source_facts(),
        )
        .unwrap();
    let link = child.review_attribution().unwrap();
    assert!(link.origin().candidate_binding().same_operation(&original));
    assert_eq!(link.origin().attempt_ref(), &handle.attempt_ref());
    assert_eq!(link.role(), ReviewOperationRole::ContextRead);
    assert!(!child.same_operation(&original));
    assert_ne!(child.facts().operation_id, original.facts().operation_id);
    assert!(original.review_attribution().is_none());
    assert!(handle.bound_to(&original, attribution.work_authorization()));
    assert_eq!(format!("{link:?}"), "ReviewChildAttribution([REDACTED])");
}

#[test]
fn equal_candidate_facts_or_another_owner_cannot_relabel_a_context_read() {
    let (_approvals, _handle, attribution) = fixture();
    let lookalike =
        PreparedAuthorizationBinding::new(attribution.candidate_binding().facts().clone());
    assert!(
        attribution
            .context_read_binding(
                &lookalike,
                attribution.work_authorization().authorization().as_ref(),
                source_facts(),
            )
            .is_err()
    );
    let unrelated_owner = Owner;
    assert!(
        attribution
            .context_read_binding(
                attribution.candidate_binding(),
                &unrelated_owner,
                source_facts(),
            )
            .is_err()
    );
}

#[test]
fn child_must_have_fresh_identity_same_coordinates_and_the_declared_role() {
    let (_approvals, _handle, attribution) = fixture();
    let mutations: [fn(&mut OperationAuthorizationFacts); 4] = [
        |facts| {
            facts.execution_scope = OperationExecutionScope::RuntimeInput {
                owner_session_id: SessionId::new(),
                runtime_epoch_id: crate::RuntimeEpochId::new(),
                submitted_input_id: crate::InputId::new(),
                canonical_input_id: crate::InputId::new(),
            }
        },
        |facts| facts.run_id = Some(crate::RunId::new()),
        |facts| {
            facts.context_revision = Some(
                crate::live_execution::CanonicalContextRevision::from_transcript_revision(
                    "other".into(),
                ),
            );
        },
        |facts| {
            if let AuthorizationOperation::Source(source) = &mut facts.operation {
                source.usage = SourceAuthorizationUse::Retain;
            }
        },
    ];
    for mutate in mutations {
        let mut facts = source_facts();
        mutate(&mut facts);
        assert!(
            attribution
                .context_read_binding(
                    attribution.candidate_binding(),
                    attribution.work_authorization().authorization().as_ref(),
                    facts,
                )
                .is_err()
        );
    }
    let mut reused = source_facts();
    reused.operation_id = attribution.candidate_binding().facts().operation_id.clone();
    assert!(
        attribution
            .context_read_binding(
                attribution.candidate_binding(),
                attribution.work_authorization().authorization().as_ref(),
                reused,
            )
            .is_err()
    );
    assert!(
        attribution
            .bind_child(
                source_facts(),
                attribution.work_authorization(),
                ReviewOperationRole::ReviewerInference,
            )
            .is_err()
    );
}

#[test]
fn reviewer_request_cannot_change_origin_coordinates_or_use_a_lookalike_context() {
    let (_approvals, _handle, attribution) = fixture();
    let request = LlmRequestAuthorization::for_operation_review(attribution.clone());
    let prepared = request.prepare(model_facts()).unwrap();
    let link = prepared.binding().review_attribution().unwrap();
    assert_eq!(link.role(), ReviewOperationRole::ReviewerInference);
    assert_eq!(link.origin().attempt_ref(), attribution.attempt_ref());
    assert!(matches!(&prepared.binding().facts().operation,
        AuthorizationOperation::Model(model) if model.usage == ModelAuthorizationUse::Inference));
    assert!(
        request
            .with_coordinates(Some(crate::RunId::new()), None)
            .prepare(model_facts())
            .is_err()
    );
    let lookalike = WorkAuthorizationContext::new(
        Arc::clone(attribution.work_authorization().authorization()),
        attribution.work_authorization().execution_scope().clone(),
    );
    assert!(
        crate::authorization::PreparedOperationCheck::prepare(
            lookalike,
            prepared.binding().clone(),
        )
        .is_err()
    );
}

#[test]
fn ordinary_model_requests_carry_no_review_attribution() {
    let work = WorkAuthorizationContext::new(Arc::new(Owner), OperationExecutionScope::Domain);
    let request = LlmRequestAuthorization::new(
        work,
        crate::OperationId::new(),
        ModelAuthorizationUse::Inference,
    );
    assert!(
        request
            .prepare(model_facts())
            .unwrap()
            .binding()
            .review_attribution()
            .is_none()
    );
}

#[tokio::test]
async fn started_observation_precedes_review_and_failure_keeps_the_reviewer_unentered() {
    use super::super::{
        BoundOperationReview, OperationReviewObserver, OperationReviewer, ReviewAttemptStatus,
        ReviewCandidate, ReviewEntrySupport, ReviewObservation, ReviewRetirementReason,
        ReviewVerdict, ReviewerFailure, admit_operation_review,
    };
    use crate::authorization::{
        OperationObservation, OperationObservationError, PreparedOperationCheck,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Staging {
        fail: bool,
        started: parking_lot::Mutex<Vec<ReviewAttemptRef>>,
        calls: AtomicUsize,
        settled: parking_lot::Mutex<Vec<ReviewObservation>>,
    }
    struct StagingDecision(Arc<Staging>);
    impl PreparedOperationAuthorization for StagingDecision {
        fn review_tier(&self) -> crate::authorization::OperationReviewTier {
            crate::authorization::OperationReviewTier::R2
        }
        fn check_current(
            &self,
            _: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            Ok(())
        }
        fn observe(
            &self,
            _: &PreparedAuthorizationBinding,
            event: OperationObservation,
        ) -> Result<(), OperationObservationError> {
            if let OperationObservation::ReviewAttemptStarted { attempt_ref } = event {
                if self.0.fail {
                    return Err(OperationObservationError);
                }
                self.0.started.lock().push(attempt_ref);
            }
            Ok(())
        }
    }
    struct StagingWork(Arc<Staging>);
    impl WorkAuthorization for StagingWork {
        fn prepare(
            &self,
            _: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Ok(Arc::new(StagingDecision(Arc::clone(&self.0))))
        }
    }
    impl OperationReviewObserver for Staging {
        fn observe(&self, observation: ReviewObservation) {
            self.settled.lock().push(observation);
        }
    }
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl OperationReviewer for Staging {
        async fn review(
            &self,
            candidate: &ReviewCandidate<'_>,
        ) -> Result<ReviewVerdict, ReviewerFailure> {
            assert_eq!(
                self.started.lock().as_slice(),
                &[candidate.attribution().attempt_ref().clone()]
            );
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ReviewVerdict::Deny)
        }
    }
    for fail in [true, false] {
        let staging = Arc::new(Staging {
            fail,
            started: parking_lot::Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            settled: parking_lot::Mutex::new(Vec::new()),
        });
        let work = WorkAuthorizationContext::new(
            Arc::new(StagingWork(Arc::clone(&staging))),
            OperationExecutionScope::Domain,
        );
        let prepared = PreparedOperationCheck::prepare(
            work.clone(),
            PreparedAuthorizationBinding::new(source_facts()),
        )
        .unwrap();
        let review = Arc::new(
            BoundOperationReview::new(
                staging.clone(),
                ApprovalService::new(),
                std::time::Duration::from_secs(5),
            )
            .with_observer(staging.clone()),
        );
        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        let call = crate::ToolCallView {
            id: "staging",
            name: "tool",
            args: &args,
        };
        let lookalike = WorkAuthorizationContext::new(
            Arc::clone(work.authorization()),
            work.execution_scope().clone(),
        );
        assert!(matches!(
            admit_operation_review(Some(&review), ReviewEntrySupport::ConsumesAtEntry, call, &prepared, &lookalike).await,
            Err(crate::ToolError::AuthorizationRefused { refusal })
                if refusal.kind() == OperationRefusalKind::MalformedFacts,
        ));
        assert_eq!(staging.calls.load(Ordering::SeqCst), 0);
        assert!(staging.started.lock().is_empty());
        assert!(staging.settled.lock().is_empty());
        let result = admit_operation_review(
            Some(&review),
            ReviewEntrySupport::ConsumesAtEntry,
            call,
            &prepared,
            &work,
        )
        .await;
        if fail {
            assert!(matches!(
                result,
                Err(crate::ToolError::OperationObservationUnavailable)
            ));
            assert_eq!(staging.calls.load(Ordering::SeqCst), 0);
            assert!(staging.started.lock().is_empty());
            let settled = staging.settled.lock();
            assert_eq!(settled.len(), 2);
            assert_eq!(settled[0].status, ReviewAttemptStatus::Pending);
            assert_eq!(settled[1].status, ReviewAttemptStatus::Retired);
            assert_eq!(
                settled[1].retirement,
                Some(ReviewRetirementReason::Abandoned)
            );
            assert_eq!(settled[0].attempt, settled[1].attempt);
        } else {
            assert!(matches!(
                result,
                Err(crate::ToolError::ReviewUnsatisfied {
                    kind: super::super::ReviewUnsatisfiedKind::Denied,
                })
            ));
            assert_eq!(staging.calls.load(Ordering::SeqCst), 1);
        }
    }
}
