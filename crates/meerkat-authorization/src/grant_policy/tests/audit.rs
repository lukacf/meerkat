use super::*;
use meerkat_authorization_contracts::audit::{
    AuditObservation, AuthorizationAuditObservation, AuthorizationAuditSink,
};
use meerkat_core::authorization::{
    OperationAuthorizationError, OperationObservationError, OperationObservedOutcome,
    PreparedOperationCheck,
};

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<AuthorizationAuditObservation>>,
    fail: AtomicBool,
}
impl AuthorizationAuditSink for RecordingSink {
    fn append(
        &self,
        event: AuthorizationAuditObservation,
    ) -> Result<(), OperationObservationError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(OperationObservationError);
        }
        self.events.lock().expect("events").push(event);
        Ok(())
    }
}

#[test]
fn real_compiler_records_grant_observation_refusal_entry_and_deferred_return_without_completion_claim()
 {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let sink = Arc::new(RecordingSink::default());
    let context = policy
        .audited_work_context(vec![association].into(), scope(), sink.clone())
        .expect("audited context");
    let binding = source(SourceAuthorizationUse::Read, "public");
    let check =
        PreparedOperationCheck::prepare(context.clone(), binding).expect("real grant checks");
    check.observe_entry().expect("entry staged");
    let native = meerkat_core::ops::AsyncOpRef::detached(meerkat_core::OperationId::new());
    let returned = Ok(meerkat_core::ops::ToolDispatchOutcome::new(
        meerkat_core::ToolResult::new("call".into(), "accepted".into(), false),
        vec![native.clone()],
        vec![],
    ));
    check
        .observe_outcome(OperationObservedOutcome::from_tool_dispatch(&returned))
        .expect("returned observation");
    let forbidden = source(SourceAuthorizationUse::Retain, "public");
    assert!(PreparedOperationCheck::prepare(context, forbidden).is_err());
    let events = sink.events.lock().expect("events");
    assert_eq!(events.len(), 4);
    let AuditObservation::Prepared { policy, .. } = &events[0].observation else {
        unreachable!()
    };
    assert_eq!(
        policy.operation_authorities,
        vec![fixture.association().candidate().authority_basis.clone()]
    );
    assert!(policy.controller_lineages.is_empty());
    assert!(matches!(events[1].observation, AuditObservation::Entry));
    let AuditObservation::Outcome {
        outcome:
            OperationObservedOutcome::ToolDispatchReturned {
                asynchronous_operations,
                ..
            },
    } = &events[2].observation
    else {
        unreachable!()
    };
    assert_eq!(asynchronous_operations, &[native]);
    assert_eq!(events[0].operation_id, events[2].operation_id);
    assert!(matches!(
        events[3].observation,
        AuditObservation::Refused { .. }
    ));
}

#[test]
fn warm_checks_do_not_append_and_known_entry_failure_keeps_physical_result_separate() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let sink = Arc::new(RecordingSink::default());
    let context = policy
        .audited_work_context(vec![association].into(), scope(), sink.clone())
        .expect("context");
    let check =
        PreparedOperationCheck::prepare(context, source(SourceAuthorizationUse::Read, "public"))
            .expect("prepare");
    for _ in 0..32 {
        check.current().expect("warm current");
    }
    assert_eq!(sink.events.lock().expect("events").len(), 1);
    sink.fail.store(true, Ordering::SeqCst);
    assert_eq!(check.observe_entry(), Err(OperationObservationError));
    let physical: Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> =
        Ok(meerkat_core::ops::ToolDispatchOutcome::sync_result(
            meerkat_core::ToolResult::new("call".into(), "actual result".into(), false),
        ));
    assert_eq!(
        check.observe_outcome(OperationObservedOutcome::from_tool_dispatch(&physical)),
        Err(OperationObservationError)
    );
    assert!(
        matches!(physical, Ok(outcome) if outcome.result.text_content() == "actual result"),
        "original physical result retained"
    );
    assert_eq!(sink.events.lock().expect("events").len(), 1);
}

#[test]
fn final_deadline_denial_is_observed_and_failed_observation_is_infrastructure() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let sink = Arc::new(RecordingSink::default());
    let context = policy
        .audited_work_context(vec![association].into(), scope(), sink.clone())
        .expect("context");
    let binding = source(SourceAuthorizationUse::Read, "public");
    let check = PreparedOperationCheck::prepare(context, binding.clone()).expect("prepare");
    for _ in 0..32 {
        check.current().expect("warm check");
    }
    assert_eq!(sink.events.lock().expect("events").len(), 1);
    fixture.clock.0.store(300, Ordering::SeqCst);
    assert!(
        matches!(check.current(), Err(OperationAuthorizationError::Refused(refusal)) if refusal.kind() == OperationRefusalKind::Denied)
    );
    {
        let events = sink.events.lock().expect("events");
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].operation_id, binding.facts().operation_id);
        assert!(matches!(
            events[1].observation,
            AuditObservation::Refused {
                reason: OperationRefusalKind::Denied,
                ..
            }
        ));
        assert!(!events.iter().any(|event| matches!(
            event.observation,
            AuditObservation::Entry | AuditObservation::Outcome { .. }
        )));
    }
    sink.fail.store(true, Ordering::SeqCst);
    assert!(matches!(
        check.current(),
        Err(OperationAuthorizationError::ObservationUnavailable(
            OperationObservationError
        ))
    ));
    assert_eq!(sink.events.lock().expect("events").len(), 2);
}

#[test]
fn reprepare_after_actual_owner_policy_change_records_refusal_in_current_publication() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, admitted, _) = fixture.policy(association.clone());
    let sink = Arc::new(RecordingSink::default());
    let context = policy
        .audited_work_context(vec![association].into(), scope(), sink.clone())
        .expect("context");
    let check =
        PreparedOperationCheck::prepare(context, source(SourceAuthorizationUse::Read, "public"))
            .expect("prepare");
    {
        let _publication = fixture
            .publication
            .begin_owner_change()
            .expect("publication");
        admitted.requester_allowed.store(false, Ordering::SeqCst);
    }
    assert!(
        matches!(check.current(), Err(OperationAuthorizationError::Refused(refusal)) if refusal.kind() == OperationRefusalKind::Denied)
    );
    let events = sink.events.lock().expect("events");
    assert_eq!(
        events.len(),
        2,
        "old decision must not add a second refusal"
    );
    assert!(matches!(
        events[0].observation,
        AuditObservation::Prepared { .. }
    ));
    assert!(matches!(
        events[1].observation,
        AuditObservation::Refused {
            reason: OperationRefusalKind::Denied,
            ..
        }
    ));
}

// Regression fixtures for the pre-entry observation/error distinction.
// Expected RED before the common policy/infrastructure sum error is added.
#[test]
fn observation_infrastructure_failed_prepared_append_is_not_policy_denial() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let sink = Arc::new(RecordingSink::default());
    sink.fail.store(true, Ordering::SeqCst);
    let context = policy
        .audited_work_context(vec![association].into(), scope(), sink.clone())
        .expect("context");
    let error =
        PreparedOperationCheck::prepare(context, source(SourceAuthorizationUse::Read, "public"))
            .expect_err("unavailable Prepared append prevents entry");
    assert!(sink.events.lock().expect("events").is_empty());
    assert_eq!(
        error,
        OperationAuthorizationError::ObservationUnavailable(OperationObservationError),
        "an allowed operation whose Prepared append failed is infrastructure failure"
    );
}

#[test]
fn observation_infrastructure_failed_refusal_append_is_not_feedback_authority() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let sink = Arc::new(RecordingSink::default());
    sink.fail.store(true, Ordering::SeqCst);
    let context = policy
        .audited_work_context(vec![association].into(), scope(), sink.clone())
        .expect("context");
    let error =
        PreparedOperationCheck::prepare(context, source(SourceAuthorizationUse::Retain, "public"))
            .expect_err("failed Refused append prevents feedback recursion");
    assert!(sink.events.lock().expect("events").is_empty());
    assert_eq!(
        error,
        OperationAuthorizationError::ObservationUnavailable(OperationObservationError),
        "the original policy outcome must not hide unavailable observation custody"
    );
}

#[test]
fn observation_infrastructure_failed_final_refusal_append_is_not_swallowed() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let sink = Arc::new(RecordingSink::default());
    let context = policy
        .audited_work_context(vec![association].into(), scope(), sink.clone())
        .expect("context");
    let check =
        PreparedOperationCheck::prepare(context, source(SourceAuthorizationUse::Read, "public"))
            .expect("prepared while current");
    fixture.clock.0.store(300, Ordering::SeqCst);
    sink.fail.store(true, Ordering::SeqCst);
    let error = check
        .current()
        .expect_err("failed final refusal observation");
    assert_eq!(
        sink.events.lock().expect("events").len(),
        1,
        "only the original Prepared record exists"
    );
    assert_eq!(
        error,
        OperationAuthorizationError::ObservationUnavailable(OperationObservationError),
        "current() must not ignore a failed refusal observation"
    );
}

#[test]
fn owner_review_tier_survives_the_audited_production_chain_and_reprepare() {
    use meerkat_core::authorization::OperationReviewTier;
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, resources) = fixture.policy(association.clone());
    let sink = Arc::new(RecordingSink::default());
    let context = policy
        .audited_work_context(vec![association].into(), scope(), sink)
        .expect("audited context");
    let binding = source(SourceAuthorizationUse::Read, "public");
    let check = PreparedOperationCheck::prepare(context, binding).expect("real grant checks");
    assert_eq!(check.review_tier(), OperationReviewTier::R1);

    // The owner requires R2 under the same publication as its permission.
    let change = fixture
        .publication
        .begin_owner_change()
        .expect("owner publication");
    *resources.review_tier.lock().expect("review tier") = OperationReviewTier::R2;
    drop(change);

    // The retained decision keeps its own tier; the current check re-prepares
    // and carries the freshly resolved tier through every wrapper.
    assert_eq!(check.review_tier(), OperationReviewTier::R1);
    let current = check.current().expect("still permitted");
    assert!(!current.same_check(&check), "owner change re-prepared");
    assert_eq!(current.review_tier(), OperationReviewTier::R2);
}
