//! A recycle and a durable steer that waits.
//!
//! A recycle rebuilds the session's driver and recovers its live inputs
//! again from their rows, on the same machine. The machine takes a recycle
//! only while no run is current, so there are two scenes:
//!
//! - a run is running and a steer waits for its boundary: the recycle is
//!   refused and changes nothing; the steer joins the run's next request,
//!   once;
//! - no run is current and a steer is queued: the recycle recovers it with
//!   the delivery class its row stores, the machine keeps that class for a
//!   steer that never left its lane, and the next run takes the steer as its
//!   own input, once.
use super::*;

/// The delivery class the machine holds for `input_id`.
async fn machine_class(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    input_id: &InputId,
) -> Option<String> {
    let driver = {
        let sessions = adapter.sessions.read().await;
        Arc::clone(&sessions.get(session_id).expect("registered session").driver)
    };
    let entry = driver.lock().await;
    entry
        .shared_dsl_authority()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .state()
        .input_live_boundary_delivery
        .get(&input_id.to_string())
        .map(|delivery| format!("{delivery:?}"))
}

async fn recycle_beside_a_running_run_is_refused(rig: DurableSteerRig) {
    let batch = rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let steer = peer_steer("waiting while a recycle is asked for");
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;

    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let answer = crate::traits::RuntimeControlPlane::recycle(rig.adapter.as_ref(), &runtime_id)
        .await
        .map(|report| report.inputs_transferred);
    assert!(
        answer.is_err(),
        "the machine takes no recycle beside a running run: {answer:?}"
    );
    assert_eq!(
        machine_class(&rig.adapter, &rig.session_id, &steer_id).await,
        Some("DurableAppend".to_string()),
        "the waiting steer keeps its delivery"
    );
    assert_eq!(rig.steer_queue().await, vec![steer_id.clone()]);
    assert!(
        rig.state.has_waiting_delivery_for_test(),
        "and its registration for the boundary"
    );

    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(
        rig.script
            .boundary_takes()
            .last()
            .expect("a boundary was taken")
            .iter()
            .map(|(input_id, _)| input_id.clone())
            .collect::<Vec<_>>(),
        vec![steer_id.clone()],
        "the run's next request carries the steer"
    );
    rig.script.step(RunnerStep::Finish);
    for input_id in [&batch, &steer_id] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    assert_eq!(rig.script.applied_durable(), vec![steer_id.clone()]);
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 0);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "no follow-up turn ran"
    );
}

#[tokio::test]
async fn recycle_beside_a_running_run_is_refused_and_the_waiting_steer_joins_once() {
    recycle_beside_a_running_run_is_refused(DurableSteerRig::ephemeral().await).await;
}

#[tokio::test]
async fn persistent_recycle_beside_a_running_run_is_refused_and_the_waiting_steer_joins_once() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    recycle_beside_a_running_run_is_refused(DurableSteerRig::persistent(store).await).await;
}

async fn recycle_with_no_run_current_keeps_a_queued_steer(adapter: Arc<MeerkatMachine>) {
    let session_id = SessionId::new();
    adapter
        .register_session(session_id.clone())
        .await
        .expect("register the session with no executor");
    let steer = peer_steer("queued across a recycle");
    let steer_id = steer.id().clone();
    let (outcome, _completion) = adapter
        .accept_input_with_completion(&session_id, steer)
        .await
        .expect("steer accepted");
    assert!(outcome.is_accepted());
    assert_eq!(
        machine_class(&adapter, &session_id, &steer_id).await,
        Some("DurableAppend".to_string()),
        "admission gives the steer its delivery class"
    );

    let runtime_id = MeerkatMachine::logical_runtime_id(&session_id);
    let report = crate::traits::RuntimeControlPlane::recycle(adapter.as_ref(), &runtime_id)
        .await
        .expect("a recycle with no run current");
    assert_eq!(report.inputs_transferred, 1);
    assert_eq!(
        machine_class(&adapter, &session_id, &steer_id).await,
        Some("DurableAppend".to_string()),
        "the recycled steer keeps the class its row stores"
    );

    // An executor attaches: the next run takes the steer as its own input.
    let rig = DurableSteerRig::with_adapter_for_session(adapter, session_id).await;
    rig.wait_for_apply_calls(1).await;
    assert_eq!(
        rig.script.primitives(),
        vec![vec![steer_id.clone()]],
        "the next run starts with the steer"
    );
    assert_eq!(
        machine_class(&rig.adapter, &rig.session_id, &steer_id).await,
        None,
        "staging removed the class"
    );
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    assert!(
        rig.script.applied_durable().is_empty(),
        "nothing was applied at a boundary"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "the steer was delivered once, at that run's start"
    );
}

#[tokio::test]
async fn recycle_with_no_run_current_keeps_a_queued_steer_and_the_next_run_delivers_it_once() {
    recycle_with_no_run_current_keeps_a_queued_steer(Arc::new(MeerkatMachine::ephemeral())).await;
}

#[tokio::test]
async fn persistent_recycle_with_no_run_current_keeps_a_queued_steer_and_the_next_run_delivers_it_once()
 {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    recycle_with_no_run_current_keeps_a_queued_steer(Arc::new(
        MeerkatMachine::persistent_without_blobs(store).expect("persistent machine"),
    ))
    .await;
}
