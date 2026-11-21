//! A durable steer that waits for its run's next boundary keeps that place
//! when the process restarts and the same run resumes.
//!
//! A durable steer "waits for the next boundary of the same run", and a run
//! that resumes after a restart is that run. So a steer that was waiting when
//! the process died joins the resumed run's next request, and a steer
//! admitted while the resumed run runs joins the same request behind it, in
//! admission order, each exactly once. When no run resumes, the waiting
//! steers start the next run as its own inputs, as before.
use super::next_request_recovery::observed_today::stand_continuable_run_tail;
use super::*;

/// The inputs the most recent boundary take applied, in order.
fn last_take_ids(rig: &DurableSteerRig) -> Vec<InputId> {
    rig.script
        .boundary_takes()
        .last()
        .expect("a boundary was taken")
        .iter()
        .map(|(input_id, _)| input_id.clone())
        .collect()
}

struct Restarted {
    rig: DurableSteerRig,
    store: Arc<dyn crate::store::RuntimeStore>,
    batch: InputId,
    /// The run that was open when the process died.
    crashed_run: RunId,
    /// The steers admitted before the restart, in admission order.
    waiting: Vec<InputId>,
}

/// A persistent run is busy; each of `waiting_payloads` is admitted as a peer
/// steer and waits for the run's next boundary. The process dies. A fresh
/// runtime registers the session over the same store.
///
/// With `run_resumes`, the store holds what a durable run's round commit
/// leaves (a committed base and the run's provisional tail, whose cursor
/// binds the batch input), so the restart resumes the SAME run. Without it
/// the restart finds no run to continue.
async fn restart_with_waiting_steers(
    store: Arc<dyn crate::store::RuntimeStore>,
    waiting_payloads: &[&str],
    run_resumes: bool,
) -> Restarted {
    let crashed = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let session_id = crashed.session_id.clone();
    let batch = crashed.start_busy_turn().await;
    let crashed_run = crashed
        .script
        .active_run
        .lock()
        .unwrap()
        .clone()
        .expect("active run");
    if run_resumes {
        stand_continuable_run_tail(
            &store,
            &session_id,
            &crashed_run,
            std::slice::from_ref(&batch),
        )
        .await;
    }
    let mut waiting = Vec::new();
    for payload in waiting_payloads {
        let steer = peer_steer(payload);
        waiting.push(steer.id().clone());
        crashed.admit(steer).await;
    }
    if !waiting.is_empty() {
        crashed.wait_for_waiting_delivery().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            crashed.steer_queue().await,
            waiting,
            "before the restart the steers wait in the steer lane, in admission order"
        );
    }
    // The process dies with the run open. The first runtime is never driven
    // again.
    std::mem::forget(crashed);

    let rig = DurableSteerRig::with_adapter_for_session(
        Arc::new(
            MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
                .expect("persistent machine"),
        ),
        session_id,
    )
    .await;
    rig.wait_for_apply_calls(1).await;
    if run_resumes {
        assert_eq!(
            rig.script.applied_runs.lock().unwrap().clone(),
            vec![crashed_run.clone()],
            "the restart resumes the SAME run, under its own id"
        );
        assert_eq!(
            rig.script.primitives(),
            vec![vec![batch.clone()]],
            "the resumed run carries its own bound input"
        );
    }
    Restarted {
        rig,
        store,
        batch,
        crashed_run,
        waiting,
    }
}

/// The scripted runner commits no session document, so the hand-stood tail
/// leaves the store before the run's terminal.
async fn discard_the_stood_tail(restarted: &Restarted) {
    let runtime_id = MeerkatMachine::logical_runtime_id(&restarted.rig.session_id);
    let tail = restarted
        .store
        .load_whole_blob_provisional_tail(&runtime_id)
        .await
        .expect("read the provisional tail")
        .expect("the run's tail stands")
        .authority()
        .clone();
    assert!(
        restarted
            .store
            .discard_whole_blob_provisional_tail(&runtime_id, &tail)
            .await
            .expect("discard the tail")
    );
}

/// Give a live-boundary preparation the time to register, without requiring
/// one: the boundary step that follows reads what the request carries.
async fn settle_live_boundary_work(rig: &DurableSteerRig) {
    let _ = tokio::time::timeout(Duration::from_millis(500), async {
        while !rig.state.has_waiting_delivery_for_test() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// The resumed run's next request carries `expected`, in that order; each is
/// bound to the resumed run, consumed with it and delivered exactly once, and
/// no follow-up turn runs.
async fn assert_the_resumed_runs_next_request_carries(restarted: &Restarted, expected: &[InputId]) {
    let rig = &restarted.rig;
    settle_live_boundary_work(rig).await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(
        last_take_ids(rig),
        expected,
        "the resumed run's next request carries every steer that waited for it, in admission order"
    );
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    for input_id in expected {
        let row = restarted
            .store
            .load_input_state(&runtime_id, input_id)
            .await
            .expect("load the steer's row")
            .expect("the steer's row is stored");
        assert_eq!(row.seed.phase, InputLifecycleState::Staged);
        assert_eq!(
            row.seed.last_run_id.as_ref(),
            Some(&restarted.crashed_run),
            "{input_id} is bound to the resumed run"
        );
    }

    discard_the_stood_tail(restarted).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&restarted.batch, InputLifecycleState::Consumed)
        .await;
    for input_id in expected {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    assert_eq!(
        rig.script.applied_durable(),
        expected,
        "each steer was applied at the boundary exactly once, in admission order"
    );
    let primitives = rig.script.primitives();
    for input_id in expected {
        assert_eq!(
            contributions(&primitives, input_id),
            0,
            "{input_id} starts no turn of its own"
        );
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "no follow-up turn ran"
    );
}

/// Steer E waits for a boundary when the process dies, the same run resumes,
/// and steer F is admitted while it runs: the resumed run's next request
/// carries E and then F.
async fn waiting_steer_and_a_later_one_join_the_resumed_runs_next_request(
    store: Arc<dyn crate::store::RuntimeStore>,
) {
    let restarted =
        restart_with_waiting_steers(store, &["waited across the restart E"], true).await;
    let later = peer_steer("admitted during the resumed run F");
    let f_id = later.id().clone();
    restarted.rig.admit(later).await;
    let mut expected = restarted.waiting.clone();
    expected.push(f_id);
    assert_the_resumed_runs_next_request_carries(&restarted, &expected).await;
}

#[tokio::test]
async fn steer_waiting_at_a_restart_and_a_later_one_are_both_in_the_resumed_runs_next_request() {
    waiting_steer_and_a_later_one_join_the_resumed_runs_next_request(Arc::new(
        crate::store::InMemoryRuntimeStore::new(),
    ))
    .await;
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_steer_waiting_at_a_restart_and_a_later_one_are_both_in_the_resumed_runs_next_request()
 {
    let directory = tempfile::tempdir().unwrap();
    waiting_steer_and_a_later_one_join_the_resumed_runs_next_request(Arc::new(
        crate::store::SqliteRuntimeStore::new(directory.path().join("runtime.sqlite")).unwrap(),
    ))
    .await;
}

/// No steer is admitted after the restart: the one that waited still joins
/// the resumed run's next request.
#[tokio::test]
async fn steer_waiting_alone_at_a_restart_is_in_the_resumed_runs_next_request() {
    let restarted = restart_with_waiting_steers(
        Arc::new(crate::store::InMemoryRuntimeStore::new()),
        &["waited alone across the restart E"],
        true,
    )
    .await;
    let expected = restarted.waiting.clone();
    assert_the_resumed_runs_next_request_carries(&restarted, &expected).await;
}

/// Two steers wait at the restart and a third is admitted after it: one
/// request carries all three, in admission order.
#[tokio::test]
async fn two_steers_waiting_at_a_restart_and_a_later_one_are_in_one_request_in_admission_order() {
    let restarted = restart_with_waiting_steers(
        Arc::new(crate::store::InMemoryRuntimeStore::new()),
        &[
            "waited across the restart, first",
            "waited across the restart, second",
        ],
        true,
    )
    .await;
    let later = peer_steer("admitted during the resumed run, third");
    let third = later.id().clone();
    restarted.rig.admit(later).await;
    let mut expected = restarted.waiting.clone();
    expected.push(third);
    assert_the_resumed_runs_next_request_carries(&restarted, &expected).await;
}

/// CONTROL: the restart finds no run to continue. The steer that waited
/// starts the next run as its own input (the steer lane is served first), a
/// steer admitted during that run joins its next request, and the crashed
/// run's batch input runs after. Each input is delivered exactly once.
#[tokio::test]
async fn restart_without_a_resumed_run_starts_the_next_run_with_the_waiting_steer() {
    let restarted = restart_with_waiting_steers(
        Arc::new(crate::store::InMemoryRuntimeStore::new()),
        &["waited across a restart with no resumed run E"],
        false,
    )
    .await;
    let rig = &restarted.rig;
    let e_id = restarted.waiting[0].clone();
    assert_ne!(
        rig.script.applied_runs.lock().unwrap().clone(),
        vec![restarted.crashed_run.clone()],
        "no run resumes: the first run after the restart is a new one"
    );
    assert_eq!(
        rig.script.primitives(),
        vec![vec![e_id.clone()]],
        "the next run starts with the steer that waited"
    );

    let later = peer_steer("admitted during the next run F");
    let f_id = later.id().clone();
    rig.admit(later).await;
    rig.wait_for_waiting_delivery().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(
        last_take_ids(rig),
        vec![f_id.clone()],
        "the run's next request carries the steer admitted during it"
    );

    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    for input_id in [&restarted.batch, &e_id, &f_id] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    let primitives = rig.script.primitives();
    assert_eq!(
        primitives,
        vec![vec![e_id.clone()], vec![restarted.batch.clone()]],
        "two runs: the waiting steer's, then the crashed run's batch input"
    );
    assert_eq!(
        rig.script.applied_durable(),
        vec![f_id],
        "only the steer admitted during the run was applied at a boundary"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 2);
}
