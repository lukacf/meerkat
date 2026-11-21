//! D67: public ownership of a running mob and the Closing admission state.
//! An idle close is decided on the actor's serialized admission path, so a
//! racing command is either admitted first (the close answers Busy) or
//! refused after it, never both; the actor's own clones never keep a mob
//! open, and a retained public handle does.

use super::*;
use crate::runtime::actor::spawn_preparation::SpawnPreparationTestGate;
use crate::runtime::admission_close::{CloseIfIdleOutcome, MobAdmissionCloseReason};

/// Bound for test steps that must happen promptly; it only turns a hang into
/// a failure.
const STEP: Duration = Duration::from_secs(10);

/// A turn-driven worker: it has no kickoff turn, so once seated it leaves
/// the mob idle. (An autonomous member's unresolved kickoff is work in
/// flight and keeps the mob busy.)
fn worker_spec(name: &str) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new(ProfileName::from("worker"), AgentIdentity::from(name));
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

fn assert_admission_closed(error: MobError, expected: MobAdmissionCloseReason) {
    match error {
        MobError::AdmissionClosed { reason, .. } => assert_eq!(reason, expected),
        other => panic!("expected AdmissionClosed ({expected}), got {other}"),
    }
}

/// Hold the named member's spawn preparation until a permit is added.
fn hold_spawn(identity: &str) -> Arc<tokio::sync::Semaphore> {
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    arm_spawn_preparation_gate_for_test(
        &AgentIdentity::from(identity),
        SpawnPreparationTestGate {
            entered: None,
            release: Some(Arc::clone(&release)),
            settled: None,
        },
    );
    release
}

/// [`hold_spawn`], also reporting when the named member's preparation has
/// entered the hold: the spawn is then admitted work in flight.
fn hold_spawn_entered(
    identity: &str,
) -> (
    Arc<tokio::sync::Semaphore>,
    tokio::sync::mpsc::UnboundedReceiver<AgentIdentity>,
) {
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (entered_tx, entered) = tokio::sync::mpsc::unbounded_channel();
    arm_spawn_preparation_gate_for_test(
        &AgentIdentity::from(identity),
        SpawnPreparationTestGate {
            entered: Some(entered_tx),
            release: Some(Arc::clone(&release)),
            settled: None,
        },
    );
    (release, entered)
}

/// Wait until the held spawn's preparation has entered its hold.
async fn spawn_entered(entered: &mut tokio::sync::mpsc::UnboundedReceiver<AgentIdentity>) {
    tokio::time::timeout(STEP, entered.recv())
        .await
        .expect("the spawn preparation enters its hold promptly")
        .expect("spawn preparation gate");
}

async fn enqueue_spawn(
    handle: &MobHandle,
    identity: &str,
) -> oneshot::Receiver<Result<crate::runtime::MemberSpawnReceipt, MobError>> {
    handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::Spawn {
            spec: Box::new(worker_spec(identity)),
            spawn_source: super::handle::SpawnSource::Consumer,
            owner_bridge_session_id: None,
            ops_registry: None,
            held: None,
            reply_tx,
        })
        .await
        .expect("enqueue spawn")
}

async fn enqueue_close_if_idle(
    handle: &MobHandle,
) -> oneshot::Receiver<Result<CloseIfIdleOutcome, MobError>> {
    handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::CloseIfIdle { reply_tx })
        .await
        .expect("enqueue close")
}

async fn reply<T>(reply: oneshot::Receiver<Result<T, MobError>>) -> Result<T, MobError> {
    tokio::time::timeout(STEP, reply)
        .await
        .expect("the actor answers promptly")
        .expect("reply channel")
}

/// One actor round trip, letting commands queued before it run. The owner
/// release is a separate wake branch, so a round trip does not prove when the
/// release was observed; the assertions that follow hold either way.
async fn actor_round_trip(handle: &MobHandle) {
    let phase = handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::QueryPhase { reply_tx })
        .await
        .expect("enqueue phase query");
    reply(phase).await.expect("phase query");
}

async fn await_closed(handle: &MobHandle) -> MobAdmissionCloseReason {
    let mut closed = handle.admission_closed();
    let waited = tokio::time::timeout(STEP, closed.wait_for(Option::is_some))
        .await
        .map(|closed| closed.map(|reason| *reason));
    match waited {
        Ok(Ok(Some(reason))) => reason,
        _ => {
            // Report what kept it open: a busy snapshot, or `None` when the
            // mob is idle and the owner release itself never arrived.
            let busy = handle
                .close_admission(MobAdmissionCloseReason::StandDown)
                .await;
            panic!("admission did not close; busy snapshot: {busy:?}");
        }
    }
}

/// A retired member's last execution observation is not work in flight: a
/// status read that saw an open run, then the member's retirement, leaves the
/// mob idle, so an idle close closes.
#[tokio::test]
async fn idle_close_ignores_a_retired_members_last_observed_work() {
    let (handle, service) = create_test_mob(with_unique_mob_id(
        sample_definition(),
        "d67-retired-observed-work",
    ))
    .await;
    let member = AgentIdentity::from("d67-retired-observed-work");
    handle
        .spawn_spec(worker_spec("d67-retired-observed-work"))
        .await
        .expect("spawn worker");
    // The status read observes an open run: one unit of in-flight work.
    service.set_execution_snapshots(std::iter::repeat_n(test_member_execution_snapshot(1, 0), 4));
    let progress = handle
        .member_status(&member)
        .await
        .expect("member status")
        .progress
        .expect("a local member's status carries progress");
    assert_eq!(progress.in_flight_work, 1);
    service.set_execution_snapshots(std::iter::empty());

    handle.retire(member).await.expect("retire worker");
    assert_eq!(
        handle.close_if_idle().await.expect("close if idle"),
        CloseIfIdleOutcome::Closed,
        "a retired member's last observed work must not keep the mob busy"
    );
}

#[tokio::test]
async fn idle_close_closes_admission_and_refuses_later_mutation() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    assert!(handle.admission_closed().borrow().is_none());

    assert_eq!(
        handle.close_if_idle().await.expect("close if idle"),
        CloseIfIdleOutcome::Closed
    );
    assert_eq!(
        *handle.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::IdleClose)
    );
    assert_admission_closed(
        handle
            .spawn_spec(worker_spec("d67-idle-late"))
            .await
            .expect_err("a spawn after the close is refused"),
        MobAdmissionCloseReason::IdleClose,
    );
    assert_eq!(
        handle.close_if_idle().await.expect("second close"),
        CloseIfIdleOutcome::AlreadyClosed(MobAdmissionCloseReason::IdleClose)
    );
    // A later unconditional close keeps the first reason.
    assert_eq!(
        handle
            .close_admission(MobAdmissionCloseReason::StandDown)
            .await
            .expect("close admission"),
        None
    );
    assert_eq!(
        *handle.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::IdleClose)
    );
    // Reads and the ordinary Stop stay admitted on a closed mob.
    assert!(
        handle
            .get_member(&AgentIdentity::from("d67-idle-late"))
            .await
            .expect("read")
            .is_none()
    );
    handle
        .stop()
        .await
        .expect("Stop is unchanged on a closed mob");
}

#[tokio::test]
async fn admission_racing_the_close_is_admitted_or_refused_never_both() {
    // Spawn first in mailbox order: admitted, so the close finds it in flight
    // and leaves it alone.
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let release = hold_spawn("d67-race-first");
    let spawn = enqueue_spawn(&handle, "d67-race-first").await;
    let close = enqueue_close_if_idle(&handle).await;
    match reply(close).await.expect("close answers") {
        CloseIfIdleOutcome::Busy(busy) => assert!(busy.roster_transitions > 0, "{busy:?}"),
        other => panic!("an admitted spawn keeps the mob busy, got {other:?}"),
    }
    release.add_permits(1);
    reply(spawn).await.expect("the admitted spawn completes");
    assert!(handle.admission_closed().borrow().is_none());
    assert!(
        handle
            .get_member(&AgentIdentity::from("d67-race-first"))
            .await
            .expect("read")
            .is_some()
    );
    handle.shutdown().await.expect("shutdown");

    // Close first: the mob closes and the spawn behind it is refused.
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let close = enqueue_close_if_idle(&handle).await;
    let spawn = enqueue_spawn(&handle, "d67-race-second").await;
    assert_eq!(
        reply(close).await.expect("close answers"),
        CloseIfIdleOutcome::Closed
    );
    assert_admission_closed(
        reply(spawn)
            .await
            .expect_err("the spawn after the close is refused"),
        MobAdmissionCloseReason::IdleClose,
    );
    assert!(
        handle
            .get_member(&AgentIdentity::from("d67-race-second"))
            .await
            .expect("read")
            .is_none()
    );
    handle.shutdown().await.expect("shutdown");

    // Truly concurrent callers: whichever wins, the two answers agree.
    for round in 0..16 {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        let name = format!("d67-race-{round}");
        let (spawned, closed) = tokio::join!(
            handle.spawn_spec(worker_spec(&name)),
            handle.close_if_idle()
        );
        let closed = closed.expect("close answers");
        let present = handle
            .get_member(&AgentIdentity::from(name.as_str()))
            .await
            .expect("read")
            .is_some();
        match spawned {
            Ok(_) => {
                assert!(
                    present,
                    "round {round}: an admitted spawn seated its member"
                );
                assert!(
                    matches!(
                        closed,
                        CloseIfIdleOutcome::Busy(_) | CloseIfIdleOutcome::Closed
                    ),
                    "round {round}: {closed:?}"
                );
            }
            Err(error) => {
                assert_admission_closed(error, MobAdmissionCloseReason::IdleClose);
                assert_eq!(closed, CloseIfIdleOutcome::Closed, "round {round}");
                assert!(!present, "round {round}: a refused spawn seated nothing");
            }
        }
        handle.shutdown().await.expect("shutdown");
    }
}

#[tokio::test]
async fn internal_clones_do_not_keep_an_idle_mob_open() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let observer = handle.observer();
    let read = handle.read_handle();
    assert!(handle.is_public_owner());
    assert!(!observer.is_public_owner());

    // The actor's flow executor, event pumps and recovery drivers still hold
    // their clones; only the public owner goes away.
    drop(handle);
    assert_eq!(
        await_closed(&observer).await,
        MobAdmissionCloseReason::LastOwnerReleased
    );
    assert_admission_closed(
        observer
            .spawn_spec(worker_spec("d67-released-late"))
            .await
            .expect_err("a spawn after the release is refused"),
        MobAdmissionCloseReason::LastOwnerReleased,
    );
    drop(read);
    observer
        .stop()
        .await
        .expect("the observer can still tear the mob down");
}

#[tokio::test]
async fn a_retained_public_handle_keeps_the_mob_open() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let retained = handle.clone();
    let observer = handle.observer();
    drop(handle);
    actor_round_trip(&observer).await;
    actor_round_trip(&observer).await;
    assert!(
        observer.admission_closed().borrow().is_none(),
        "a retained public clone keeps admission open"
    );
    retained
        .spawn_spec(worker_spec("d67-retained"))
        .await
        .expect("the retained owner still admits work");

    drop(retained);
    assert_eq!(
        await_closed(&observer).await,
        MobAdmissionCloseReason::LastOwnerReleased
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_released_busy_mob_closes_once_its_admitted_work_drains() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let observer = handle.observer();
    let (release, mut entered) = hold_spawn_entered("d67-drain");
    let spawn = enqueue_spawn(&handle, "d67-drain").await;
    // The spawn is admitted and in flight before the last owner leaves.
    spawn_entered(&mut entered).await;
    drop(handle);
    // Non-owning observers keep status reads queued for the rest of the
    // test: later traffic must not postpone the release decision forever.
    let reading = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let readers = (0..4)
        .map(|_| {
            let reader = observer.observer();
            let reading = Arc::clone(&reading);
            tokio::spawn(async move {
                while reading.load(std::sync::atomic::Ordering::SeqCst) {
                    let _ = reader.status().await;
                }
            })
        })
        .collect::<Vec<_>>();
    actor_round_trip(&observer).await;
    actor_round_trip(&observer).await;
    assert!(
        observer.admission_closed().borrow().is_none(),
        "admitted work in flight keeps the released mob open"
    );

    release.add_permits(1);
    reply(spawn).await.expect("the admitted spawn completes");
    assert_eq!(
        await_closed(&observer).await,
        MobAdmissionCloseReason::LastOwnerReleased
    );
    reading.store(false, std::sync::atomic::Ordering::SeqCst);
    for reader in readers {
        reader.await.expect("status reader");
    }
    assert!(
        observer
            .get_member(&AgentIdentity::from("d67-drain"))
            .await
            .expect("read")
            .is_some(),
        "closing never undoes admitted work"
    );
}

/// The release counts the commands already queued when it is observed. A
/// batch of spawn completions behind it is taken off the mailbox together
/// (the first by the ordinary receive, the rest inline), and each counts
/// once: the release then closes the idle mob with no later traffic.
#[tokio::test]
async fn a_release_counted_behind_a_spawn_completion_batch_closes_without_later_traffic() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let observer = handle.observer();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let parked = handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ParkActorForObservationTest {
            entered_tx,
            release_rx,
            reply_tx,
        })
        .await
        .expect("park the actor");
    tokio::time::timeout(STEP, entered_rx)
        .await
        .expect("the actor parks")
        .expect("park signal");
    // A batch of completions for tickets no slot holds: the batch handler
    // only reports them, and admits nothing.
    for ticket in [u64::MAX, u64::MAX - 1, u64::MAX - 2] {
        handle
            .command_tx
            .send(super::super::scope_gate::RoutedMobCommand::internal(
                MobCommand::SpawnProvisioned {
                    spawn_ticket: ticket,
                    result: Err(MobError::Internal("test completion".to_string())),
                },
            ))
            .await
            .expect("queue a spawn completion");
    }
    drop(handle);
    let _ = release_tx.send(());
    reply(parked).await.expect("the parked command answers");
    assert_eq!(
        await_closed(&observer).await,
        MobAdmissionCloseReason::LastOwnerReleased,
        "the counted batch is paid off by its own receives"
    );
}

#[tokio::test]
async fn close_admission_closes_a_busy_mob_without_stopping_its_work() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let release = hold_spawn("d67-stand-down");
    let spawn = enqueue_spawn(&handle, "d67-stand-down").await;
    let busy = handle
        .close_admission(MobAdmissionCloseReason::StandDown)
        .await
        .expect("close admission")
        .expect("the in-flight spawn is reported");
    assert!(busy.roster_transitions > 0, "{busy:?}");
    assert_eq!(
        *handle.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::StandDown)
    );
    assert_admission_closed(
        handle
            .spawn_spec(worker_spec("d67-stand-down-late"))
            .await
            .expect_err("new work is refused"),
        MobAdmissionCloseReason::StandDown,
    );

    release.add_permits(1);
    reply(spawn)
        .await
        .expect("work admitted before the close completes");
    assert_eq!(
        handle.close_if_idle().await.expect("close if idle"),
        CloseIfIdleOutcome::AlreadyClosed(MobAdmissionCloseReason::StandDown)
    );
}

/// After `shutdown()` the actor takes no more commands, so the mob grants no
/// owner lease either: the admitted Shutdown closes admission with its own
/// reason, which every handle observes.
#[tokio::test]
async fn shutdown_closes_admission_and_refuses_later_owners() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let observer = handle.observer();
    assert!(observer.admission_closed().borrow().is_none());

    handle.shutdown().await.expect("shutdown");
    assert_eq!(
        *observer.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::Shutdown)
    );
    assert_eq!(
        *handle.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::Shutdown)
    );
    for acquirer in [&observer, &handle] {
        match acquirer.acquire_owner() {
            Ok(_) => panic!("a shut-down mob grants no owner"),
            Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::Shutdown),
        }
    }
}

/// A Shutdown after an earlier close keeps the first reason (D67).
#[tokio::test]
async fn shutdown_keeps_an_earlier_close_reason() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    assert_eq!(
        handle
            .close_admission(MobAdmissionCloseReason::StandDown)
            .await
            .expect("close admission"),
        None
    );

    handle.shutdown().await.expect("shutdown");
    assert_eq!(
        *handle.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::StandDown)
    );
    match handle.acquire_owner() {
        Ok(_) => panic!("a closed mob grants no owner"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::StandDown),
    }
}

/// Control: a Stop, the resumable pause, leaves admission open.
#[tokio::test]
async fn stop_leaves_admission_open() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle.stop().await.expect("stop");
    actor_round_trip(&handle).await;
    assert!(
        handle.admission_closed().borrow().is_none(),
        "a stopped mob keeps admission open"
    );
    let lease = match handle.acquire_owner() {
        Ok(lease) => lease,
        Err(error) => panic!("a stopped mob grants a foreground owner: {error}"),
    };
    assert!(lease.is_public_owner());
}

/// The admitted Shutdown closes admission before its teardown: while the
/// teardown still waits on a held session, the mob grants no owner lease and
/// refuses new work with the Shutdown reason.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
#[tokio::test]
async fn shutdown_closes_admission_before_its_teardown() {
    let mut definition = sample_definition();
    definition
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .expect("lead profile")
        .as_inline_mut()
        .expect("inline lead profile")
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let (handle, service) = create_test_mob_with_runtime_backed_real_comms(definition).await;
    let held_session = handle
        .spawn(
            ProfileName::from("lead"),
            AgentIdentity::from("d67-shutdown-held"),
            None,
        )
        .await
        .expect("spawn member")
        .bridge_session_id()
        .expect("session-backed member")
        .clone();
    let guard = service
        .runtime_adapter
        .hold_session_registration_transaction_for_test(&held_session)
        .await;
    let mut parked = handle
        .lifecycle_observations
        .shutdown_teardown_parked
        .subscribe();
    assert!(handle.admission_closed().borrow().is_none());

    let shutdown_handle = handle.clone();
    let shutdown = tokio::spawn(async move {
        shutdown_handle
            .shutdown_with_report(
                crate::runtime::ShutdownOptions::default()
                    .with_deadline(Instant::now() + Duration::from_secs(3)),
            )
            .await
    });
    tokio::time::timeout(STEP, parked.wait_for(|parked| *parked))
        .await
        .expect("the shutdown parks on its teardown")
        .expect("the probe outlives the test");
    assert_eq!(
        *handle.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::Shutdown),
        "admission closed before the teardown"
    );
    match handle.acquire_owner() {
        Ok(_) => panic!("a shutting-down mob grants no owner"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::Shutdown),
    }
    let late = tokio::time::timeout(
        STEP,
        handle.spawn(
            ProfileName::from("lead"),
            AgentIdentity::from("d67-shutdown-late"),
            None,
        ),
    )
    .await
    .expect("the actor answers a spawn during the teardown");
    match late {
        Ok(_) => panic!("a shutting-down mob admits no new member"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::Shutdown),
    }
    assert!(
        !shutdown.is_finished(),
        "the teardown is still waiting on the held session"
    );

    drop(guard);
    tokio::time::timeout(STEP, shutdown)
        .await
        .expect("shutdown returns")
        .expect("join")
        .expect("shutdown succeeds");
}

/// After `destroy()` the actor has exited, so the mob grants no owner lease
/// either: the admitted destroy closed admission with its own reason.
#[tokio::test]
async fn destroy_closes_admission_and_refuses_later_owners() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let observer = handle.observer();
    assert!(observer.admission_closed().borrow().is_none());

    handle.destroy().await.expect("destroy");
    assert_eq!(
        *observer.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::Destroyed)
    );
    match observer.acquire_owner() {
        Ok(_) => panic!("a destroyed mob grants no owner"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::Destroyed),
    }
}

/// The destroy closes admission when it is admitted (its durable marker),
/// not when it finishes: a destroy left incomplete after its admission keeps
/// admission closed.
#[tokio::test]
async fn an_incomplete_admitted_destroy_keeps_admission_closed() {
    let definition = with_unique_mob_id(sample_definition(), "d67-destroy-incomplete");
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let handle = MobBuilder::new(
        definition,
        MobStorage::with_events_and_runtime_metadata(
            events.clone(),
            Arc::new(InMemoryMobRuntimeMetadataStore::new()),
        ),
    )
    .with_session_service(service.clone())
    .create()
    .await
    .expect("create mob");
    handle
        .spawn_spec(worker_spec("d67-destroy-incomplete-worker"))
        .await
        .expect("spawn worker");
    events.fail_appends_for("MemberRetired").await;

    match handle.destroy().await {
        Err(crate::runtime::handle::MobDestroyError::Incomplete { .. }) => {}
        other => panic!("expected an incomplete destroy, got {other:?}"),
    }
    assert_eq!(
        *handle.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::Destroyed)
    );
    match handle.acquire_owner() {
        Ok(_) => panic!("a mob with an admitted destroy grants no owner"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::Destroyed),
    }
}

/// A mob restored with its durable destroy marker (a destroy admitted, then
/// left incomplete, before the process stopped) starts with admission
/// closed, before its handle escapes: nothing is spawned into it, and no
/// owner lease is granted, while the destroy's retry has not finished, and
/// that retry is still admitted. A current-thread runtime, so the restored
/// actor task has not run when the handle is first read.
#[tokio::test]
async fn a_mob_restored_with_an_admitted_destroy_starts_closed() {
    let definition = with_unique_mob_id(sample_definition(), "d67-destroy-restart");
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let runtime_metadata = Arc::new(InMemoryMobRuntimeMetadataStore::new());
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let handle = MobBuilder::new(
        definition,
        MobStorage::with_events_and_runtime_metadata(events.clone(), runtime_metadata.clone()),
    )
    .with_session_service(service.clone())
    .create()
    .await
    .expect("create mob");
    handle
        .spawn_spec(worker_spec("d67-destroy-restart-worker"))
        .await
        .expect("spawn worker");
    events.fail_appends_for("MemberRetired").await;
    match handle.destroy().await {
        Err(crate::runtime::handle::MobDestroyError::Incomplete { .. }) => {}
        other => panic!("expected an incomplete destroy, got {other:?}"),
    }
    crash_stop_and_release_routes(handle).await;

    // The retirement append still fails, so the restored actor's automatic
    // destroy retry stays incomplete and the actor stays up while the test
    // reads it.
    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events.clone(),
        runtime_metadata,
    ))
    .with_session_service(service)
    .notify_orchestrator_on_resume(false)
    .resume()
    .await
    .expect("resume the destroying mob from its durable marker");
    // Read before this test first yields: only what the builder did before
    // it returned the handle.
    let closed_at_return = *resumed.admission_closed().borrow();
    let owner_at_return = resumed.acquire_owner().map(drop);
    let late_spawn = resumed
        .spawn_spec(worker_spec("d67-destroy-restart-late"))
        .await;
    match late_spawn {
        Ok(_) => panic!("a restored destroying mob admits no new member"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::Destroyed),
    }
    assert_eq!(
        closed_at_return,
        Some(MobAdmissionCloseReason::Destroyed),
        "admission is closed before the restored handle escapes"
    );
    match owner_at_return {
        Ok(()) => panic!("a restored destroying mob grants no owner, even before its actor runs"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::Destroyed),
    }
    assert_eq!(
        *resumed.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::Destroyed)
    );

    // The destroy's own retry is still admitted: once the retirement append
    // succeeds, it finishes the destroy and the actor exits.
    let mut task = resumed.actor_task_watch();
    events.allow_appends_for("MemberRetired").await;
    assert_eq!(
        task.wait().await,
        crate::runtime::MobActorTaskWait::Returned
    );
}

/// A durability fail-stop exits the actor, so the mob grants no owner lease
/// either: admission closes with the fail-stop's own reason before the actor
/// quiesces and closes its mailbox.
#[tokio::test]
async fn a_durability_fail_stop_closes_admission() {
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let definition = with_unique_mob_id(sample_definition(), "d67-durability-fail-stop");
    let runtime_metadata = Arc::new(FaultInjectedRuntimeMetadataStore::new());
    let handle = MobBuilder::new(
        definition,
        MobStorage::with_events_and_runtime_metadata(
            Arc::new(InMemoryMobEventStore::new()),
            runtime_metadata.clone(),
        ),
    )
    .with_session_service(service.clone())
    .create()
    .await
    .expect("create mob");
    let observer = handle.observer();
    assert!(observer.admission_closed().borrow().is_none());

    runtime_metadata.lose_nth_compare_supervisor_completion_with_unreadable_reread(2);
    let error = handle
        .rotate_supervisor()
        .await
        .expect_err("an unreadable reconciliation fail-stops the actor");
    assert!(
        error
            .to_string()
            .contains("fail-stopping for cold recovery"),
        "unexpected reconciliation error: {error}"
    );
    wait_for_actor_fail_stop(&handle).await;
    assert_eq!(
        *observer.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::DurabilityFailStop)
    );
    match observer.acquire_owner() {
        Ok(_) => panic!("a fail-stopped mob grants no owner"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::DurabilityFailStop),
    }
    drop(observer);
    crash_stop_and_release_routes(handle).await;
}

/// A lost actor task (here, a panic in its loop) takes no further command,
/// so the mob grants no owner lease either: the task's spawn wrapper closes
/// admission with its own reason before it reports the task lost.
#[tokio::test]
async fn a_lost_actor_task_closes_admission() {
    let definition = with_unique_mob_id(sample_definition(), "d67-actor-lost");
    let (handle, _service) = create_test_mob(definition).await;
    let observer = handle.observer();
    let mut task = handle.actor_task_watch();
    assert!(observer.admission_closed().borrow().is_none());

    crate::runtime::actor::panic_actor_loop_for_test(handle.mob_id());
    // Any wake takes the loop to its next turn, where the seam panics.
    let _phase = handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::QueryPhase { reply_tx })
        .await;
    assert_eq!(task.wait().await, crate::runtime::MobActorTaskWait::Lost);
    assert_eq!(
        *observer.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::ActorLost)
    );
    match observer.acquire_owner() {
        Ok(_) => panic!("a lost actor grants no owner"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::ActorLost),
    }
}

/// A structural composition dispatch failure at a loop boundary stops the
/// actor through its ordinary return, not as a lost task: admission closes
/// with its own reason first, so the stopped mob grants no owner lease.
#[tokio::test]
async fn a_dispatch_failure_closes_admission() {
    let definition = with_unique_mob_id(sample_definition(), "d67-dispatch-failed");
    let (handle, _service) = create_test_mob(definition).await;
    let observer = handle.observer();
    let mut task = handle.actor_task_watch();
    assert!(observer.admission_closed().borrow().is_none());

    crate::runtime::actor::fail_next_boundary_flush_for_test(handle.mob_id());
    // Any command takes the loop to its next boundary, where the flush fails.
    let _phase = handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::QueryPhase { reply_tx })
        .await;
    assert_eq!(
        task.wait().await,
        crate::runtime::MobActorTaskWait::Returned
    );
    assert_eq!(
        *observer.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::DispatchFailed)
    );
    match observer.acquire_owner() {
        Ok(_) => panic!("a mob stopped by a dispatch failure grants no owner"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::DispatchFailed),
    }
}

/// A durable recovery that fail-stops before command admission exits the
/// actor with admission closed for the fail-stop, so the mob grants no owner
/// lease on the exited actor.
#[tokio::test]
async fn a_startup_fail_stop_closes_admission() {
    let definition = with_unique_mob_id(sample_definition(), "d67-startup-fail-stop");
    crate::runtime::actor::fail_stop_next_actor_start_for_test(&definition.id);
    let (handle, _service) = create_test_mob(definition).await;
    let mut task = handle.actor_task_watch();

    assert_eq!(
        task.wait().await,
        crate::runtime::MobActorTaskWait::Returned
    );
    assert_eq!(
        *handle.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::DurabilityFailStop)
    );
    match handle.acquire_owner() {
        Ok(_) => panic!("a mob fail-stopped at its start grants no owner"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::DurabilityFailStop),
    }
}

/// While the admitted Shutdown still joins actor work inline (here a held
/// member-live task), a command the Closing state refuses is answered with
/// the close reason, the answer it gets once the Shutdown parks, not as a
/// closed channel: the actor is alive, and its Shutdown can still fail.
#[tokio::test]
async fn a_spawn_during_the_shutdown_drain_is_refused_with_the_close_reason() {
    let definition = with_unique_mob_id(sample_definition(), "d67-shutdown-drain");
    let (handle, _service) = create_test_mob(definition).await;
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let spawned = handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::SpawnHeldLiveMutationForTest {
            release_rx,
            reply_tx,
        })
        .await
        .expect("enqueue the held task");
    tokio::time::timeout(STEP, spawned)
        .await
        .expect("the held task is spawned")
        .expect("spawn reply")
        .expect("spawn the held task");
    let mut admitted = handle.lifecycle_observations.shutdown_admitted.subscribe();

    let shutdown_handle = handle.clone();
    let shutdown = tokio::spawn(async move { shutdown_handle.shutdown().await });
    tokio::time::timeout(STEP, admitted.wait_for(|admitted| *admitted))
        .await
        .expect("the Shutdown is admitted")
        .expect("the observation outlives the test");
    let late = tokio::time::timeout(
        STEP,
        handle.spawn_spec(worker_spec("d67-shutdown-drain-late")),
    )
    .await
    .expect("the actor answers a spawn while it drains");
    match late {
        Ok(_) => panic!("a shutting-down mob admits no new member"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::Shutdown),
    }
    assert!(
        !shutdown.is_finished(),
        "the Shutdown still joins the held task"
    );

    drop(release_tx);
    tokio::time::timeout(STEP, shutdown)
        .await
        .expect("the Shutdown returns")
        .expect("join")
        .expect("the Shutdown finishes");
}

/// A Shutdown that fails after its admission keeps the actor as the retry
/// owner with admission closed: the Closing set stays refused, and a retried
/// Shutdown is admitted and finishes.
#[tokio::test]
async fn a_failed_shutdown_keeps_admission_closed_and_its_retry_finishes() {
    let definition = with_unique_mob_id(sample_definition(), "d67-failed-shutdown");
    let (handle, _service) = create_test_mob(definition).await;

    crate::runtime::actor::fail_next_shutdown_drain_for_test(handle.mob_id());
    let error = tokio::time::timeout(STEP, handle.shutdown())
        .await
        .expect("the failed Shutdown answers")
        .expect_err("the Shutdown fails in its drain");
    assert!(
        error
            .to_string()
            .contains("test seam: the Shutdown's lifecycle drain fails"),
        "unexpected Shutdown failure: {error}"
    );
    assert_eq!(
        *handle.admission_closed().borrow(),
        Some(MobAdmissionCloseReason::Shutdown)
    );
    let late = tokio::time::timeout(
        STEP,
        handle.spawn_spec(worker_spec("d67-failed-shutdown-late")),
    )
    .await
    .expect("the live actor answers a spawn");
    match late {
        Ok(_) => panic!("a mob whose Shutdown failed admits no new member"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::Shutdown),
    }
    match handle.acquire_owner() {
        Ok(_) => panic!("a mob whose Shutdown failed grants no owner"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::Shutdown),
    }

    tokio::time::timeout(STEP, handle.shutdown())
        .await
        .expect("the retried Shutdown answers")
        .expect("the retried Shutdown is admitted and finishes");
}

/// After a failed Shutdown, Stop and Destroy stay available too.
#[tokio::test]
async fn stop_and_destroy_stay_available_after_a_failed_shutdown() {
    let definition = with_unique_mob_id(sample_definition(), "d67-failed-shutdown-teardown");
    let (handle, _service) = create_test_mob(definition).await;

    crate::runtime::actor::fail_next_shutdown_drain_for_test(handle.mob_id());
    tokio::time::timeout(STEP, handle.shutdown())
        .await
        .expect("the failed Shutdown answers")
        .expect_err("the Shutdown fails in its drain");
    tokio::time::timeout(STEP, handle.stop())
        .await
        .expect("the Stop answers")
        .expect("the Stop is admitted and finishes");
    tokio::time::timeout(STEP, handle.destroy())
        .await
        .expect("the destroy answers")
        .expect("the destroy is admitted and finishes");
}

/// Hold the next Shutdown's drain on a member-live task it joins, until the
/// returned sender is dropped.
async fn hold_shutdown_drain(handle: &MobHandle) -> tokio::sync::oneshot::Sender<()> {
    tokio::time::timeout(STEP, handle.hold_shutdown_drain_for_test())
        .await
        .expect("the held task is spawned")
        .expect("spawn the held task")
}

/// Start a Shutdown and return once it is admitted, so it is draining.
async fn start_shutdown(handle: &MobHandle) -> tokio::task::JoinHandle<Result<(), MobError>> {
    let mut admitted = handle.lifecycle_observations.shutdown_admitted.subscribe();
    let shutdown_handle = handle.clone();
    let shutdown = tokio::spawn(async move { shutdown_handle.shutdown().await });
    tokio::time::timeout(STEP, admitted.wait_for(|admitted| *admitted))
        .await
        .expect("the Shutdown is admitted")
        .expect("the observation outlives the test");
    shutdown
}

fn assert_shutdown_in_progress<T>(result: Result<T, MobError>, expected: &MobId) {
    match result {
        Ok(_) => panic!("a draining Shutdown runs no other request"),
        Err(MobError::ShutdownInProgress { mob_id }) => assert_eq!(&mob_id, expected),
        Err(other) => panic!("expected ShutdownInProgress, got {other}"),
    }
}

/// While a Shutdown drains its own work, `stop()` is refused at once with
/// `ShutdownInProgress`: never a closed channel (the actor is alive) and
/// never a wait for a lifecycle transition. Here the Shutdown then fails, so
/// the retried Stop runs on the live actor.
#[tokio::test]
async fn a_stop_during_a_failing_shutdown_drain_is_refused_at_once_and_runs_on_retry() {
    let definition = with_unique_mob_id(sample_definition(), "d67-stop-failing-drain");
    let (handle, _service) = create_test_mob(definition).await;
    let release_tx = hold_shutdown_drain(&handle).await;
    crate::runtime::actor::fail_next_shutdown_drain_for_test(handle.mob_id());
    let shutdown = start_shutdown(&handle).await;

    let asked = tokio::time::Instant::now();
    let refused = tokio::time::timeout(STEP, handle.stop())
        .await
        .expect("the Stop is answered during the drain");
    let waited = asked.elapsed();
    assert_shutdown_in_progress(refused, handle.mob_id());
    assert!(
        waited < Duration::from_secs(1),
        "stop() returned the refusal after {waited:?}"
    );
    assert!(
        !shutdown.is_finished(),
        "the Shutdown still joins the held task"
    );

    drop(release_tx);
    tokio::time::timeout(STEP, shutdown)
        .await
        .expect("the Shutdown returns")
        .expect("join")
        .expect_err("the Shutdown fails in its drain");
    tokio::time::timeout(STEP, handle.stop())
        .await
        .expect("the retried Stop answers")
        .expect("the retried Stop runs on the live actor");
    assert_eq!(
        handle
            .status()
            .await
            .expect("the live actor reports its phase"),
        MobState::Stopped
    );
}

/// The same Stop during a Shutdown drain that then commits is refused the
/// same way, and once the actor has exited a request finds its command
/// channel closed.
#[tokio::test]
async fn a_stop_during_a_committing_shutdown_drain_is_refused_then_finds_the_actor_gone() {
    let definition = with_unique_mob_id(sample_definition(), "d67-stop-committing-drain");
    let (handle, _service) = create_test_mob(definition).await;
    let release_tx = hold_shutdown_drain(&handle).await;
    let mut task = handle.actor_task_watch();
    let shutdown = start_shutdown(&handle).await;

    let refused = tokio::time::timeout(STEP, handle.stop())
        .await
        .expect("the Stop is answered during the drain");
    assert_shutdown_in_progress(refused, handle.mob_id());

    drop(release_tx);
    tokio::time::timeout(STEP, shutdown)
        .await
        .expect("the Shutdown returns")
        .expect("join")
        .expect("the Shutdown finishes");
    assert_eq!(
        task.wait().await,
        crate::runtime::MobActorTaskWait::Returned
    );
    match tokio::time::timeout(STEP, handle.stop())
        .await
        .expect("a Stop after the exit answers")
    {
        Ok(_) => panic!("an exited actor runs no Stop"),
        Err(MobError::ActorCommandChannelClosed) => {}
        Err(other) => panic!("expected a closed command channel after the exit, got {other}"),
    }
}

/// A recovered placed Reset intent returns once the mob's admission is
/// closed. The live actor refuses a Reset for the rest of its life, so no
/// retry could run it; the durable intent stays recorded for the next start.
#[cfg(feature = "runtime-adapter")]
#[tokio::test]
async fn a_recovered_placed_reset_intent_returns_once_admission_is_closed() {
    let definition = with_unique_mob_id(sample_definition(), "d67-recovered-reset-closed");
    let (handle, _service) = create_test_mob(definition).await;
    tokio::time::timeout(
        STEP,
        handle.close_admission(MobAdmissionCloseReason::StandDown),
    )
    .await
    .expect("the close answers")
    .expect("the idle mob closes its admission");
    tokio::time::timeout(
        STEP,
        crate::runtime::builder::drive_recovered_placed_lifecycle_intent(
            handle.clone(),
            crate::machines::mob_machine::PlacedCompletionLifecycleIntentKind::Reset,
            None,
        ),
    )
    .await
    .expect("the recovered Reset intent returns once admission is closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acquire_owner_racing_the_close_is_owned_or_refused_never_both() {
    for round in 0..16 {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        let observer = handle.observer();
        let acquirer = observer.clone();
        let acquire = tokio::spawn(async move { acquirer.acquire_owner() });
        let closer = observer.clone();
        let close = tokio::spawn(async move { closer.close_if_idle().await });
        let acquired = acquire.await.expect("acquire task");
        let closed = close.await.expect("close task").expect("close answers");
        match acquired {
            Ok(lease) => {
                assert!(lease.is_public_owner());
                match closed {
                    CloseIfIdleOutcome::Busy(busy) => {
                        assert!(busy.foreground_owners >= 1, "round {round}: {busy:?}");
                    }
                    other => panic!("round {round}: an acquired owner was closed: {other:?}"),
                }
                assert!(observer.admission_closed().borrow().is_none());
            }
            Err(error) => {
                assert_admission_closed(error, MobAdmissionCloseReason::IdleClose);
                assert_eq!(closed, CloseIfIdleOutcome::Closed, "round {round}");
            }
        }
        handle.shutdown().await.expect("shutdown");
    }
}

#[tokio::test]
async fn an_owner_acquired_after_the_release_keeps_a_draining_mob_open() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let observer = handle.observer();
    let (release, mut entered) = hold_spawn_entered("d67-lease-drain");
    let spawn = enqueue_spawn(&handle, "d67-lease-drain").await;
    // The last owner leaves while the spawn is in flight: the release is
    // latched behind the busy mob.
    spawn_entered(&mut entered).await;
    drop(handle);
    actor_round_trip(&observer).await;
    let lease = observer
        .acquire_owner()
        .expect("an open mob grants a foreground owner");

    release.add_permits(1);
    reply(spawn).await.expect("the admitted spawn completes");
    actor_round_trip(&observer).await;
    actor_round_trip(&observer).await;
    assert!(
        observer.admission_closed().borrow().is_none(),
        "the earlier release does not close a mob a new owner holds"
    );

    drop(lease);
    assert_eq!(
        await_closed(&observer).await,
        MobAdmissionCloseReason::LastOwnerReleased
    );
    match observer.acquire_owner() {
        Ok(_) => panic!("a closed mob grants no owner"),
        Err(error) => assert_admission_closed(error, MobAdmissionCloseReason::LastOwnerReleased),
    }
}

#[tokio::test]
async fn an_owner_call_in_flight_keeps_the_mob_open_until_it_is_admitted() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let observer = handle.observer();
    let release = hold_spawn("d67-in-flight");
    let caller = handle.clone();
    let call = tokio::spawn(async move {
        caller
            .spawn_spec(worker_spec("d67-in-flight"))
            .await
            .map(|_| ())
    });
    drop(handle);
    actor_round_trip(&observer).await;
    actor_round_trip(&observer).await;
    assert!(
        observer.admission_closed().borrow().is_none(),
        "the caller's owning handle keeps the mob open while its spawn is in flight"
    );

    release.add_permits(1);
    tokio::time::timeout(STEP, call)
        .await
        .expect("the call completes")
        .expect("call task")
        .expect("the in-flight spawn is admitted, not orphaned");
    assert_eq!(
        await_closed(&observer).await,
        MobAdmissionCloseReason::LastOwnerReleased
    );
    assert!(
        observer
            .get_member(&AgentIdentity::from("d67-in-flight"))
            .await
            .expect("read")
            .is_some()
    );
}

#[tokio::test]
async fn native_accepted_but_held_work_keeps_the_mob_busy() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("d67-held");
    // The lead profile is externally addressable, so it takes direct turns.
    let mut spec = SpawnMemberSpec::new(ProfileName::from("lead"), identity.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    handle
        .spawn_spec(spec)
        .await
        .expect("spawn turn-driven lead");

    service.hold_start_turns();
    let mut held = service.held_start_turn_entries();
    let entered_before = *held.borrow_and_update();
    handle
        .member(&identity)
        .await
        .expect("member handle")
        .send("hold this turn", HandlingMode::Queue)
        .await
        .expect("the turn is admitted");
    tokio::time::timeout(STEP, held.wait_for(|entries| *entries > entered_before))
        .await
        .expect("the turn enters the hold")
        .expect("held-turn signal");

    match handle.close_if_idle().await.expect("close if idle") {
        CloseIfIdleOutcome::Busy(busy) => assert!(busy.runs >= 1, "{busy:?}"),
        other => panic!("a held native turn keeps the mob busy, got {other:?}"),
    }
    assert!(handle.admission_closed().borrow().is_none());

    service.release_held_start_turns();
    let closed = tokio::time::timeout(STEP, async {
        loop {
            match handle.close_if_idle().await.expect("close if idle") {
                CloseIfIdleOutcome::Closed => break,
                CloseIfIdleOutcome::Busy(_) => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the mob closes once the held turn ends");
}

#[test]
fn live_control_is_classified_by_verb() {
    use super::super::bridge_protocol::BridgeLiveControlVerb;
    let control = |verb| {
        let (reply_tx, _reply_rx) = oneshot::channel();
        MobCommand::MemberLiveControl {
            agent_identity: AgentIdentity::from("d67-live"),
            channel_id: "channel".to_string(),
            verb,
            reply_tx,
        }
    };
    assert!(control(BridgeLiveControlVerb::CommitInput).refused_while_admission_closed());
    assert!(!control(BridgeLiveControlVerb::Interrupt).refused_while_admission_closed());
    assert!(!control(BridgeLiveControlVerb::Refresh).refused_while_admission_closed());
}
