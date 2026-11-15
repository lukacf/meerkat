//! Genuine registered R2 Find cancellation around the real SQLite/native step.
//! These controls target the actual c624 Core entered-operation implementation.
//! Accepted graph completion is not a claim that cancelled Find completed.
use super::*;
use crate::private_originals::governed_submit::tests::first_input_tests::dedicated::{
    DedicatedFixture,
    entered_fixture::{BOUND, CheckpointPosition, FIND_CALL, ReviewedWorkflow},
};
use crate::private_originals::setup_workflow::launch::tests::{
    prepare_planning, submit_planning_parent,
};
use futures::{FutureExt, StreamExt};
use meerkat_core::lifecycle::InputId;
use meerkat_core::{
    AgentErrorClass, AgentEvent, SessionServiceHistoryExt, service::SessionHistoryQuery,
};
use meerkat_mobkit::identity_first::{AgentIdentity, TurnOutcome, TurnOutput, TurnTicket};
use meerkat_runtime::{
    CompletionOutcome,
    terminal_status::{InputTerminalReceiptRead, InputTerminalReceiptWait},
};
use meerkat_workgraph::{WorkAdmissionOutcome, WorkGraphEventFilter, WorkGraphEventKind};
use std::panic::AssertUnwindSafe;
use toolkit_store::{EffectStatus, ToolkitStore};

type Fixture = DedicatedFixture<meerkat::PersistentSessionService<meerkat::FactoryAgentBuilder>>;

async fn native_census(fixture: &Fixture) -> serde_json::Value {
    let mut rows = Vec::new();
    for session in [
        &fixture.f.session,
        &fixture.parent_session,
        &fixture.lead_session,
    ] {
        let states = fixture
            .history
            .as_ref()
            .unwrap()
            .load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(session))
            .await
            .unwrap();
        let history = fixture
            .f
            .service
            .read_history(
                session,
                SessionHistoryQuery {
                    offset: 0,
                    limit: Some(100),
                },
            )
            .await
            .unwrap();
        assert!(!history.has_more);
        rows.push(serde_json::json!({"inputs":states.iter().map(|row| &row.state.input_id).collect::<Vec<_>>(),
            "history":history.messages}));
    }
    serde_json::json!(rows)
}

// These owner-written rows were established before commit_selected. Read
// them only after the graph worker joined, outside its source/publication locks.
async fn retained_prior_effects(
    fixture: &Fixture,
    transfer: &Arc<InstalledWorkflowContextTransfer>,
    attempt: &Arc<TransferAttempt>,
) {
    let membership = &attempt.envelope.membership;
    let workflow = fixture
        .f
        .source
        .workflow_store()
        .unwrap()
        .load_setup_workflow(membership.workflow())
        .await
        .unwrap();
    assert!(workflow.terms() == &fixture.terms);
    assert!(workflow.waiters_for_source(membership.source()) == vec![membership.clone()]);
    assert!(!workflow.departed(membership).unwrap());
    assert_eq!(
        membership.source(),
        &fixture
            .f
            .source
            .key(fixture.f.original_reference())
            .unwrap()
    );
    let source = fixture
        .f
        .store
        .get(membership.source())
        .await
        .unwrap()
        .unwrap();
    let source_record = fixture
        .f
        .source
        .decode_record(&source, fixture.f.original_reference())
        .unwrap();
    let original = transfer.original.upgrade().unwrap();
    assert!(source_record.final_association.as_ref() == Some(original.association()));
    let acquisition = fixture
        .f
        .store
        .get(membership.acquisition())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&acquisition.key, membership.acquisition());
    let planning = transfer.planning().unwrap();
    {
        let interests = planning.interests.lock().unwrap();
        let exact: Vec<_> = interests
            .iter()
            .filter(|row| row.membership == *membership)
            .collect();
        assert_eq!(exact.len(), 1);
        assert!(!exact[0].closed);
        assert!(
            exact[0]
                .original
                .as_ref()
                .is_some_and(|held| Arc::ptr_eq(held, &original))
        );
        assert!(
            exact[0].entries.is_empty(),
            "no G input entry was started by cancelled Find"
        );
    }
    for (key, kind) in [
        (
            membership.workflow().launch_effect(),
            "setup_workflow_launch",
        ),
        (
            membership.delivery_effect().unwrap(),
            "setup_workflow_delivery",
        ),
    ] {
        let effect = fixture.f.store.get_effect(&key).await.unwrap().unwrap();
        assert_eq!(effect.key, key);
        assert_eq!(effect.intent.kind, kind);
        assert_eq!(effect.status, EffectStatus::Pending);
        assert_eq!(effect.attempt, 0);
        assert!(effect.lease.is_none() && effect.outcome.is_none());
    }
    // Pending is the actual untouched outbox state here, not a fabricated
    // completion_uncertain row, receipt or whole-Find NotApplied conclusion.
}

async fn exercise(position: CheckpointPosition) {
    let observed = ReviewedWorkflow::new(position);
    // RAII release exists before construction/dispatch can start a worker.
    let release_on_drop = observed.checkpoint.release_guard();
    let fixture = Box::pin(Fixture::persistent_reviewed_workflow(observed.clone())).await;
    let checked = AssertUnwindSafe(tokio::time::timeout(BOUND, async {
        fixture.publish_reviewed_workflow_bootstrap().unwrap();
        fixture
            .member
            .prepare_waiter_source(fixture.f.original_reference())
            .await
            .unwrap();
        fixture
            .f
            .source
            .prepare_workflow_parent_source(&fixture.parent.original)
            .await
            .unwrap();
        let parent = submit_planning_parent(&fixture.f.source, &fixture.parent).await;
        let PrivateOriginalSubmission::Admitted {
            turn_ticket: Some(ticket),
            ..
        } = parent
        else {
            panic!("real P input is required")
        };
        assert!(matches!(
            fixture
                .f
                .wait_for_turn(
                    &AgentIdentity::parse("planning_parent").unwrap(),
                    TurnTicket::parse(&ticket).unwrap(),
                    BOUND
                )
                .await,
            TurnOutcome::Completed {
                output: TurnOutput::Text { .. }
            }
        ));
        let planning = prepare_planning(
            &fixture.f.source,
            &fixture.parent.original,
            &fixture.lead,
            &fixture.terms,
            &fixture.lineage,
        )
        .await
        .unwrap();
        observed.select_planning(planning);
        let models_before = fixture.f.model_requests();
        let native_before = native_census(&fixture).await;
        // Subscribe to the real actor before submitting A. The leaf Drop is
        // necessary, but only this native terminal proves outer dispatch ended.
        let mut events = fixture
            .f
            .service
            .subscribe_session_events(&fixture.f.session)
            .await
            .unwrap();
        let submitted = fixture.f.submit().await.unwrap();
        assert!(matches!(
            submitted,
            PrivateOriginalSubmission::Admitted {
                turn_ticket: Some(_),
                ..
            }
        ));
        let input_id = match &submitted {
            PrivateOriginalSubmission::Admitted { input_id, .. } => {
                InputId::from_uuid(input_id.parse().unwrap())
            }
            _ => unreachable!("actual A admission checked above"),
        };
        let (admission, deadline) = observed.checkpoint.waiting().await;
        let transfer = observed.transfer().canonical().unwrap();
        let attempt = { transfer.attempt.lock().unwrap().as_ref().unwrap().clone() };
        let TransferOrigin::Entered(commit) = &attempt.origin else {
            panic!("observed data is not the actual entered commit owner")
        };
        assert_eq!(commit.admission(), &admission);
        assert_eq!(attempt.admission, admission);
        assert_eq!(attempt.envelope.find_call, FIND_CALL);
        assert!(!commit.finished());
        let diagnostic = commit.diagnostic();
        let held = diagnostic.observation();
        assert_eq!(held.worker_stage, "storage");
        assert_eq!(held.graph_error, "unobserved");
        assert_eq!(held.fence_refusal, "unobserved");
        assert_eq!(held.worker_result, "unobserved");
        assert!(!held.joined && !held.join_failed);
        assert_eq!(
            held.callback_entered,
            matches!(position, CheckpointPosition::AdmittedNativeStep)
        );
        assert_eq!(observed.review.used(), 0);
        let run = observed.review.run();
        assert!(
            fixture
                .f
                .machine
                .hard_cancel_run_if_current(
                    &fixture.f.session,
                    &run,
                    "entered workflow cancellation control"
                )
                .await
                .unwrap()
        );
        observed.leaf.dropped().await;
        loop {
            let envelope = events
                .next()
                .await
                .expect("real session stream closed before cancellation");
            if let AgentEvent::RunFailed {
                session_id,
                identity,
                error_report,
                ..
            } = envelope.payload
                && session_id == fixture.f.session
                && identity.run_id.as_ref() == Some(&run)
            {
                assert_eq!(error_report.class, AgentErrorClass::Cancelled);
                break;
            }
        }
        assert!(
            observed.checkpoint.is_held(),
            "native cancellation must finish before releasing the worker"
        );
        assert!(!commit.finished());
        assert_eq!(
            observed.review.used(),
            0,
            "actual entered worker still owns the reviewed operation"
        );
        assert!(
            fixture.f.source.owner.clock().now().unwrap().unix_ms < deadline,
            "admitted control cannot pass by expiring the request"
        );
        // No native history, Toolkit records or graph reads while held.
        // Observations use the actor stream, leaf observer, fixed clock and counters.
        assert_eq!(
            fixture.f.model_requests(),
            models_before + 1,
            "one actual A model dispatch"
        );
        observed.checkpoint.release();
        let joined = commit.join().await;
        let settled = diagnostic.observation();
        assert!(settled.joined && !settled.join_failed);
        assert_eq!(
            commit.admission(),
            &admission,
            "cancellation preserves the worker's exact admission"
        );
        assert_eq!(
            commit.join().await,
            joined,
            "repeat join observes the same physical attempt"
        );
        let cached = diagnostic.observation();
        assert!(cached.cached_join_observed);
        assert_eq!(
            commit.admission(),
            &admission,
            "cached join preserves the worker's exact admission"
        );
        assert_eq!(cached.worker_stage, settled.worker_stage);
        assert_eq!(cached.graph_error, settled.graph_error);
        assert_eq!(cached.fence_refusal, settled.fence_refusal);
        assert_eq!(cached.callback_entered, settled.callback_entered);
        assert_eq!(cached.worker_result, settled.worker_result);
        let rows = transfer
            .service
            .observe_admission(admission.key.clone(), attempt.request.clone())
            .await
            .unwrap();
        let graph_events = transfer
            .service
            .events(WorkGraphEventFilter::default())
            .await
            .unwrap();
        match position {
            CheckpointPosition::BeforeNativeStep => {
                assert!(matches!(joined, Err(PrivateOriginalError::Denied)));
                assert_eq!(settled.worker_stage, "storage");
                assert_eq!(settled.graph_error, "Conflict");
                assert_eq!(settled.fence_refusal, "Denied");
                assert_eq!(settled.worker_result, "Denied");
                assert!(!settled.callback_entered);
                assert!(
                    rows.is_none(),
                    "queued native step must not create a graph admission"
                );
                assert!(graph_events.is_empty());
                assert_eq!(
                    observed.checkpoint.positions(),
                    vec![CheckpointPosition::BeforeNativeStep]
                );
                // Earlier membership/interest/effect rows remain real. This
                // denial does not claim whole Find was never invoked.
            }
            CheckpointPosition::AdmittedNativeStep => {
                assert_eq!(joined.unwrap(), admission);
                assert_eq!(settled.worker_stage, "completed");
                assert_eq!(settled.graph_error, "none");
                assert_eq!(
                    settled.fence_refusal, "unobserved",
                    "successful path does not add a fence read"
                );
                assert_eq!(settled.worker_result, "ok");
                assert!(settled.callback_entered);
                let Some(WorkAdmissionOutcome::Replayed(item)) = rows else {
                    panic!("same committed graph row")
                };
                validate_item_identity(&transfer.service, &admission, &attempt.request, &item)
                    .unwrap();
                assert_eq!(graph_events.len(), 1);
                assert_eq!(graph_events[0].kind, WorkGraphEventKind::Created);
                assert_eq!(graph_events[0].item_id.as_ref(), Some(&item.id));
                assert_eq!(
                    observed.checkpoint.positions(),
                    vec![
                        CheckpointPosition::BeforeNativeStep,
                        CheckpointPosition::AdmittedNativeStep
                    ]
                );
            }
        }
        retained_prior_effects(&fixture, &transfer, &attempt).await;
        assert!(
            attempt.delivery.lock().unwrap().is_none(),
            "cancelled dispatch did not start delivery"
        );
        assert!(
            attempt.advance.lock().unwrap().is_none(),
            "cancelled dispatch did not start automatic advance"
        );
        // The cancelled actor event precedes the runtime history commit.
        // After the entered worker joins, await this exact input's finalized
        // receipt before comparing durable censuses. The existing BOUND owns
        // this event-driven wait; no operation or cancellation is retried.
        let terminal = fixture
            .f
            .machine
            .wait_input_terminal_receipt(&fixture.f.session, &input_id)
            .await
            .unwrap()
            .expect("actual A input must remain available");
        let InputTerminalReceiptWait::Resolved(terminal) = terminal else {
            panic!("actual A input detached before its receipt finalized")
        };
        let InputTerminalReceiptRead::Finalized(receipt) = terminal.report else {
            panic!("actual A cancellation requires a finalized receipt")
        };
        assert_eq!(receipt.input_id(), &input_id);
        assert_eq!(receipt.run_id(), Some(&run));
        assert!(matches!(receipt.outcome(), CompletionOutcome::Cancelled));
        let native_after = native_census(&fixture).await;
        assert_eq!(
            native_after[1], native_before[1],
            "no extra P input or dispatch"
        );
        assert_eq!(native_after[2], native_before[2], "no G input or dispatch");
        let before_a = native_before[0]["inputs"].as_array().unwrap();
        let after_a = native_after[0]["inputs"].as_array().unwrap();
        assert_eq!(
            after_a.len(),
            before_a.len() + 1,
            "one actual original A input"
        );
        assert!(before_a.iter().all(|input| after_a.contains(input)));
        assert_eq!(
            native_census(&fixture).await,
            native_after,
            "joining retains a stable native census"
        );
        assert_eq!(fixture.f.model_requests(), models_before + 1);
        let (feedback, _, entries) = observed.script_census();
        assert!(
            feedback.is_empty(),
            "cancelled dispatch did not return a substitute success"
        );
        assert_eq!(entries, 1);
    }))
    .catch_unwind()
    .await;
    // Release before close, including assertion/timeout paths. Full owner joins
    // observe failed commits without manufacturing success or aborting workers.
    observed.checkpoint.release();
    let joined = fixture
        .f
        .source
        .join_workflow_context_transfers(false)
        .await;
    let closed = fixture.close_reviewed_workflow().await;
    drop(release_on_drop);
    assert!(
        joined.is_ok() && closed.is_ok(),
        "actual owner joins: {joined:?}/{closed:?}"
    );
    match checked {
        Err(panic) => std::panic::resume_unwind(panic),
        Ok(Err(_)) => panic!("entered workflow cancellation boundary did not complete"),
        Ok(Ok(())) => (),
    }
    tokio::time::timeout(BOUND, observed.review.assert_used_once())
        .await
        .expect("first entered attempt Used after actual native/worker joins");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reviewed_find_queued_graph_step_refuses_after_actual_dispatch_cancellation() {
    exercise(CheckpointPosition::BeforeNativeStep).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reviewed_find_admitted_graph_step_finishes_after_actual_dispatch_cancellation() {
    exercise(CheckpointPosition::AdmittedNativeStep).await;
}
