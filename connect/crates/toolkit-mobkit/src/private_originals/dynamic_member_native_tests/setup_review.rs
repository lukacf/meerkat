//! Actual Add, native controller capture, approved source selection and input.
//! This component does not execute Find or install a helper. Its review click
//! and unavailable store read are mechanical; admission and source are real.
use super::*;
use crate::installed_host::operator_setup::OperatorParentOutcome;
use crate::private_originals::setup_request::{recovery_tests::RecoveryStore, tests::request};
use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
use meerkat_core::{
    ApprovalDecision, ApprovalPrincipalId, ApprovalRecord, ApprovalRequest, ApprovalService,
};

async fn add(fixture: &Fixture, name: &str) -> (meerkat_core::SessionId, Arc<AdmittedMemberClass>) {
    let admitted = fixture.admit(name).await;
    let context = fixture.native.identity_first_context().unwrap();
    let started = fixture
        .gateway
        .add_member_class(
            &context,
            admitted.clone(),
            spec(name, &class().profile),
            fixture.route(name),
            fixture.intent(name),
            tokio::time::Instant::now() + WAIT,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let outcome = match started {
        MemberClassAddStart::Settled(outcome) => outcome,
        MemberClassAddStart::Unsettled(settlement) => settlement.settled().await.unwrap(),
    };
    assert!(
        outcome.authorization.is_ok() && outcome.cleanup.is_ok(),
        "{outcome:?}"
    );
    assert!(matches!(
        outcome
            .native
            .unwrap()
            .outcomes
            .get(&AgentIdentity::parse(name).unwrap()),
        Some(RestoreOutcome::Created { .. })
    ));
    let session = fixture
        .native
        .identity_runtime()
        .unwrap()
        .status(&AgentIdentity::parse(name).unwrap())
        .await
        .unwrap()
        .session_id
        .unwrap();
    (session, admitted)
}

async fn pending(
    fixture: &Fixture,
    name: &str,
    label: &str,
) -> (LocalControllerSetup, PrivateOriginalSubmit) {
    let mut event = private_event(label);
    event.route = fixture.route(name).id;
    event.content_sha256 = digest(PROMPT.as_bytes());
    let transport_authentication = event.original_authentication.clone();
    let original = fixture.source.retain_event(event).await.unwrap();
    let submission = PrivateOriginalSubmit {
        original,
        content: PROMPT.into(),
        transport_authentication,
    };
    let prepared = fixture
        .gateway
        .prepare_local_controller_setup(submission.clone())
        .await
        .unwrap();
    (prepared, submission)
}

fn approve(request: ApprovalRequest) -> ApprovalRecord {
    let approvals = ApprovalService::new();
    let pending = approvals.request(request).unwrap();
    approvals
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("dynamic-protected-review").unwrap(),
            None,
            None,
        )
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_class_review_selects_same_native_original_and_exact_replay() {
    let fixture = Box::pin(Fixture::start()).await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 5, Box::pin(async {
        let (session, _) = add(&fixture, ADDED).await;
        *fixture.factory.next_call.lock().unwrap() = None;
        // The synthetic provider performs no credential acquisition. Publish
        // its actual actor-selected credential to the same native machine,
        // as in the ordinary dynamic-input fixture; readiness still owns entry.
        let lease = fixture.service.acquire_live_session_actor_turn_boundary_lease(&session).await.unwrap();
        let pin = fixture.service.pin_controller_client_for_actor(&lease).await.unwrap();
        drop(lease);
        meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
            &fixture.machine.generated_auth_lease_handle(), pin.selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-controller-only-fixture"),
            tokio::time::Instant::now() + WAIT,
        ).await.unwrap();
        let policy_before = serde_json::to_vec(fixture.lifetime.selected_policy()).unwrap();
        let (prepared, submission) = pending(&fixture, ADDED, "dynamic-explicit-setup").await;
        assert_eq!(prepared.session_id(), &session);
        assert!(fixture.machine.session_input_states(&session).await.unwrap().report.is_empty());
        let source_key = fixture.source.key(&submission.original).unwrap();
        let owner = fixture.gateway.application_operator_setup_for(&prepared).unwrap();
        let review_request = request(&prepared);
        let outcome = owner.review_original_setup_request(prepared, review_request,
            |request| async move { Ok(approve(request)) }).await.unwrap();
        let OperatorParentOutcome::SetupRequestSelected { selection } = outcome else {
            panic!("the actual dynamic caller did not receive its reviewed source selection")
        };
        assert!(selection.original == submission.original);
        assert_eq!(selection.session_id, session);
        // The existing canonical key has no dynamic/runtime-specific suffix.
        let expected = format!("operator-original-setup-{}", digest(&canonical_json(&(
            "toolkit.operator-application-parent", &source_key.id)).unwrap()));
        assert_eq!(selection.parent[0].grant_id.as_str(), expected);
        let decision_key = StoreKey { realm: source_key.realm.clone(), owner: source_key.owner.clone(), id: expected };
        let decision = fixture.source.store.get(&decision_key).await.unwrap().unwrap();
        let source_before = fixture.source.store.get(&source_key).await.unwrap().unwrap();
        let record = fixture.source.decode_record(&source_before, &submission.original).unwrap();
        assert!(!fixture.source.stock_setup_source(&record, &submission.original).unwrap(),
            "a current dynamic caller is valid but not eligible for static stock cold recovery");
        let mut unknown = submission.original.clone();
        unknown.route = Id::new("unknown-dynamic-setup-route").unwrap();
        assert!(matches!(fixture.source.stock_setup_source(&record, &unknown), Err(PrivateOriginalError::Denied)),
            "an unknown route cannot become an optional-capture decline");
        let (prepared, _) = pending(&fixture, ADDED, "dynamic-explicit-setup").await;
        let review_request = request(&prepared);
        let replay = owner.review_original_setup_request(prepared, review_request,
            |_| async { panic!("an exact approved decision must not present again") }).await.unwrap();
        assert!(matches!(replay, OperatorParentOutcome::SetupRequestSelected { selection: ref saved } if saved == &selection));
        assert_eq!(fixture.source.store.get(&decision_key).await.unwrap(), Some(decision));
        assert_eq!(fixture.source.store.get(&source_key).await.unwrap(), Some(source_before));
        assert!(fixture.machine.session_input_states(&session).await.unwrap().report.is_empty());
        let admitted = fixture.source.submit_authenticated_original(submission.clone()).await.unwrap();
        let PrivateOriginalSubmission::Admitted { turn_ticket: Some(ticket), .. } = admitted else {
            panic!("reviewed original did not enter its actual native session")
        };
        let outcome = fixture.native.identity_runtime().unwrap().wait_for_turn(&AgentIdentity::parse(ADDED).unwrap(),
            TurnTicket::parse(&ticket).unwrap(), WAIT).await;
        assert!(matches!(outcome, TurnOutcome::Completed { output: TurnOutput::Text { text, .. } } if text == ANSWER));
        assert_eq!(fixture.source.bound_input_capture_for_test(&submission.original), (true, Some(false)),
            "warm native admission must not enable static stock cold recovery");
        let binding = fixture.source.binding_evidence(&submission.original).await.unwrap().unwrap();
        let WorkAuthorityBasis::GrantLineage { lineage } = &binding.candidate().authority_basis else {
            panic!("explicit review did not replace the ordinary callback-only basis")
        };
        assert_eq!(&lineage[..1], selection.parent.as_slice());
        assert_eq!(lineage.len(), 2);
        crate::installed_host::operator_setup::validate_bound_setup_selection(&fixture.lifetime,
            &fixture.source.owner, fixture.source.store.as_ref(), &selection, &fixture.source).await.unwrap();
        let facts = fixture.source.installed_setup_facts(&submission.original).await.unwrap();
        assert_eq!(facts.target.as_str(), ADDED);
        assert_eq!(facts.target_member, instance(ADDED).member.logical_owner);
        let requests = fixture.provider.0.load(Ordering::SeqCst);
        assert!(matches!(fixture.source.submit_authenticated_original(submission).await.unwrap(),
            PrivateOriginalSubmission::Admitted { deduplicated: true, .. }));
        assert_eq!(fixture.provider.0.load(Ordering::SeqCst), requests);
        assert_eq!(fixture.machine.session_input_states(&session).await.unwrap().report.len(), 1);
        assert_eq!(serde_json::to_vec(fixture.lifetime.selected_policy()).unwrap(), policy_before);
    }))).catch_unwind().await;
    fixture.close().await;
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("dynamic setup review timed out"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_class_review_refuses_changed_member_after_human_wait() {
    let fixture = Box::pin(Fixture::start()).await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 4,
        Box::pin(async {
            let (session, admitted) = add(&fixture, ADDED).await;
            let (prepared, submission) =
                pending(&fixture, ADDED, "dynamic-review-invalidated").await;
            let key = fixture.source.key(&submission.original).unwrap();
            let before = fixture.source.store.get(&key).await.unwrap().unwrap();
            let owner = fixture
                .gateway
                .application_operator_setup_for(&prepared)
                .unwrap();
            let review_request = request(&prepared);
            let result = owner
                .review_original_setup_request(prepared, review_request, |request| async move {
                    let approved = approve(request);
                    admitted.retire().await.unwrap();
                    Ok(approved)
                })
                .await;
            assert!(matches!(result, Err(HostLifetimeError::Denied)));
            assert!(matches!(
                fixture
                    .source
                    .selected_source_route(&fixture.route(ADDED).id),
                Err(PrivateOriginalError::Denied)
            ));
            assert_eq!(fixture.source.store.get(&key).await.unwrap(), Some(before));
            let decision_key = StoreKey {
                realm: key.realm.clone(),
                owner: key.owner.clone(),
                id: format!(
                    "operator-original-setup-{}",
                    digest(
                        &canonical_json(&("toolkit.operator-application-parent", &key.id)).unwrap()
                    )
                ),
            };
            assert!(
                fixture
                    .source
                    .store
                    .get(&decision_key)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                fixture
                    .source
                    .owner
                    .grants()
                    .issued_reference(&EvidenceId::new(decision_key.id).unwrap())
                    .unwrap()
                    .is_none()
            );
            assert!(
                fixture
                    .machine
                    .session_input_states(&session)
                    .await
                    .unwrap()
                    .report
                    .is_empty()
            );
            assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 0);
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("stale dynamic setup review timed out"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_class_review_keeps_unavailable_and_foreign_selection_refused() {
    let selected = Arc::new(Mutex::new(None));
    let capture = selected.clone();
    let fixture = Box::pin(Fixture::start_selected_source(
        class(),
        None,
        Some(Box::new(move |inner| {
            let fault = RecoveryStore::new(inner);
            *capture.lock().unwrap() = Some(fault.clone());
            fault
        })),
    ))
    .await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 5,
        Box::pin(async {
            let fault = selected.lock().unwrap().as_ref().unwrap().clone();
            let (session, _) = add(&fixture, ADDED).await;
            let (other_session, _) = add(&fixture, "project-refused").await;
            let (prepared, submission) =
                pending(&fixture, ADDED, "dynamic-review-unavailable").await;
            let owner = fixture
                .gateway
                .application_operator_setup_for(&prepared)
                .unwrap();
            let review_request = request(&prepared);
            let OperatorParentOutcome::SetupRequestSelected { selection } = owner
                .review_original_setup_request(prepared, review_request, |request| async move {
                    Ok(approve(request))
                })
                .await
                .unwrap()
            else {
                panic!("review selection missing")
            };
            let key = fixture.source.key(&submission.original).unwrap();
            let before = fixture.source.store.get(&key).await.unwrap().unwrap();
            let decision_key = StoreKey {
                realm: key.realm.clone(),
                owner: key.owner.clone(),
                id: selection.parent[0].grant_id.as_str().to_owned(),
            };
            let (prepared, _) = pending(&fixture, ADDED, "dynamic-review-unavailable").await;
            *fault.fail_get.lock().unwrap() = Some(decision_key);
            assert!(matches!(
                fixture
                    .gateway
                    .select_original_setup_request(&prepared, selection.parent.clone())
                    .await,
                Err(PrivateOriginalError::Unavailable)
            ));
            assert_eq!(
                fixture.source.store.get(&key).await.unwrap(),
                Some(before.clone())
            );
            let mut wrong_session = selection.clone();
            wrong_session.session_id = other_session.clone();
            assert!(matches!(
                crate::installed_host::operator_setup::validate_setup_selection(
                    &fixture.lifetime,
                    &fixture.source.owner,
                    fixture.source.store.as_ref(),
                    &wrong_session,
                    &fixture.source
                )
                .await,
                Err(HostLifetimeError::Denied)
            ));
            let (_, other) = pending(&fixture, "project-refused", "dynamic-review-foreign").await;
            let mut foreign = selection;
            foreign.original = other.original;
            foreign.session_id = other_session.clone();
            assert!(matches!(
                crate::installed_host::operator_setup::validate_setup_selection(
                    &fixture.lifetime,
                    &fixture.source.owner,
                    fixture.source.store.as_ref(),
                    &foreign,
                    &fixture.source
                )
                .await,
                Err(HostLifetimeError::Denied)
            ));
            assert_eq!(fixture.source.store.get(&key).await.unwrap(), Some(before));
            assert!(
                fixture
                    .machine
                    .session_input_states(&session)
                    .await
                    .unwrap()
                    .report
                    .is_empty()
            );
            assert!(
                fixture
                    .machine
                    .session_input_states(&other_session)
                    .await
                    .unwrap()
                    .report
                    .is_empty()
            );
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("dynamic selection refusal timed out"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_class_review_records_entered_issue_before_retired_caller_refusal() {
    let selected = Arc::new(Mutex::new(None));
    let capture = selected.clone();
    let fixture = Box::pin(Fixture::start_selected_source(
        class(),
        None,
        Some(Box::new(move |inner| {
            let fault = RecoveryStore::new(inner);
            *capture.lock().unwrap() = Some(fault.clone());
            fault
        })),
    ))
    .await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 5,
        Box::pin(async {
            let fault = selected.lock().unwrap().as_ref().unwrap().clone();
            let (session, admitted) = add(&fixture, ADDED).await;
            let (prepared, submission) =
                pending(&fixture, ADDED, "dynamic-issued-before-retirement").await;
            let key = fixture.source.key(&submission.original).unwrap();
            let before = fixture.source.store.get(&key).await.unwrap().unwrap();
            let owner = fixture
                .gateway
                .application_operator_setup_for(&prepared)
                .unwrap();
            let request = request(&prepared);
            // Native issue has returned when the existing store receives its
            // acknowledgement. Retire the actual canonical member before that
            // write returns; no later source selection or input is permitted.
            *fault.retire_on_issued.lock().unwrap() = Some(admitted);
            let outcome = owner
                .review_original_setup_request(prepared, request, |request| async move {
                    Ok(approve(request))
                })
                .await;
            assert!(matches!(outcome, Err(HostLifetimeError::Denied)));
            assert!(
                fault.retire_on_issued.lock().unwrap().is_none(),
                "the real issuer's acknowledgement was reached"
            );
            let decision_key = StoreKey {
                realm: key.realm.clone(),
                owner: key.owner.clone(),
                id: format!(
                    "operator-original-setup-{}",
                    digest(
                        &canonical_json(&("toolkit.operator-application-parent", &key.id)).unwrap()
                    )
                ),
            };
            let decision = fixture
                .source
                .store
                .get(&decision_key)
                .await
                .unwrap()
                .unwrap();
            let parent = fixture
                .source
                .owner
                .grants()
                .issued_reference(&EvidenceId::new(decision_key.id.clone()).unwrap())
                .unwrap()
                .expect("the entered native issue must remain observable");
            assert_eq!(decision.data.body["issued"]["state"], "issued");
            assert_eq!(
                decision.data.body["issued"]["lineage"],
                serde_json::to_value(vec![parent]).unwrap()
            );
            assert_eq!(fixture.source.store.get(&key).await.unwrap(), Some(before));
            assert!(
                fixture
                    .machine
                    .session_input_states(&session)
                    .await
                    .unwrap()
                    .report
                    .is_empty()
            );
            assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 0);
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("issued outcome acknowledgement timed out"),
    }
}
