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

async fn enqueue_stop(handle: &MobHandle) -> oneshot::Receiver<Result<(), MobError>> {
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
    tokio::spawn(async move { handle.stop().await })
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

async fn expect_reply_ok(reply: oneshot::Receiver<Result<(), MobError>>, context: &str) {
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

fn still_waiting(reply: &mut oneshot::Receiver<Result<(), MobError>>) -> bool {
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

#[tokio::test]
async fn shutdown_waits_for_a_member_turn_and_a_second_shutdown_joins_it() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let session = spawn_winding_down_member(&handle, &service, "shutdown-wind-down").await;

    let first = tokio::spawn({
        let handle = handle.clone();
        async move { handle.shutdown().await }
    });
    stop_awaits(std::slice::from_ref(&session)).await;
    let interrupts = service.interrupt_call_count_for(&session);
    let mut second = handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::Shutdown { reply_tx })
        .await
        .expect("enqueue second shutdown");
    assert_eq!(actor_round_trip(&handle).await, MobState::Running);
    assert!(
        still_waiting(&mut second),
        "the second shutdown waits on the pending one"
    );

    service.set_session_active(&session, false).await;
    // The joined command receives the pending shutdown's own result: done,
    // or the retryable runtime-unregister lane the handle retries (#1413).
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
    expect_task_ok(first, "first shutdown").await;
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
    service.set_start_turn_delay_ms(600_000);
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
    wait_for_start_turn_call_count(
        service.as_ref(),
        1,
        "the member's turn is in flight, so the stop must interrupt it",
    )
    .await;
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
