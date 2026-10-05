//! #1390: a mob Stop/Shutdown awaits each interrupted member's end of turn as
//! a typed signal, concurrently and off the actor loop, instead of polling
//! `is_active` on a fixed 1 s budget per member.

use super::*;
use crate::runtime::actor::stop_idle_wait_probe;

/// Bound for test steps that must happen promptly; it only turns a hang into
/// a failure.
const STEP: Duration = Duration::from_secs(10);

/// Spawn an autonomous member whose session reports an active turn that
/// keeps winding down after its stop interrupt, until the test ends it.
async fn spawn_winding_down_member(
    handle: &MobHandle,
    service: &MockSessionService,
    name: &str,
) -> SessionId {
    let identity = AgentIdentity::from(name);
    let mut spec = SpawnMemberSpec::new("worker", identity.as_str());
    spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
    handle
        .spawn_spec(spec)
        .await
        .expect("spawn autonomous member");
    let session_id = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("session-backed autonomous member");
    service.set_session_active(&session_id, true).await;
    service.hold_wind_down_after_interrupt(&session_id).await;
    session_id
}

async fn enqueue_stop(
    handle: &MobHandle,
) -> oneshot::Receiver<Result<crate::MobStopReport, MobError>> {
    handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::Stop { reply_tx })
        .await
        .expect("enqueue stop")
}

type LifecycleTask = tokio::task::JoinHandle<Result<(), MobError>>;

/// Start a Stop through the handle. The handle still drives the exact
/// interrupts' level-triggered lane (#1413); once they land, the Stop parks on
/// the members' end of turn.
fn start_stop(handle: &MobHandle) -> LifecycleTask {
    let handle = handle.clone();
    tokio::spawn(async move { handle.stop().await.map(|_| ()) })
}

/// Wait until a stop is parked on the end of turn of every listed session.
async fn stop_awaits(sessions: &[SessionId]) {
    tokio::time::timeout(STEP, stop_idle_wait_probe::all_waiting(sessions))
        .await
        .expect("the stop awaits every interrupted member's end of turn");
}

/// One actor round trip. Commands are served in order, so once this answers,
/// every command sent before it has been routed.
async fn actor_round_trip(handle: &MobHandle) -> MobState {
    let phase = handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::QueryPhase { reply_tx })
        .await
        .expect("enqueue phase query");
    tokio::time::timeout(STEP, phase)
        .await
        .expect("the actor keeps serving commands while the stop waits")
        .expect("phase reply")
        .expect("phase query")
}

async fn expect_reply_ok<T>(reply: oneshot::Receiver<Result<T, MobError>>, context: &str) {
    tokio::time::timeout(STEP, reply)
        .await
        .unwrap_or_else(|_| panic!("{context}: completes once the turns end"))
        .expect("reply")
        .unwrap_or_else(|error| panic!("{context}: failed: {error}"));
}

async fn expect_task_ok(task: LifecycleTask, context: &str) {
    tokio::time::timeout(STEP, task)
        .await
        .unwrap_or_else(|_| panic!("{context}: completes once the turns end"))
        .expect("lifecycle task")
        .unwrap_or_else(|error| panic!("{context}: failed: {error}"));
}

fn still_waiting<T>(reply: &mut oneshot::Receiver<Result<T, MobError>>) -> bool {
    matches!(
        reply.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    )
}

#[tokio::test]
async fn stop_waits_for_a_member_turn_that_winds_down_past_one_second() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let session = spawn_winding_down_member(&handle, &service, "slow-wind-down").await;

    let stop = start_stop(&handle);
    stop_awaits(std::slice::from_ref(&session)).await;
    // A Stop command sent now meets the parked stop. The removed poll answered
    // it with LifecycleOperationPending after 40 x 25 ms; past that budget it
    // is still waiting on the turn, and the actor still serves commands.
    let mut direct = enqueue_stop(&handle).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(1_500), &mut direct)
            .await
            .is_err(),
        "a turn that is slow to wind down must not fail the stop"
    );
    assert_eq!(actor_round_trip(&handle).await, MobState::Running);

    service.set_session_active(&session, false).await;
    expect_task_ok(stop, "slow wind-down stop").await;
    expect_reply_ok(direct, "slow wind-down direct stop").await;
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
}

#[tokio::test]
async fn stop_awaits_every_member_end_of_turn_concurrently() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let first = spawn_winding_down_member(&handle, &service, "concurrent-a").await;
    let second = spawn_winding_down_member(&handle, &service, "concurrent-b").await;
    let third = spawn_winding_down_member(&handle, &service, "concurrent-c").await;

    let stop = start_stop(&handle);
    // All three waits are parked at once: the stop does not wait member by
    // member.
    stop_awaits(&[first.clone(), second.clone(), third.clone()]).await;
    let mut direct = enqueue_stop(&handle).await;

    // End the turns in reverse order; the stop completes with the last.
    service.set_session_active(&third, false).await;
    service.set_session_active(&second, false).await;
    assert_eq!(actor_round_trip(&handle).await, MobState::Running);
    assert!(
        still_waiting(&mut direct),
        "the stop still waits for the member whose turn has not ended"
    );
    service.set_session_active(&first, false).await;
    expect_task_ok(stop, "concurrent wind-down stop").await;
    expect_reply_ok(direct, "concurrent wind-down direct stop").await;
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
}

#[tokio::test]
async fn a_second_stop_joins_the_pending_stop() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let session = spawn_winding_down_member(&handle, &service, "join-stop").await;

    let first = start_stop(&handle);
    stop_awaits(std::slice::from_ref(&session)).await;
    let interrupts = service.interrupt_call_count_for(&session);
    let mut second = enqueue_stop(&handle).await;
    actor_round_trip(&handle).await;
    assert!(
        still_waiting(&mut second),
        "the second stop waits on the pending one"
    );

    service.set_session_active(&session, false).await;
    expect_task_ok(first, "joined stop, first").await;
    expect_reply_ok(second, "joined stop, second").await;
    assert_eq!(
        service.interrupt_call_count_for(&session),
        interrupts,
        "a joined stop does not interrupt the member again"
    );
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
}

#[tokio::test]
async fn shutdown_during_a_pending_stop_runs_after_it() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let session = spawn_winding_down_member(&handle, &service, "stop-then-shutdown").await;

    let stop = start_stop(&handle);
    stop_awaits(std::slice::from_ref(&session)).await;
    // Through the handle: a Shutdown has its own level-triggered interrupt
    // and runtime-unregister lanes once it runs (#1413).
    let shutdown = tokio::spawn({
        let handle = handle.clone();
        async move { handle.shutdown().await }
    });
    actor_round_trip(&handle).await;
    assert!(
        !shutdown.is_finished(),
        "shutdown waits for the pending stop"
    );

    service.set_session_active(&session, false).await;
    expect_task_ok(stop, "stop before shutdown").await;
    expect_task_ok(shutdown, "shutdown after the stop").await;
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
}

#[tokio::test]
async fn resume_during_a_pending_stop_runs_after_it() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let session = spawn_winding_down_member(&handle, &service, "stop-then-resume").await;

    let stop = start_stop(&handle);
    stop_awaits(std::slice::from_ref(&session)).await;
    let resume = tokio::spawn({
        let handle = handle.clone();
        async move { handle.resume().await }
    });
    actor_round_trip(&handle).await;
    assert!(!resume.is_finished(), "resume waits for the pending stop");

    service.set_session_active(&session, false).await;
    expect_task_ok(stop, "stop before resume").await;
    // Resume runs against the Stopped mob the stop left, so it is admitted
    // rather than refused as a resume of a running mob.
    expect_task_ok(resume, "resume after the stop").await;
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn destroy_during_a_pending_stop_runs_after_it() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let session = spawn_winding_down_member(&handle, &service, "stop-then-destroy").await;

    let stop = start_stop(&handle);
    stop_awaits(std::slice::from_ref(&session)).await;
    let destroy = tokio::spawn({
        let handle = handle.clone();
        async move { handle.destroy().await }
    });
    actor_round_trip(&handle).await;
    assert!(!destroy.is_finished(), "destroy waits for the pending stop");

    service.set_session_active(&session, false).await;
    expect_task_ok(stop, "stop before destroy").await;
    tokio::time::timeout(STEP, destroy)
        .await
        .expect("destroy completes after the stop")
        .expect("destroy task")
        .expect("destroy");
    assert_eq!(handle.status().await.unwrap(), MobState::Destroyed);
}

/// Shutdown cancels a member's in-flight turn immediately (it does not wait
/// for the turn's next boundary), reports the run as cancelled by shutdown,
/// and a second Shutdown arriving meanwhile joins it and receives its result
/// without interrupting the member again.
#[tokio::test]
async fn shutdown_cancels_a_member_turn_and_a_second_shutdown_joins_it() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let session = spawn_winding_down_member(&handle, &service, "shutdown-wind-down").await;
    let identity = AgentIdentity::from("shutdown-wind-down");
    // The member's kickoff turn is running: the runtime has its run current.
    tokio::time::timeout(STEP, service.wait_keep_alive_turn_entered(&session))
        .await
        .expect("the kickoff turn starts");
    let adapter = MobSessionService::runtime_adapter(service.as_ref())
        .expect("the test mob is runtime-backed");
    let kickoff_run = adapter
        .current_run(&session)
        .await
        .expect("the kickoff turn's run is current");

    let first = tokio::spawn({
        let handle = handle.clone();
        async move {
            handle
                .shutdown_with_report(crate::runtime::ShutdownOptions::default())
                .await
        }
    });
    stop_awaits(std::slice::from_ref(&session)).await;
    let interrupts = service.interrupt_call_count_for(&session);
    let mut second = handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::Shutdown {
            deadline: None,
            reply_tx,
        })
        .await
        .expect("enqueue second shutdown");
    assert_eq!(actor_round_trip(&handle).await, MobState::Running);
    assert!(
        still_waiting(&mut second),
        "the second shutdown waits on the pending one"
    );

    service.set_session_active(&session, false).await;
    let joined = tokio::time::timeout(STEP, second)
        .await
        .expect("the joined shutdown answers once the turn ends")
        .expect("joined shutdown reply");
    assert!(
        matches!(
            joined,
            Ok(()) | Err(MobError::LifecycleOperationPending { .. })
        ),
        "the joined shutdown carries the pending shutdown's result: {joined:?}"
    );
    let report = tokio::time::timeout(STEP, first)
        .await
        .expect("the first shutdown completes")
        .expect("first shutdown task")
        .expect("first shutdown");
    assert_eq!(
        report.runs.get(&identity),
        Some(
            &crate::runtime::stop_report::MemberStopRun::CancelledByShutdown {
                run_id: kickoff_run
            }
        ),
        "the shutdown cancelled the member's turn immediately: {:?}",
        report.runs
    );
    assert_eq!(
        report.run_starts.get(&identity),
        Some(&crate::runtime::stop_report::MemberRunStarts::Held),
        "the shutdown held the local member's run starts before its interrupt"
    );
    assert_eq!(
        service.interrupt_call_count_for(&session),
        interrupts,
        "a joined shutdown does not interrupt the member again"
    );
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
}

#[tokio::test]
async fn a_stop_command_parks_on_its_in_flight_interrupt_and_completes_when_it_settles() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("gated-interrupt");
    let mut spec = SpawnMemberSpec::new("worker", identity.as_str());
    spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
    handle
        .spawn_spec(spec)
        .await
        .expect("spawn autonomous member");
    let session = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("session-backed autonomous member");
    // The member's keep-alive turn is running and ends at its boundary when
    // the stop's cancel lands (a turn the boundary cancel cannot end is
    // correctly waited on by the run-settled stop). Property from #1452.
    tokio::time::timeout(STEP, service.wait_keep_alive_turn_entered(&session))
        .await
        .expect("the member's turn is in flight, so the stop must interrupt it");
    let gate = service.install_interrupt_gate(&session).await;

    // A Stop command with no handle retry around it: the exact interrupt is
    // still in flight, and the stop parks on it instead of answering
    // AutonomousStopInterruptsPending.
    let mut stop = enqueue_stop(&handle).await;
    tokio::time::timeout(STEP, service.interrupt_gate_entered.notified())
        .await
        .expect("the stop's interrupt reaches the member");
    assert_eq!(actor_round_trip(&handle).await, MobState::Running);
    assert!(
        still_waiting(&mut stop),
        "the stop waits on its in-flight interrupt"
    );

    gate.release_all();
    expect_reply_ok(stop, "stop after its interrupt settles").await;
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
}

/// A stopped member's session reports its turn over before the runtime
/// records the interrupted run's end. The Stop resolves only once the runtime
/// has, so when it returns no run is current, the Resume that follows runs
/// against a member with no stopped run left over, and nothing interrupts the
/// member again.
#[tokio::test]
async fn stop_resolves_once_the_runtime_records_the_run_end_and_resume_follows() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let session = spawn_winding_down_member(&handle, &service, "stop-run-settlement").await;
    let adapter = MobSessionService::runtime_adapter(service.as_ref())
        .expect("the test mob is runtime-backed");

    let stop = start_stop(&handle);
    stop_awaits(std::slice::from_ref(&session)).await;
    let interrupts = service.interrupt_call_count_for(&session);
    service.set_session_active(&session, false).await;
    expect_task_ok(stop, "stop once the member's turn ends").await;
    assert_eq!(
        adapter.current_run(&session).await,
        None,
        "the stop returned only after the runtime recorded the interrupted run's end"
    );
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);

    tokio::time::timeout(STEP, handle.resume())
        .await
        .expect("resume completes")
        .expect("resume after the stop");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    assert_eq!(
        service.interrupt_call_count_for(&session),
        interrupts,
        "neither the settled stop nor the resume interrupts the member again"
    );
    handle.shutdown().await.expect("shutdown");
}

/// Shutdown's immediate cancel can leave its outcome to the runtime: here the
/// executor's interrupt callback outlasts the acknowledgement bound, so the
/// dispatch reports `InterruptDispatchOutcomeUnknown` while the run is still
/// current, and the run then ends on its own. The Shutdown does not fail on
/// the pending outcome: it waits for the run's recorded end and reports the
/// run from its recorded terminal, here the run's own end before the cancel.
#[tokio::test]
async fn shutdown_reports_a_run_that_ended_before_its_cancel_from_the_recorded_terminal() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("ended-before-cancel");
    let mut spec = SpawnMemberSpec::new("worker", identity.as_str());
    spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
    handle
        .spawn_spec(spec)
        .await
        .expect("spawn autonomous member");
    let session = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("session-backed autonomous member");
    tokio::time::timeout(STEP, service.wait_keep_alive_turn_entered(&session))
        .await
        .expect("the kickoff turn starts");
    let adapter = MobSessionService::runtime_adapter(service.as_ref())
        .expect("the test mob is runtime-backed");
    let run = adapter
        .current_run(&session)
        .await
        .expect("the kickoff turn's run is current");

    // The executor's interrupt callback wedges until the test releases it,
    // so the Shutdown's hard cancel reports its outcome unknown once this
    // machine's acknowledgement bound passes.
    adapter.set_user_interrupt_ack_timeout_for_test(Duration::from_millis(50));
    let control = service.install_runtime_control_barrier().await;
    struct ReleaseControl(Arc<TestRuntimeControlBarrier>);
    impl Drop for ReleaseControl {
        fn drop(&mut self) {
            self.0.release_all();
        }
    }
    let release_control = ReleaseControl(Arc::clone(&control));

    let shutdown = tokio::spawn({
        let handle = handle.clone();
        async move {
            handle
                .shutdown_with_report(crate::runtime::ShutdownOptions::default())
                .await
        }
    });
    tokio::time::timeout(STEP, control.wait_hard_call_entered())
        .await
        .expect("the Shutdown's hard cancel reaches the member's executor");
    assert_eq!(
        adapter.current_run(&session).await,
        Some(run.clone()),
        "the cancel was dispatched while the run was current"
    );

    // The turn returns on its own before the wedged cancel lands; its run
    // ends completed.
    service.release_keep_alive_turn(&session).await;
    let report = tokio::time::timeout(STEP, shutdown)
        .await
        .expect("the Shutdown completes once the run's end is recorded")
        .expect("shutdown task")
        .expect("the Shutdown does not fail on the cancel's pending outcome");
    assert_eq!(
        report.runs.get(&identity),
        Some(&crate::runtime::stop_report::MemberStopRun::RunEndedBeforeCancel { run_id: run }),
        "the run is reported from its recorded terminal: {:?}",
        report.runs
    );
    assert_eq!(adapter.current_run(&session).await, None);
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
    drop(release_control);
    service.clear_runtime_control_barrier().await;
}

#[tokio::test]
async fn the_hang_guard_reports_the_member_still_winding_down() {
    let identity = AgentIdentity::from("never-winds-down");
    let error = crate::runtime::actor::member_stop_within_hang_guard(
        &identity,
        Duration::from_millis(20),
        std::future::pending(),
    )
    .await
    .expect_err("a turn that never ends trips the hang guard");
    assert!(
        matches!(
            &error,
            MobError::LifecycleOperationProgressStalled {
                member_id: Some(member),
                stage: "autonomous_member_stop_idle",
                ..
            } if member == &identity
        ),
        "the hang guard names the member still active: {error:?}"
    );
}

#[tokio::test]
async fn shutdown_during_a_resume_rollback_completes_once_the_member_turn_ends() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("rollback-wind-down");
    let mut spec = SpawnMemberSpec::new("worker", identity.as_str());
    spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
    handle
        .spawn_spec(spec)
        .await
        .expect("spawn autonomous member");
    let session = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("session-backed autonomous member");
    handle.stop().await.expect("stop before the failing resume");

    // The resume's readiness fails on the member's missing comms runtime, so
    // the resume rolls back by stopping the member again; that member's turn
    // is slow to wind down.
    service.set_session_active(&session, true).await;
    service.hold_wind_down_after_interrupt(&session).await;
    service.set_missing_comms_runtime(&session).await;
    let resume = tokio::spawn({
        let handle = handle.clone();
        async move { handle.resume().await }
    });
    stop_awaits(std::slice::from_ref(&session)).await;

    let shutdown = tokio::spawn({
        let handle = handle.clone();
        async move { handle.shutdown().await }
    });
    actor_round_trip(&handle).await;
    assert!(!shutdown.is_finished(), "shutdown waits for the rollback");

    service.set_session_active(&session, false).await;
    let resumed = tokio::time::timeout(STEP, resume)
        .await
        .expect("the rolled-back resume answers once the turn ends")
        .expect("resume task");
    assert!(
        resumed.is_err(),
        "the resume reports its readiness failure after rolling back"
    );
    tokio::time::timeout(STEP, shutdown)
        .await
        .expect("shutdown completes after the rollback")
        .expect("shutdown task")
        .expect("shutdown");
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
}

/// #1494: a handle whose actor side is played by the test. Its commands land
/// on `commands` and its machine-state view is `state_tx`; the real mob only
/// supplies the rest of the handle.
async fn handle_with_scripted_actor(
    pending: impl FnOnce(&mut crate::machines::mob_machine::MobMachineState),
) -> (
    MobHandle,
    mpsc::Receiver<crate::runtime::scope_gate::RoutedMobCommand>,
    tokio::sync::watch::Sender<crate::machines::mob_machine::MobMachineState>,
    crate::machines::mob_machine::MobMachineState,
) {
    let (real, _service) = create_test_mob(sample_definition()).await;
    let drained = real.machine_state_watch_rx.borrow().clone();
    let mut blocked = drained.clone();
    pending(&mut blocked);
    let (state_tx, state_rx) = tokio::sync::watch::channel(blocked);
    let (command_tx, commands) = mpsc::channel(8);
    let mut handle = real.clone();
    handle.command_tx = command_tx;
    handle.machine_state_watch_rx = state_rx;
    (handle, commands, state_tx, drained)
}

async fn refuse_next_stop(
    commands: &mut mpsc::Receiver<crate::runtime::scope_gate::RoutedMobCommand>,
    refusal: MobError,
) {
    let routed = tokio::time::timeout(STEP, commands.recv())
        .await
        .expect("the handle sends Stop")
        .expect("command channel open");
    let MobCommand::Stop { reply_tx } = routed.cmd else {
        panic!("expected Stop, got {}", routed.cmd.kind());
    };
    let _ = reply_tx.send(Err(refusal));
}

/// #1494: a Stop the actor refuses because placed completion cleanup is still
/// settling is re-issued exactly once, when that cleanup drains, and never
/// on a retry cadence. The clock is paused: the old 25 ms resend loop would
/// re-send during the idle barrier, and a timer-driven re-issue would make
/// virtual time pass after the drain.
#[tokio::test(start_paused = true)]
async fn a_stop_refused_on_placed_completion_cleanup_reissues_once_when_it_drains() {
    let (handle, mut commands, state_tx, drained) = handle_with_scripted_actor(|state| {
        state
            .pending_placed_completion_outcomes
            .insert(crate::machines::mob_machine::PlacedCompletionObligation::default());
    })
    .await;
    let stop = tokio::spawn(async move { handle.stop().await });
    refuse_next_stop(
        &mut commands,
        MobError::PlacedCompletionCleanupPending {
            pending: 1,
            resolved: 0,
        },
    )
    .await;

    // Run until idle: on the paused clock this returns once the handle has
    // taken the refusal and parked.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        commands.try_recv().is_err(),
        "no Stop is re-sent while the placed completion cleanup is pending"
    );
    assert!(!stop.is_finished());

    let drained_at = tokio::time::Instant::now();
    state_tx.send_replace(drained);
    let routed = tokio::time::timeout(STEP, commands.recv())
        .await
        .expect("Stop is re-issued when the cleanup drains")
        .expect("command channel open");
    assert_eq!(
        tokio::time::Instant::now(),
        drained_at,
        "the drain re-issued the Stop, not a timer"
    );
    let MobCommand::Stop { reply_tx } = routed.cmd else {
        panic!("expected Stop, got {}", routed.cmd.kind());
    };
    let _ = reply_tx.send(Ok(crate::MobStopReport::default()));
    stop.await
        .expect("stop task does not panic")
        .expect("the re-issued Stop completes");
    assert!(commands.try_recv().is_err(), "exactly one re-issue");
}

/// #1494: Shutdown refused on placed kickoff cleanup waits for those custody
/// sets to drain, then re-issues once.
#[tokio::test(start_paused = true)]
async fn a_shutdown_refused_on_placed_kickoff_cleanup_reissues_once_when_it_drains() {
    let (handle, mut commands, state_tx, drained) = handle_with_scripted_actor(|state| {
        state
            .resolved_placed_kickoff_outcomes
            .insert(crate::machines::mob_machine::PlacedKickoffObligation::default());
    })
    .await;
    let shutdown = tokio::spawn(async move { handle.shutdown().await });
    let routed = tokio::time::timeout(STEP, commands.recv())
        .await
        .expect("the handle sends Shutdown")
        .expect("command channel open");
    let MobCommand::Shutdown { reply_tx, .. } = routed.cmd else {
        panic!("expected Shutdown, got {}", routed.cmd.kind());
    };
    let _ = reply_tx.send(Err(MobError::PlacedKickoffCleanupPending {
        pending: 0,
        resolved: 1,
    }));

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        commands.try_recv().is_err(),
        "no Shutdown is re-sent while placed kickoff cleanup is pending"
    );
    let drained_at = tokio::time::Instant::now();
    state_tx.send_replace(drained);
    let routed = tokio::time::timeout(STEP, commands.recv())
        .await
        .expect("Shutdown is re-issued when the cleanup drains")
        .expect("command channel open");
    assert_eq!(tokio::time::Instant::now(), drained_at);
    let MobCommand::Shutdown { reply_tx, .. } = routed.cmd else {
        panic!("expected Shutdown, got {}", routed.cmd.kind());
    };
    let _ = reply_tx.send(Ok(()));
    shutdown
        .await
        .expect("shutdown task does not panic")
        .expect("the re-issued Shutdown completes");
}

/// #1494: a Stop refused because another lifecycle operation is settling is
/// re-issued once at once (the state that lets it succeed may already be
/// committed), and a repeated refusal is re-issued only when the machine
/// commits a later transition, never on a timer.
#[tokio::test(start_paused = true)]
async fn a_stop_refused_on_a_pending_lifecycle_operation_reissues_on_the_next_commit() {
    let (handle, mut commands, state_tx, drained) = handle_with_scripted_actor(|_| {}).await;
    let stop = tokio::spawn(async move { handle.stop().await });
    let pending = || MobError::LifecycleOperationPending {
        intent: "member retirement".to_owned(),
    };
    refuse_next_stop(&mut commands, pending()).await;
    // The first refusal is re-evaluated at once.
    refuse_next_stop(&mut commands, pending()).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        commands.try_recv().is_err(),
        "a repeated refusal is not re-sent before the machine commits again"
    );
    let committed_at = tokio::time::Instant::now();
    state_tx.send_replace(drained);
    let routed = tokio::time::timeout(STEP, commands.recv())
        .await
        .expect("Stop is re-issued on the next commit")
        .expect("command channel open");
    assert_eq!(tokio::time::Instant::now(), committed_at);
    let MobCommand::Stop { reply_tx } = routed.cmd else {
        panic!("expected Stop, got {}", routed.cmd.kind());
    };
    let _ = reply_tx.send(Ok(crate::MobStopReport::default()));
    stop.await
        .expect("stop task does not panic")
        .expect("the re-issued Stop completes");
}

/// #1494: the hang guard, not a retry budget, bounds the wait. A cleanup that
/// never drains returns the actor's refusal once the guard elapses, after
/// exactly one Stop. The scripted actor refuses every Stop it receives, so a
/// resend loop fails the count instead of hanging.
#[tokio::test(start_paused = true)]
async fn a_stop_whose_blocker_never_settles_returns_the_refusal_at_the_hang_guard() {
    let (handle, mut commands, _state_tx, _drained) = handle_with_scripted_actor(|state| {
        state
            .pending_placed_completion_outcomes
            .insert(crate::machines::mob_machine::PlacedCompletionObligation::default());
    })
    .await;
    let stops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let refuser_stops = Arc::clone(&stops);
    let refuser = tokio::spawn(async move {
        while let Some(routed) = commands.recv().await {
            if let MobCommand::Stop { reply_tx } = routed.cmd {
                refuser_stops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = reply_tx.send(Err(MobError::PlacedCompletionCleanupPending {
                    pending: 1,
                    resolved: 0,
                }));
            }
        }
    });
    let started = tokio::time::Instant::now();
    let outcome = handle.stop().await;
    assert!(
        matches!(
            outcome,
            Err(MobError::PlacedCompletionCleanupPending { .. })
        ),
        "the refusal stands at the hang guard, got {outcome:?}"
    );
    assert!(started.elapsed() >= Duration::from_secs(600));
    assert_eq!(
        stops.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "no Stop is re-sent while the cleanup never drains"
    );
    refuser.abort();
}
