//! A durable steer that waits for its run's next boundary keeps that place
//! when the process restarts and the same run resumes.
//!
//! A durable steer "waits for the next boundary of the same run", and a run
//! that resumes after a restart is that run. So a steer that was waiting when
//! the process died joins the resumed run's next request, and a steer
//! admitted while the resumed run runs joins the same request behind it, in
//! admission order, each exactly once. When no run resumes, the waiting
//! steers start the next run as its own inputs, as before.
//!
//! These rows reconstruct the restart inside one test process: the first
//! runtime is abandoned and never driven again, and a fresh runtime
//! registers the session over the same store. The tail that makes the run
//! resumable is stood by the rig, because the scripted runner writes no
//! session document.
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
/// steer and waits for the run's next boundary. The first runtime is then
/// abandoned, as when its process dies, and a fresh runtime registers the
/// session over the same store.
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
    let abandoned = abandon_with_waiting_steers(&store, waiting_payloads, run_resumes).await;
    restart_over(store, abandoned).await
}

/// What the abandoned first runtime leaves behind.
struct Abandoned {
    session_id: SessionId,
    batch: InputId,
    crashed_run: RunId,
    waiting: Vec<InputId>,
    /// Whether the store holds a tail that makes `crashed_run` resumable.
    run_resumes: bool,
}

/// The first half of [`restart_with_waiting_steers`]: the busy run, the
/// waiting steers, and the first runtime abandoned.
async fn abandon_with_waiting_steers(
    store: &Arc<dyn crate::store::RuntimeStore>,
    waiting_payloads: &[&str],
    run_resumes: bool,
) -> Abandoned {
    let crashed = DurableSteerRig::persistent(Arc::clone(store)).await;
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
            store,
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
    // The first runtime is never driven again, as when its process dies with
    // the run open.
    std::mem::forget(crashed);
    Abandoned {
        session_id,
        batch,
        crashed_run,
        waiting,
        run_resumes,
    }
}

/// The second half: a fresh runtime registers the session over `store`.
async fn restart_over(
    store: Arc<dyn crate::store::RuntimeStore>,
    abandoned: Abandoned,
) -> Restarted {
    let Abandoned {
        session_id,
        batch,
        crashed_run,
        waiting,
        run_resumes,
    } = abandoned;
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
    discard_the_stood_tail_of(&restarted.store, &restarted.rig.session_id).await;
}

async fn discard_the_stood_tail_of(
    store: &Arc<dyn crate::store::RuntimeStore>,
    session_id: &SessionId,
) {
    let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
    let tail = store
        .load_whole_blob_provisional_tail(&runtime_id)
        .await
        .expect("read the provisional tail")
        .expect("the run's tail stands")
        .authority()
        .clone();
    assert!(
        store
            .discard_whole_blob_provisional_tail(&runtime_id, &tail)
            .await
            .expect("discard the tail")
    );
}

async fn recall(
    rig: &DurableSteerRig,
    input_id: &InputId,
) -> Option<crate::queued_input_recall::QueuedInputRecall> {
    tokio::time::timeout(
        Duration::from_secs(5),
        rig.adapter.recall_queued_input(&rig.session_id, input_id),
    )
    .await
    .expect("a recall answers without waiting for a boundary")
    .expect("recall")
}

/// Finish every run the runtime starts until each of `inputs` is consumed.
async fn drain(rig: &DurableSteerRig, inputs: &[InputId]) {
    let mut finished = 0;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let mut consumed = true;
            for input_id in inputs {
                consumed &= rig.phase(input_id).await == Some(InputLifecycleState::Consumed);
            }
            if consumed {
                return;
            }
            if *rig.script.applies_observable.borrow() > finished {
                rig.script.step(RunnerStep::Finish);
                finished += 1;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the runtime consumes every input");
}

/// How often the runtime delivered `input_id`: turn starts plus boundary
/// applies.
fn deliveries(rig: &DurableSteerRig, input_id: &InputId) -> usize {
    contributions(&rig.script.primitives(), input_id)
        + rig
            .script
            .applied_durable()
            .iter()
            .filter(|applied| *applied == input_id)
            .count()
}

/// The run ends with `waiting` still in the steer lane: exactly one
/// follow-up turn starts with them, in that order, and nothing was applied
/// at a boundary.
async fn assert_one_follow_up_turn_delivers(restarted: &Restarted, waiting: &[InputId]) {
    let rig = &restarted.rig;
    discard_the_stood_tail(restarted).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&restarted.batch, InputLifecycleState::Consumed)
        .await;
    for input_id in waiting {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    assert_eq!(
        rig.script.primitives()[1],
        waiting,
        "one follow-up turn starts with the waiting steers, in admission order"
    );
    assert!(
        rig.script.applied_durable().is_empty(),
        "no boundary applied any of them"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        2,
        "exactly one follow-up turn ran"
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

/// The runtime loop begins the resumed run in the machine before the
/// executor has opened the run's first boundary. The row holds the loop
/// between the two, long enough for the waiting steer's boundary work to
/// run: a preparation made then would be refused, as for a run that ended,
/// and the steer would be left to the follow-up turn. The steer must still
/// be in the resumed run's next request.
#[tokio::test]
async fn steer_waiting_at_a_restart_joins_when_the_executor_starts_the_resumed_run_late() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let abandoned = abandon_with_waiting_steers(
        &store,
        &["waited for an executor that started late E"],
        true,
    )
    .await;
    let adapter = Arc::new(
        MeerkatMachine::persistent_without_blobs(Arc::clone(&store)).expect("persistent machine"),
    );
    let (loop_before_apply, release_apply) =
        adapter.arm_runtime_loop_before_executor_apply_test_hook(abandoned.session_id.clone());
    let rig =
        DurableSteerRig::with_adapter_for_session(adapter, abandoned.session_id.clone()).await;
    tokio::time::timeout(Duration::from_secs(5), loop_before_apply)
        .await
        .expect("the loop reaches the executor's apply")
        .expect("the loop's hook is alive");
    // The resumed run is begun in the machine and the executor has not
    // started it. The waiting steer's work has all the time it needs.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !rig.state.has_waiting_delivery_for_test(),
        "nothing can register before the executor opens the run's boundary"
    );
    release_apply.send(()).expect("the loop is still held");
    rig.wait_for_apply_calls(1).await;
    assert_eq!(
        rig.script.applied_runs.lock().unwrap().clone(),
        vec![abandoned.crashed_run.clone()],
        "the restart resumes the SAME run"
    );
    let restarted = Restarted {
        rig,
        store,
        batch: abandoned.batch,
        crashed_run: abandoned.crashed_run,
        waiting: abandoned.waiting,
    };
    let expected = restarted.waiting.clone();
    assert_the_resumed_runs_next_request_carries(&restarted, &expected).await;
}

/// Whether the live-boundary work of `input_id` is running.
async fn steer_work_is_running(rig: &DurableSteerRig, input_id: &InputId) -> bool {
    let driver = {
        let sessions = rig.adapter.sessions.read().await;
        Arc::clone(
            &sessions
                .get(&rig.session_id)
                .expect("registered session")
                .driver,
        )
    };
    let entry = driver.lock().await;
    entry
        .driver_ingress()
        .durable_steer_order()
        .is_working(input_id)
}

/// Steer F is admitted after the restart and before the work of steer E,
/// which waited across it, has begun: the row holds the loop before the
/// executor starts the resumed run, so E's work still waits for that start.
/// F's own work finds E ahead of it with no work running and ends without a
/// preparation; F keeps its place in the steer lane behind E. Once the
/// executor starts the run, E's work takes the boundary and joins F behind
/// it: one request of the resumed run carries E and then F, each once, and
/// no follow-up turn runs.
#[tokio::test]
async fn steer_admitted_before_the_waiting_steers_work_began_joins_the_same_request_behind_it() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let abandoned =
        abandon_with_waiting_steers(&store, &["waited across the restart E"], true).await;
    let e_id = abandoned.waiting[0].clone();
    let adapter = Arc::new(
        MeerkatMachine::persistent_without_blobs(Arc::clone(&store)).expect("persistent machine"),
    );
    let (loop_before_apply, release_apply) =
        adapter.arm_runtime_loop_before_executor_apply_test_hook(abandoned.session_id.clone());
    let rig =
        DurableSteerRig::with_adapter_for_session(adapter, abandoned.session_id.clone()).await;
    tokio::time::timeout(Duration::from_secs(5), loop_before_apply)
        .await
        .expect("the loop reaches the executor's apply")
        .expect("the loop's hook is alive");
    assert!(
        !steer_work_is_running(&rig, &e_id).await,
        "the waiting steer's work has not begun: the executor has not started the run"
    );

    let later = peer_steer("admitted before the waiting steer's work began F");
    let f_id = later.id().clone();
    rig.admit(later).await;
    // The later steer's own work has all the time it needs to end.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !steer_work_is_running(&rig, &f_id).await,
        "the later steer's own work ended: an older steer waits ahead of it with no work running"
    );
    assert!(
        !steer_work_is_running(&rig, &e_id).await,
        "the waiting steer's work has still not begun"
    );
    assert!(
        !rig.state.has_waiting_delivery_for_test(),
        "the later steer's own work prepared nothing"
    );
    assert_eq!(
        rig.steer_queue().await,
        vec![e_id.clone(), f_id.clone()],
        "both steers wait in the steer lane, in admission order"
    );

    release_apply.send(()).expect("the loop is still held");
    rig.wait_for_apply_calls(1).await;
    assert_eq!(
        rig.script.applied_runs.lock().unwrap().clone(),
        vec![abandoned.crashed_run.clone()],
        "the restart resumes the SAME run"
    );
    let restarted = Restarted {
        rig,
        store,
        batch: abandoned.batch,
        crashed_run: abandoned.crashed_run,
        waiting: abandoned.waiting,
    };
    // The later steer's own work is over, so only the walk of the waiting
    // steer's work can put it in this request.
    assert_the_resumed_runs_next_request_carries(&restarted, &[e_id, f_id]).await;
}

/// Two steers wait at the restart and the first is recalled once its
/// delivery has registered for the resumed run's boundary. The recalled one
/// reaches no request; the one behind it and a later one join the resumed
/// run's next request.
#[tokio::test]
async fn steer_recalled_after_a_restart_reaches_no_request_and_the_others_join_the_resumed_run() {
    let restarted = restart_with_waiting_steers(
        Arc::new(crate::store::InMemoryRuntimeStore::new()),
        &[
            "recalled after the restart",
            "kept its place behind the recalled one",
        ],
        true,
    )
    .await;
    let rig = &restarted.rig;
    let recalled = restarted.waiting[0].clone();
    rig.wait_for_waiting_delivery().await;
    assert_eq!(
        recall(rig, &recalled).await,
        Some(crate::queued_input_recall::QueuedInputRecall::Abandoned {
            input_id: recalled.clone(),
            attempt_count: 0,
        })
    );
    let later = peer_steer("admitted after the recall");
    let later_id = later.id().clone();
    rig.admit(later).await;
    let expected = vec![restarted.waiting[1].clone(), later_id];
    assert_the_resumed_runs_next_request_carries(&restarted, &expected).await;
    assert_eq!(deliveries(rig, &recalled), 0);
    assert!(
        rig.script
            .boundary_takes()
            .iter()
            .flatten()
            .all(|(_, payload)| !payload.contains("recalled after the restart")),
        "the recalled payload reaches no request"
    );
    assert_eq!(
        rig.phase(&recalled).await,
        Some(InputLifecycleState::Abandoned)
    );
}

/// The steer that waited is recalled after the resumed run's runner parked
/// with its claim and before anything is published: its claim stays with the
/// lane, and the steer behind it joins that request.
#[tokio::test]
async fn steer_recalled_while_the_resumed_runs_boundary_is_parked_leaves_the_one_behind_it_in_that_request()
 {
    let restarted = restart_with_waiting_steers(
        Arc::new(crate::store::InMemoryRuntimeStore::new()),
        &["parked holder payload", "behind the parked holder"],
        true,
    )
    .await;
    let rig = &restarted.rig;
    let holder = restarted.waiting[0].clone();
    let behind = restarted.waiting[1].clone();
    rig.script.prepare_hold_armed.store(true, Ordering::SeqCst);
    rig.wait_for_waiting_delivery().await;

    let steps_before = rig.script.steps_done.load(Ordering::SeqCst);
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rig.script.prepare_hold_reached.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the waiting steer's preparation parks the runner");
    assert!(
        recall(rig, &holder)
            .await
            .is_some_and(|answer| answer.abandoned())
    );
    rig.script
        .prepare_hold_released
        .store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(5), async {
        while rig.script.steps_done.load(Ordering::SeqCst) <= steps_before {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the runner step completes");
    assert_eq!(
        last_take_ids(rig),
        vec![behind.clone()],
        "the steer behind the recalled holder joins; the recalled holder does not"
    );

    discard_the_stood_tail(&restarted).await;
    rig.script.step(RunnerStep::Finish);
    for input_id in [&restarted.batch, &behind] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    assert_eq!(rig.script.applied_durable(), vec![behind]);
    assert_eq!(deliveries(rig, &holder), 0);
    assert_eq!(
        rig.phase(&holder).await,
        Some(InputLifecycleState::Abandoned)
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 1);
}

/// The resumed run's next boundary refuses durable deliveries (an
/// extraction boundary). As for a steer admitted live, the steer that waited
/// keeps its steer-lane place, the later one follows it, and one follow-up
/// turn delivers both in admission order.
#[tokio::test]
async fn resumed_run_whose_boundary_refuses_durable_deliveries_leaves_the_steers_to_one_follow_up()
{
    let restarted = restart_with_waiting_steers(
        Arc::new(crate::store::InMemoryRuntimeStore::new()),
        &["waited for a refusing boundary E"],
        true,
    )
    .await;
    let rig = &restarted.rig;
    let later = peer_steer("admitted before the refusing boundary F");
    let f_id = later.id().clone();
    rig.admit(later).await;
    rig.wait_for_waiting_delivery().await;

    rig.script.step(RunnerStep::ExtractionBoundary);
    tokio::time::timeout(Duration::from_secs(5), async {
        while rig.state.has_waiting_delivery_for_test() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the boundary refuses the durable delivery");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut waiting = restarted.waiting.clone();
    waiting.push(f_id);
    assert_eq!(
        rig.steer_queue().await,
        waiting,
        "both keep their steer-lane place, in admission order"
    );
    assert_one_follow_up_turn_delivers(&restarted, &waiting).await;
}

/// The resumed run ends before any boundary: the steer that waited and the
/// later one take one follow-up turn in admission order, as today.
#[tokio::test]
async fn resumed_run_that_ends_before_a_boundary_leaves_the_steers_to_one_follow_up() {
    let restarted = restart_with_waiting_steers(
        Arc::new(crate::store::InMemoryRuntimeStore::new()),
        &["waited for a run that ended first E"],
        true,
    )
    .await;
    let rig = &restarted.rig;
    let later = peer_steer("admitted before the run ended F");
    let f_id = later.id().clone();
    rig.admit(later).await;
    settle_live_boundary_work(rig).await;
    let mut waiting = restarted.waiting.clone();
    waiting.push(f_id);
    assert_one_follow_up_turn_delivers(&restarted, &waiting).await;
}

/// A steer whose stored row counts a stage attempt was staged once and came
/// back to its lane before the restart; the row is stood by the rig. It does
/// not get its place in the resumed run back: the resumed run's next request
/// carries neither it nor a steer admitted behind it, and one follow-up turn
/// delivers both in admission order.
#[tokio::test]
async fn steer_with_a_stage_attempt_before_the_restart_takes_the_follow_up_turn() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let abandoned =
        abandon_with_waiting_steers(&store, &["staged once before the restart E"], true).await;
    let e_id = abandoned.waiting[0].clone();
    let runtime_id = MeerkatMachine::logical_runtime_id(&abandoned.session_id);
    let mut row = store
        .load_input_state(&runtime_id, &e_id)
        .await
        .expect("load E's row")
        .expect("E's row is stored");
    row.seed.attempt_count = 1;
    store
        .persist_input_state(
            &runtime_id,
            &crate::input_state::InputStatePersistenceRecord::from_machine_snapshot(row)
                .expect("a queued row with one attempt"),
        )
        .await
        .expect("stand E's row with one stage attempt");

    let restarted = restart_over(store, abandoned).await;
    let rig = &restarted.rig;
    let later = peer_steer("admitted during the resumed run F");
    let f_id = later.id().clone();
    rig.admit(later).await;
    settle_live_boundary_work(rig).await;
    assert!(
        !rig.state.has_waiting_delivery_for_test(),
        "no delivery registers for the resumed run's next boundary"
    );
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(
        last_take_ids(rig),
        Vec::<InputId>::new(),
        "the resumed run's next request carries neither"
    );
    assert_one_follow_up_turn_delivers(&restarted, &[e_id, f_id]).await;
}

/// The steer that waited joins the resumed run, and that runtime is
/// abandoned too before any round commit holds its append. A third runtime
/// over the same store resumes the same run once more. The steer does not
/// join that run a second time: one follow-up turn delivers it after the
/// run, exactly once.
#[tokio::test]
async fn second_restart_after_the_waiting_steer_joined_delivers_it_exactly_once() {
    let restarted = restart_with_waiting_steers(
        Arc::new(crate::store::InMemoryRuntimeStore::new()),
        &["joined and then lost with the second runtime E"],
        true,
    )
    .await;
    let Restarted {
        rig,
        store,
        batch,
        crashed_run,
        waiting,
    } = restarted;
    let e_id = waiting[0].clone();
    rig.wait_for_waiting_delivery().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(last_take_ids(&rig), vec![e_id.clone()]);
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let joined = store
        .load_input_state(&runtime_id, &e_id)
        .await
        .expect("load E's row")
        .expect("E's row is stored");
    assert_eq!(joined.seed.phase, InputLifecycleState::Staged);
    assert_eq!(joined.seed.last_run_id.as_ref(), Some(&crashed_run));

    // The second runtime is never driven again. The stood tail still names
    // the batch input alone: no round commit held E's append.
    let session_id = rig.session_id.clone();
    std::mem::forget(rig);
    let third = DurableSteerRig::with_adapter_for_session(
        Arc::new(
            MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
                .expect("persistent machine"),
        ),
        session_id.clone(),
    )
    .await;
    third.wait_for_apply_calls(1).await;
    assert_eq!(
        third.script.applied_runs.lock().unwrap().clone(),
        vec![crashed_run],
        "the same run resumes once more"
    );
    settle_live_boundary_work(&third).await;
    third
        .script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(
        last_take_ids(&third),
        Vec::<InputId>::new(),
        "a steer that had joined the run does not get its place in it back"
    );
    discard_the_stood_tail_of(&store, &session_id).await;
    drain(&third, &[batch.clone(), e_id.clone()]).await;
    assert_eq!(
        third.script.primitives(),
        vec![vec![batch], vec![e_id.clone()]],
        "one follow-up turn delivers it after the resumed run"
    );
    assert!(third.script.applied_durable().is_empty());
    assert_eq!(
        deliveries(&third, &e_id),
        1,
        "the steer is delivered exactly once after the second restart"
    );
}

/// SQLite: the store refuses the write that binds the steer that waited to
/// the resumed run. Nothing is published, the store keeps the steer queued
/// and the runtime is fenced until a reload, as for a steer admitted live.
/// A fresh runtime over the same store then delivers it exactly once.
#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_refused_join_of_a_steer_that_waited_is_not_published_and_a_reload_delivers_it_once()
{
    use crate::store::RuntimeStore;
    let directory = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(
        crate::store::SqliteRuntimeStore::new(directory.path().join("runtime.sqlite")).unwrap(),
    );
    let restarted =
        restart_with_waiting_steers(sqlite.clone(), &["refused join after the restart E"], true)
            .await;
    let Restarted {
        rig,
        store,
        batch,
        waiting,
        ..
    } = restarted;
    let e_id = waiting[0].clone();
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    rig.wait_for_waiting_delivery().await;
    let connection = rusqlite::Connection::open(sqlite.path()).unwrap();
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER refuse_join BEFORE UPDATE ON runtime_input_states
             WHEN NEW.runtime_id = '{runtime_id}' AND NEW.input_id = '{e_id}'
             BEGIN SELECT RAISE(ABORT, 'join persist failure'); END;"
        ))
        .unwrap();

    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert!(
        last_take_ids(&rig).is_empty(),
        "a join that is not durable is not published"
    );
    let row = sqlite
        .load_input_state(&runtime_id, &e_id)
        .await
        .expect("load the steer's row")
        .expect("the steer's row is stored");
    assert_eq!(
        row.seed.phase,
        InputLifecycleState::Queued,
        "the store keeps the steer queued"
    );
    connection
        .execute_batch("DROP TRIGGER refuse_join;")
        .unwrap();
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        rig.adapter
            .accept_input_with_completion(&rig.session_id, peer_steer("after the refusal")),
    )
    .await
    .expect("the fenced runtime answers an admission at once");
    match refused {
        Err(RuntimeDriverError::RecoveryRepairBlocked { reason, .. }) => assert!(
            reason.contains("live_boundary_join_commit"),
            "the refusal names the join write: {reason}"
        ),
        Err(other) => panic!("the fenced runtime refused with another error: {other:?}"),
        Ok(_) => panic!("the fenced runtime admitted a new input"),
    }

    // A fresh runtime reloads the session from the store. The fenced runtime
    // is never driven again.
    let session_id = rig.session_id.clone();
    std::mem::forget(rig);
    let reloaded = DurableSteerRig::with_adapter_for_session(
        Arc::new(
            MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
                .expect("persistent machine"),
        ),
        session_id.clone(),
    )
    .await;
    reloaded.wait_for_apply_calls(1).await;
    discard_the_stood_tail_of(&store, &session_id).await;
    drain(&reloaded, &[batch, e_id.clone()]).await;
    assert_eq!(
        deliveries(&reloaded, &e_id),
        1,
        "the steer is delivered exactly once after the reload"
    );
}
