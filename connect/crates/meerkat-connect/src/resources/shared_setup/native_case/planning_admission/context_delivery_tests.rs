//! Actual completed transfers entering the existing G through the public API.
//! Fault-injected lost acknowledgements and cold recovery are not claimed here.
use super::context_transfer_tests::{Transfer, prepare};
use super::*;
use meerkat_core::lifecycle::InputId;
use meerkat_runtime::SessionServiceRuntimeExt;
use toolkit_mobkit::private_originals::{
    InstalledWorkflowContext, PrivateOriginalError, WorkflowContextObservation,
    WorkflowDeliveryObservation,
};

async fn context(transfer: &Transfer) -> InstalledWorkflowContext {
    match transfer.1.observe().await.unwrap() {
        WorkflowContextObservation::Committed(context) => context,
        _ => panic!("actual Find transfer was not committed"),
    }
}

async fn cancel(case: &Case, original: &toolkit_mobkit::private_originals::PrivateOriginalRef) {
    let result = case
        .client
        .lock()
        .await
        .call(json!({
            "kind":"cancel_original_operation", "original":original,
        }))
        .await
        .unwrap();
    assert_eq!(result["kind"], "original_operation_cancelled");
}

pub(super) async fn accepted_input(
    case: &Case,
    planning: &PlanningCase,
    observation: &WorkflowDeliveryObservation,
) -> (InputId, Value) {
    let WorkflowDeliveryObservation::Admitted {
        session_id,
        input_id,
    } = observation
    else {
        panic!("exact native delivery must be admitted: {observation:?}")
    };
    let session = planning.member.session().unwrap();
    assert_eq!(session_id, &session.to_string());
    let input: InputId = serde_json::from_value(json!(input_id)).unwrap();
    let service = case
        .native
        .as_ref()
        .unwrap()
        .mob_runtime()
        .session_service()
        .unwrap();
    let machine = service.acquire_runtime_adapter(None).unwrap().unwrap();
    let stored = machine
        .input_state(&session, &input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.state.input_id, input);
    let association = planning
        .source
        .binding_evidence(&planning.lead.original)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.state.authority_contributors.len(), 1);
    assert!(stored.state.authority_contributors[0].association() == &association);
    assert_eq!(stored.state.authority_contributors[0].input_id(), &input);
    assert!(stored.state.idempotency_key.is_some());
    assert!(matches!(
        &stored.state.scoped_destination,
        meerkat_runtime::input_state::ScopedDestinationWitness::Bound { .. }
    ));
    // Admission is asserted separately from this controlled model's actual
    // terminal outcome. Wait for the real owner before comparing histories.
    tokio::time::timeout(BOUND, async {
        loop {
            if machine
                .input_terminal_completion(&session, &input)
                .await
                .unwrap()
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    (
        input,
        json!({"key":stored.state.idempotency_key,
        "association":association, "destination":stored.state.scoped_destination}),
    )
}

async fn exercise(
    case: &mut Case,
    pipes: toolkit_mobkit::private_host::InheritedHostPipes,
    close_before_replay: bool,
) {
    // prepare only submits the ordinary SDK Originals and observes receipts.
    // No control calls deliver() until both automatic inputs have settled.
    let (planning, transfers, a, b) = prepare(case, pipes).await;
    let ta = transfers.iter().find(|row| row.0 == a).unwrap();
    let tb = transfers.iter().find(|row| row.0 == b).unwrap();
    let a_context = context(ta).await;
    let a_observation = ta.1.reconcile_delivery().await.unwrap();
    let b_observation = tb.1.reconcile_delivery().await.unwrap();
    let (a_id, a_binding) = accepted_input(case, &planning, &a_observation).await;
    let (b_id, b_binding) = accepted_input(case, &planning, &b_observation).await;
    assert_ne!(a_id, b_id);
    assert_ne!(a_binding["key"], b_binding["key"]);
    assert_eq!(a_binding["association"], b_binding["association"]);
    assert_eq!(a_binding["destination"], b_binding["destination"]);
    assert_eq!(input_count(case, "planning-lead").await, 3);
    let before_g = history(case, "planning-lead").await;
    assert!(before_g.to_string().contains("sdk-planning-a"));
    assert!(before_g.to_string().contains("sdk-planning-b"));
    let before_p = history(case, "planning-parent").await;
    let before_a = history(case, "original-a").await;
    let before_b = history(case, "original-b").await;
    let (_, delivered, _) = workflow_evidence(case);
    cancel(case, &a).await;
    if close_before_replay {
        cancel(case, &b).await;
        assert!(matches!(
            a_context.deliver().await,
            Err(PrivateOriginalError::Denied)
        ));
        assert!(matches!(
            ta.1.observe().await,
            Err(PrivateOriginalError::Denied)
        ));
    } else {
        // A's completed disclosure and accepted input remain available while B
        // keeps the recipient live. A is not imported as a G contributor.
        assert_eq!(a_context.deliver().await.unwrap(), a_observation);
        assert_eq!(context(ta).await.deliver().await.unwrap(), a_observation);
        assert_eq!(context(tb).await.deliver().await.unwrap(), b_observation);
        cancel(case, &b).await;
        assert!(matches!(
            ta.1.observe().await,
            Err(PrivateOriginalError::Denied)
        ));
        assert!(matches!(
            a_context.deliver().await,
            Err(PrivateOriginalError::Denied)
        ));
    }
    // Metadata reconciliation after close is not a lost-ack injection and
    // cannot reopen the payload or dispatch another native input.
    assert_eq!(ta.1.reconcile_delivery().await.unwrap(), a_observation);
    assert_eq!(tb.1.reconcile_delivery().await.unwrap(), b_observation);
    assert_eq!(input_count(case, "planning-lead").await, 3);
    assert_eq!(history(case, "planning-lead").await, before_g);
    assert_eq!(history(case, "planning-parent").await, before_p);
    assert_eq!(history(case, "original-a").await, before_a);
    assert_eq!(history(case, "original-b").await, before_b);
    assert_eq!(
        workflow_evidence(case)
            .1
            .iter()
            .filter(|effect| effect.intent.kind == "setup_workflow_delivery")
            .collect::<Vec<_>>(),
        delivered
            .iter()
            .filter(|effect| effect.intent.kind == "setup_workflow_delivery")
            .collect::<Vec<_>>(),
        "replay and close preserve exact accepted delivery effects"
    );
    planning.cancel_parent(case).await;
}

fn run(close_before_replay: bool) {
    let (pipes, parent) = SdkParent::pair();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let mut case = Case::new(parent.enter());
        let checked = std::panic::AssertUnwindSafe(exercise(&mut case, pipes, close_before_replay))
            .catch_unwind()
            .await;
        let cleanup = case.cleanup().await;
        assert!(
            cleanup.is_empty(),
            "actual SDK/native owners must be joined: {cleanup:?}"
        );
        if let Err(panic) = checked {
            std::panic::resume_unwind(panic);
        }
    });
}

#[test]
fn committed_waiter_context_is_delivered_automatically_once_and_survives_departure() {
    run(false);
}
#[test]
fn closed_workflow_refuses_context_replay_after_automatic_admission() {
    run(true);
}
