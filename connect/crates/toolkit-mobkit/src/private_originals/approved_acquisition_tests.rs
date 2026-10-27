//! Actual private/local A review and first-input path through the enrolled leaf.
//! The human click and physical model response are scripted; setup selection,
//! native operation admission, WorkGraph/helper input and completion are real.
use super::*;
use crate::installed_host::operator_setup::{OperatorParentOutcome, OperatorParentPurpose};
use meerkat_authorization_contracts::constraints::DelegationDepth;
use meerkat_core::{
    ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
    ApprovalProposedAction, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind,
    ApprovalResourceRef, ApprovalRisk, ApprovalService,
};
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_runtime::input_state::{InputLifecycleState, InputTerminalOutcome, StoredInputState};
use meerkat_runtime::{LogicalRuntimeId, RuntimeStore};
use toolkit_store::ToolkitStore;

type Fixture = FirstFixture<meerkat::PersistentSessionService<FactoryAgentBuilder>>;

pub(super) async fn select_reviewed_setup(fixture: &Fixture) -> (LocalControllerSetup, Vec<GrantLineageRef>) {
    let prepared = fixture
        .source
        .prepare_local_controller_setup(fixture.submission.clone())
        .await
        .unwrap();
    assert_eq!(prepared.session_id(), &fixture.session);
    let owner = fixture
        .source
        .local_operator_setup(fixture.lifetime().clone())
        .unwrap();
    let proposal = owner
        .prepare_original_setup_parent_review(
            &prepared,
            ApprovalRequest {
                requester: ApprovalPrincipalId::new("acquisition-turn-review").unwrap(),
                owner: ApprovalOwnerRef::Session {
                    session_id: prepared.session_id().clone(),
                },
                resource: ApprovalResourceRef {
                    kind: ApprovalResourceKind::Runtime,
                    id: ApprovalResourceId::new("original-setup-request"),
                },
                proposed_action: ApprovalProposedAction {
                    kind: ApprovalActionKind::Other,
                    summary: "The installed owner supplies the exact setup request scope".into(),
                    body: None,
                },
                risk: ApprovalRisk::High,
                request_body: None,
                allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
                    .into_iter()
                    .collect(),
                expires_at: None,
                metadata: Default::default(),
                request_provenance: None,
            },
        )
        .unwrap();
    assert_eq!(
        proposal.request().proposed_action.body.as_ref().unwrap()["purpose"],
        "setup_request"
    );
    let approvals = ApprovalService::new();
    let pending = approvals.request(proposal.request().clone()).unwrap();
    let decided = approvals
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("protected-presenter-fixture").unwrap(),
            None,
            None,
        )
        .unwrap();
    let OperatorParentOutcome::Selected {
        purpose: OperatorParentPurpose::SetupRequest,
        lineage,
        ..
    } = owner
        .apply_parent_decision(proposal, decided)
        .await
        .unwrap()
    else {
        panic!("actual setup-only parent decision must be issued")
    };
    fixture
        .source
        .select_setup_request(&prepared, lineage.clone())
        .await
        .unwrap();
    (prepared, lineage)
}

pub(super) fn scope(fixture: &Fixture) -> toolkit_store::StoreScope {
    toolkit_store::StoreScope {
        realm: fixture.scope.realm.clone(),
        owner: format!("toolkit/acquisition/{}", fixture.scope.owner),
    }
}

pub(super) async fn native_history(
    runtime_store: &dyn RuntimeStore,
    session: &meerkat_core::SessionId,
) -> Vec<StoredInputState> {
    tokio::time::timeout(
        WAIT,
        runtime_store.load_input_states_strict(&LogicalRuntimeId::for_session(session)),
    )
    .await
    .expect("canonical native input history timed out")
    .expect("canonical native input history must decode completely")
}

pub(super) async fn consumed_native_input(
    runtime_store: &dyn RuntimeStore,
    session: &meerkat_core::SessionId,
    association: &InputAuthorityAssociation,
    expected_input_id: Option<&str>,
    deadline: tokio::time::Instant,
) -> StoredInputState {
    let runtime_id = LogicalRuntimeId::for_session(session);
    assert_eq!(association.candidate().target.context.as_str(), session.to_string());
    assert_eq!(association.candidate().target.logical_runtime.as_str(), runtime_id.to_string());
    // Completion notification and canonical input persistence are separate
    // observations. Only the exact admitted row may finish settling here.
    let mut observed_id = expected_input_id.map(str::to_owned);
    tokio::time::timeout_at(deadline, async {
        loop {
            let mut rows = runtime_store.load_input_states_strict(&runtime_id).await
                .expect("canonical native input history must decode completely");
            assert_eq!(rows.len(), 1, "exactly one canonical native input in this session");
            let row = rows.pop().unwrap();
            let input_id = row.state.input_id.to_string();
            match &observed_id {
                Some(expected) => assert_eq!(&input_id, expected, "canonical native input ID changed"),
                None => observed_id = Some(input_id),
            }
            assert_eq!(row.state.authority_contributors.len(), 1, "one exact source association");
            let authority = &row.state.authority_contributors[0];
            assert_eq!(authority.input_id(), &row.state.input_id);
            assert_eq!(authority.association(), association, "canonical source association changed");
            match row.seed.phase {
                InputLifecycleState::Consumed => {
                    assert_eq!(row.seed.terminal_outcome, Some(InputTerminalOutcome::Consumed));
                    assert!(row.seed.last_run_id.is_some(), "completed native run must be retained");
                    return row;
                }
                InputLifecycleState::Accepted
                | InputLifecycleState::Queued
                | InputLifecycleState::Staged
                | InputLifecycleState::Applied
                | InputLifecycleState::AppliedPendingConsumption => {
                    assert_eq!(row.seed.terminal_outcome, None, "nonterminal phase carried a terminal outcome");
                }
                _ => panic!("canonical input did not complete successfully: {:?}", row.seed),
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("canonical native input did not become Consumed before the fixture deadline")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewed_original_runs_enrolled_acquisition_and_one_controller_only_helper() {
    let script = acquisition_denial::Script::for_approved();
    let (fixture, runtime_store) = Fixture::for_approved_acquisition(script.clone()).await;
    let deadline = tokio::time::Instant::now() + WAIT;
    let mut last_stage = "fixture_ready";
    let mut checkpoint = |stage: &'static str| {
        last_stage = stage;
        eprintln!("AA1 stage: {stage}");
    };
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout_at(
        deadline,
        Box::pin(async {
            let helper =
                AgentIdentity::parse(crate::installed_host::controller_first_input_fixture::HELPER)
                    .unwrap();
            let helper_member = MemberIdentity::from(helper.as_str());
            checkpoint("before_helper_session");
            let helper_session = fixture
                .native
                .mob_handle()
                .get_member(&helper_member)
                .await
                .unwrap()
                .unwrap()
                .bridge_session_id()
                .cloned()
                .unwrap();
            checkpoint("after_helper_session");
            assert_ne!(
                fixture.session, helper_session,
                "helper must not target the running A member"
            );
            // Capture BEFORE A can admit its helper. Native rejects an incarnation
            // change; no equal-text or post-admission baseline stands in for completion.
            checkpoint("before_helper_cursor");
            let helper_before = fixture.runtime.completion_cursor(&helper).await;
            checkpoint("after_helper_cursor_before_empty_history");
            assert!(native_history(runtime_store.as_ref(), &helper_session).await.is_empty());
            assert!(native_history(runtime_store.as_ref(), &fixture.session).await.is_empty());
            checkpoint("before_setup_review");
            let (prepared, parent) = select_reviewed_setup(&fixture).await;
            checkpoint("after_setup_review");
            assert!(
                native_history(runtime_store.as_ref(), &fixture.session).await.is_empty(),
                "review and selection do not submit A"
            );
            assert_eq!(fixture.model_requests(), 0);
            let key = fixture.source.key(&fixture.submission.original).unwrap();
            let selected_row = fixture.store.get(&key).await.unwrap().unwrap();
            checkpoint("before_reselect_setup");
            fixture
                .source
                .select_setup_request(&prepared, parent.clone())
                .await
                .unwrap();
            assert_eq!(
                fixture.store.get(&key).await.unwrap().unwrap(),
                selected_row
            );
            assert!(
                fixture
                    .store
                    .list_records(&scope(&fixture), None, 10)
                    .await
                    .unwrap()
                    .is_empty()
            );

            checkpoint("before_a_submit");
            let first = fixture.submit().await;
            checkpoint("after_a_submit");
            let first_input_id = match &first {
                Ok(PrivateOriginalSubmission::Admitted { session_id, input_id, .. }) => {
                    assert_eq!(session_id, &fixture.session.to_string());
                    input_id.clone()
                }
                _ => panic!("actual A admission required: {}", submission_context(&first)),
            };
            checkpoint("before_a_completion");
            fixture.finish(&first).await.unwrap();
            checkpoint("after_a_completion");
            {
                let (feedback, planned, entered) = script.observations();
                eprintln!(
                    "AA1 feedback after A: count={} planned={planned} entered={entered}",
                    feedback.len()
                );
                for item in &feedback {
                    let worker_state = serde_json::from_str::<serde_json::Value>(
                        &item.text_content(),
                    )
                    .ok()
                    .and_then(|reply| {
                        reply.get("worker")?.get("state")?.as_str().map(str::to_owned)
                    });
                    eprintln!(
                        "AA1 feedback: is_error={} worker_state={worker_state:?}",
                        item.is_error
                    );
                }
            }
            let (feedback, planned, entered) = script.observations();
            assert_eq!(
                feedback.len(),
                1,
                "real in-turn acquisition feedback: {feedback:?}"
            );
            assert!(!feedback[0].is_error, "{feedback:?}");
            let reply: serde_json::Value =
                serde_json::from_str(&feedback[0].text_content()).unwrap();
            assert_eq!(reply["created"], true, "{reply}");
            assert_eq!(reply["worker"]["state"], "input_admitted", "{reply}");
            assert_eq!(reply["worker"]["deduplicated"], false, "{reply}");
            assert_eq!(
                reply["worker"]["source_settlement"], "fresh_acceptance_promoted",
                "{reply}"
            );
            assert!(planned > 0, "actual registered leaf resolution required");
            assert_eq!(entered, 1, "actual enrolled leaf must enter exactly once");
            checkpoint("before_helper_completion");
            let helper_after = fixture
                .runtime
                .wait_for_completion(&helper, helper_before, WAIT)
                .await
                .expect("AA1 helper completion failed after confirmed helper input_admitted");
            checkpoint("after_helper_completion");
            assert_ne!(helper_after, helper_before);
            checkpoint("before_binding_readback");
            let binding = fixture.binding().await.unwrap().unwrap();
            checkpoint("after_binding_readback_before_source_checks");
            let WorkAuthorityBasis::GrantLineage { lineage } = &binding.candidate().authority_basis
            else {
                panic!("ordinary first binder did not consume the canonical setup choice")
            };
            assert_eq!(&lineage[..1], parent.as_slice());
            assert_eq!(lineage.len(), 2);
            let resolved = fixture
                .source
                .owner
                .grants()
                .resolve_lineage(
                    lineage,
                    &binding.candidate().logical_executor,
                    binding.candidate().represented_subject.as_ref(),
                )
                .unwrap();
            assert_eq!(
                resolved.restrictions().delegation_depth,
                DelegationDepth::remaining(0)
            );
            resolved.check_current().unwrap();
            assert_eq!(
                binding.candidate().target.context.as_str(),
                fixture.session.to_string()
            );
            assert_ne!(
                lineage.last(),
                binding.candidate().controller_grant_lineage.last()
            );

            let acquisitions = fixture
                .store
                .list_records(&scope(&fixture), None, 10)
                .await
                .unwrap();
            assert_eq!(
                acquisitions.len(),
                1,
                "one canonical acquisition, not a parser-only success"
            );
            assert_eq!(acquisitions[0].key.id, reply["operation"].as_str().unwrap());
            let held = {
                let helpers = fixture.source.helper_sources.lock().unwrap();
                assert_eq!(helpers.len(), 1);
                helpers[0].clone()
            };
            let helper_binding = held.original().association();
            assert_eq!(
                helper_binding.candidate().target.context.as_str(),
                helper_session.to_string()
            );
            assert!(crate::original_ingress::controller_only_basis(
                &helper_binding.candidate().authority_basis
            ));
            assert_eq!(
                helper_binding
                    .candidate()
                    .authority_basis
                    .grant_lineages()
                    .count(),
                0,
                "helper has no service-operation access"
            );
            assert_eq!(helper_binding.candidate().controller_grant_lineage.len(), 2);
            assert_ne!(
                helper_binding.candidate().controller_grant_lineage,
                binding.candidate().controller_grant_lineage,
                "helper uses its independently selected controller parent"
            );
            let facts = fixture
                .source
                .helper_policy_source(helper_binding)
                .unwrap()
                .unwrap();
            assert_eq!(facts.contributors, vec![binding.clone()]);
            assert_eq!(
                fixture
                    .store
                    .get(&held.source_record().key)
                    .await
                    .unwrap()
                    .unwrap(),
                *held.source_record().as_ref()
            );
            checkpoint("before_a_consumed");
            let a_history = consumed_native_input(
                runtime_store.as_ref(), &fixture.session, &binding, Some(&first_input_id), deadline,
            ).await;
            checkpoint("after_a_consumed");
            assert_eq!(a_history.state.input_id.to_string(), first_input_id);
            // The worker reply intentionally omits its input ID. Read the actual
            // admitted ID from the sole canonical row of this helper session,
            // joined to its exact retained source; do not reconstruct an ID.
            checkpoint("before_helper_consumed");
            let helper_history = consumed_native_input(
                runtime_store.as_ref(), &helper_session, helper_binding, None, deadline,
            ).await;
            checkpoint("after_helper_consumed");
            let helper_input_id = helper_history.state.input_id.to_string();
            assert_ne!(helper_history.state.input_id, a_history.state.input_id);
            assert_eq!(
                fixture.model_requests(),
                3,
                "A call/answer and one actual helper controller turn"
            );

            checkpoint("before_replay_submit");
            let replay = fixture.submit().await;
            checkpoint("after_replay_submit_before_completion");
            fixture.finish(&replay).await.unwrap();
            checkpoint("after_replay_completion");
            let (
                Ok(PrivateOriginalSubmission::Admitted {
                    input_id: first_id,
                    turn_ticket: first_ticket,
                    deduplicated: false,
                    source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                    ..
                }),
                Ok(PrivateOriginalSubmission::Admitted {
                    input_id: replay_id,
                    turn_ticket: replay_ticket,
                    deduplicated: true,
                    source_settlement: PrivateSourceSettlement::RetryWithdrawn,
                    ..
                }),
            ) = (&first, &replay)
            else {
                panic!(
                    "actual first admission/replay required: {} / {}",
                    submission_context(&first),
                    submission_context(&replay)
                )
            };
            assert_eq!(first_id, replay_id);
            // Native tickets track deliveries; replay retains one canonical input.
            assert!(first_ticket.is_some());
            assert!(replay_ticket.is_some());
            assert_eq!(
                fixture.runtime.completion_cursor(&helper).await,
                helper_after
            );
            checkpoint("before_a_replay_consumed");
            let a_replay_history = consumed_native_input(
                runtime_store.as_ref(), &fixture.session, &binding, Some(&first_input_id), deadline,
            ).await;
            checkpoint("after_a_replay_consumed");
            checkpoint("before_helper_replay_consumed");
            let helper_replay_history = consumed_native_input(
                runtime_store.as_ref(), &helper_session, helper_binding, Some(&helper_input_id), deadline,
            ).await;
            checkpoint("after_helper_replay_consumed_before_final_checks");
            assert_eq!(a_replay_history.state.input_id, a_history.state.input_id);
            assert_eq!(a_replay_history.seed, a_history.seed);
            assert_eq!(helper_replay_history.state.input_id, helper_history.state.input_id);
            assert_eq!(helper_replay_history.seed, helper_history.seed);
            assert_eq!(
                fixture
                    .store
                    .list_records(&scope(&fixture), None, 10)
                    .await
                    .unwrap(),
                acquisitions
            );
            assert_eq!(fixture.source.helper_sources.lock().unwrap().len(), 1);
            assert_eq!(script.observations().2, 1);
            assert_eq!(fixture.model_requests(), 3);
            assert_eq!(fixture.binding().await.unwrap().unwrap(), binding);
            checkpoint("body_completed");
        }),
    ))
    .catch_unwind()
    .await;
    drop(checkpoint);
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("actual approved acquisition turn exceeded the fixture deadline; last stage: {last_stage}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoked_reviewed_parent_never_enters_acquisition_or_helper() {
    let script = acquisition_denial::Script::for_approved();
    let (fixture, _runtime_store) = Fixture::for_approved_acquisition(script.clone()).await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT,
        Box::pin(async {
            let (_, parent) = select_reviewed_setup(&fixture).await;
            {
                let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
                fixture
                    .source
                    .owner
                    .grants()
                    .revoke(&parent[0].root_authority, &parent[0], &mut custody)
                    .unwrap();
            }
            let first = fixture.submit().await;
            assert!(
                matches!(first, Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)),
                "revoked setup authority must refuse: {}",
                submission_context(&first)
            );
            assert!(fixture.binding().await.unwrap().is_none());
            assert_eq!(fixture.inputs().await.unwrap(), 0);
            assert_eq!(fixture.model_requests(), 0);
            assert_eq!(script.observations().2, 0);
            assert!(fixture.source.helper_sources.lock().unwrap().is_empty());
            assert!(
                fixture
                    .store
                    .list_records(&scope(&fixture), None, 10)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }),
    ))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("revoked approved acquisition control exceeded the fixture deadline"),
    }
}
