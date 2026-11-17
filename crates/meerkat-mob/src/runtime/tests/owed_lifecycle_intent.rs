//! The lifecycle intent a mob still owes, as its handles observe it: a Stop,
//! Complete or Destroy that was durably begun and has not landed. The
//! observation and the wait read the mob machine's published state, and the
//! recovery driver of a resume decides by the same rule.

use super::*;
use crate::machines::mob_machine::{
    MobMachineState, MobPhase, PlacedCompletionLifecycleIntentKind,
};
use crate::runtime::lifecycle_observation::owed_lifecycle_intent;
use futures::FutureExt as _;

/// A mob whose first runtime ended with `intent` durably begun and not
/// landed: its quiesce start is the last event of the log.
async fn storage_with_an_interrupted(
    intent: PlacedCompletionLifecycleIntentKind,
    label: &str,
) -> (MobStorage, MobId) {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let created = MobBuilder::new(
        with_unique_mob_id(sample_definition(), label),
        storage.clone(),
    )
    .with_session_service(service)
    .create()
    .await
    .expect("create mob");
    let mob_id = created.mob_id().clone();
    created.shutdown().await.expect("stop the first runtime");
    storage
        .events
        .append(NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: MobEventKind::PlacedCompletionLifecycleQuiesceStarted { intent },
        })
        .await
        .expect("record an intent the first runtime began and never finished");
    (storage, mob_id)
}

async fn cold_resume(storage: MobStorage) -> MobHandle {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    MobBuilder::for_resume(storage)
        .with_session_service(service)
        .resume()
        .await
        .expect("resume the mob")
}

/// The recovery driver of one mob held before its first attempt, as an
/// intent whose answer does not come.
struct HeldRecoveryDriver {
    mob_id: MobId,
    hold: Arc<tokio::sync::Notify>,
}

impl HeldRecoveryDriver {
    fn of(mob_id: &MobId) -> Self {
        let hold = Arc::new(tokio::sync::Notify::new());
        crate::runtime::builder::RECOVERED_LIFECYCLE_DRIVER_HOLDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(mob_id.as_str().to_string(), Arc::clone(&hold));
        Self {
            mob_id: mob_id.clone(),
            hold,
        }
    }

    fn release(&self) {
        crate::runtime::builder::RECOVERED_LIFECYCLE_DRIVER_HOLDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(self.mob_id.as_str());
        self.hold.notify_one();
    }
}

impl Drop for HeldRecoveryDriver {
    fn drop(&mut self) {
        crate::runtime::builder::RECOVERED_LIFECYCLE_DRIVER_HOLDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(self.mob_id.as_str());
    }
}

type OwedIntentWait = tokio::task::JoinHandle<Result<MobState, MobError>>;

fn wait_until_nothing_is_owed(handle: &MobHandle) -> OwedIntentWait {
    let handle = handle.clone();
    tokio::spawn(async move { handle.wait_until_no_lifecycle_intent_is_owed().await })
}

/// The answer of a wait that must end: a wait still pending after the bound
/// fails the row instead of hanging it.
async fn ended(wait: OwedIntentWait) -> Result<MobState, MobError> {
    tokio::time::timeout(Duration::from_secs(30), wait)
        .await
        .expect("the wait for an owed lifecycle intent must end")
        .expect("the waiting task ran to its end")
}

/// The machine facts the rule reads, as the handle's watch holds them.
fn published_facts(
    handle: &MobHandle,
) -> (MobPhase, Option<PlacedCompletionLifecycleIntentKind>, bool) {
    let state = handle.machine_state_watch_rx.borrow();
    (
        state.lifecycle_phase,
        state.placed_completion_lifecycle_intent,
        state.destroy_admitted,
    )
}

/// A Stop an earlier runtime durably began, whose recovery has not answered
/// when the resume hands the mob back (its driver is held, as a Stop held on
/// a member would hold it): the handle observes phase Running and an owed
/// Stop, and a wait for it stays pending. Once the Stop lands the wait ends
/// with Stopped, read from the state that owes nothing, and a host that
/// lifts the mob after the wait runs it.
#[tokio::test(start_paused = true)]
async fn a_recovered_stop_still_owed_is_observed_until_it_lands() {
    let (storage, mob_id) =
        storage_with_an_interrupted(PlacedCompletionLifecycleIntentKind::Stop, "owed-stop").await;
    let held = HeldRecoveryDriver::of(&mob_id);
    let resumed = cold_resume(storage).await;

    let observation = resumed.lifecycle_observation();
    assert_eq!(
        observation.phase,
        MobState::Running,
        "a Stop that has not answered leaves the mob as the resume found it"
    );
    assert_eq!(
        observation.owed_intent,
        Some(PlacedCompletionLifecycleIntentKind::Stop),
        "the handle says which intent the mob still owes"
    );
    assert_eq!(
        resumed.read_handle().lifecycle_observation().owed_intent,
        Some(PlacedCompletionLifecycleIntentKind::Stop),
        "a read handle observes the same machine state"
    );
    assert!(
        observation.failure.is_none(),
        "the actor is alive: {:?}",
        observation.failure
    );

    let wait = wait_until_nothing_is_owed(&resumed);
    tokio::time::sleep(Duration::from_mins(1)).await;
    assert!(
        !wait.is_finished(),
        "the wait is pending while the Stop is owed"
    );

    held.release();
    assert_eq!(
        ended(wait).await.expect("the Stop landed"),
        MobState::Stopped,
        "the wait returns the phase the landed Stop leaves"
    );
    let observation = resumed.lifecycle_observation();
    assert_eq!(observation.phase, MobState::Stopped);
    assert_eq!(
        observation.owed_intent, None,
        "a landed Stop is no longer owed"
    );

    resumed
        .resume()
        .await
        .expect("the host lifts the stopped mob after the wait");
    assert_eq!(resumed.status().await.expect("status"), MobState::Running);
    assert_eq!(resumed.lifecycle_observation().owed_intent, None);
}

/// A Stop that landed owes nothing, although the machine keeps its Stop
/// intent open in phase Stopped: on the runtime that stopped the mob, and on
/// a resume of a log that ends in `MobStopped`. The wait returns at once.
#[tokio::test]
async fn a_landed_stop_owes_nothing() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let created = MobBuilder::new(
        with_unique_mob_id(sample_definition(), "landed-stop"),
        storage.clone(),
    )
    .with_session_service(service)
    .create()
    .await
    .expect("create mob");
    assert_eq!(
        created.lifecycle_observation().owed_intent,
        None,
        "a running mob that began no lifecycle intent owes none"
    );
    created.stop().await.expect("stop the mob");
    assert_a_landed_stop_owes_nothing("the runtime that stopped the mob", &created);

    created.shutdown().await.expect("stop the first runtime");
    assert!(
        storage
            .events
            .replay_all()
            .await
            .expect("replay the log")
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobStopped)),
        "the log records the landed Stop"
    );
    let resumed = cold_resume(storage).await;
    assert_a_landed_stop_owes_nothing("a resume of the stopped mob", &resumed);
}

fn assert_a_landed_stop_owes_nothing(runtime: &str, handle: &MobHandle) {
    assert_eq!(
        published_facts(handle),
        (
            MobPhase::Stopped,
            Some(PlacedCompletionLifecycleIntentKind::Stop),
            false
        ),
        "{runtime}: the machine keeps the Stop intent open in phase Stopped"
    );
    let observation = handle.lifecycle_observation();
    assert_eq!(observation.phase, MobState::Stopped, "{runtime}");
    assert_eq!(
        observation.owed_intent, None,
        "{runtime}: an open intent whose phase was reached is not owed"
    );
    let at_once = handle
        .wait_until_no_lifecycle_intent_is_owed()
        .now_or_never()
        .unwrap_or_else(|| panic!("{runtime}: a mob that owes nothing is not waited for"));
    assert_eq!(
        at_once.expect("nothing is owed"),
        MobState::Stopped,
        "{runtime}"
    );
}

/// A destroy that was admitted (its `MobDestroying` marker is durable) and
/// whose cleanup has not finished is owed as Destroy, although the public
/// phase already reads Destroyed. The wait stays pending while nothing
/// drives the cleanup and ends once the mob is Destroyed.
#[tokio::test]
async fn an_admitted_destroy_is_owed_as_destroy() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let definition = with_unique_mob_id(sample_definition(), "owed-destroy");
    let (created, service, runs, specs, runtime_metadata, identity, identity_status) =
        create_test_mob_with_recoverable_fault_events(definition, events.clone()).await;
    created.shutdown().await.expect("stop the first runtime");

    // The resume admits a destroy before start. While its caller holds the
    // custody, the automatic destroy retry is idle: the destroy stays
    // admitted and not landed.
    let (resumed, custody) = resume_admitting_destroy_before_start(
        events.clone(),
        runs,
        specs,
        runtime_metadata,
        identity,
        identity_status,
        service,
        crate::control_policy::MobControlPrincipal::Owner,
    )
    .await;
    let resumed = resumed.expect("resume admitting a destroy before start");
    assert!(
        events
            .replay_all()
            .await
            .expect("replay the log")
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobDestroying)),
        "the admitted destroy is durable"
    );
    let (phase, open_intent, destroy_admitted) = published_facts(&resumed);
    assert!(
        destroy_admitted && phase != MobPhase::Destroyed,
        "the destroy is admitted and has not landed: phase {phase:?}"
    );
    assert_eq!(
        open_intent,
        Some(PlacedCompletionLifecycleIntentKind::Destroy),
        "the machine admits a destroy under an open Destroy intent"
    );
    let observation = resumed.lifecycle_observation();
    assert_eq!(
        observation.owed_intent,
        Some(PlacedCompletionLifecycleIntentKind::Destroy),
        "an admitted destroy is the intent the mob owes"
    );
    assert_eq!(
        observation.phase,
        MobState::Destroyed,
        "an admitted destroy closes the public phase before its cleanup lands"
    );

    let wait = wait_until_nothing_is_owed(&resumed);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !wait.is_finished(),
        "the wait is pending while the destroy's cleanup is owed"
    );

    // The caller releases its custody: the destroy retry finishes the
    // cleanup.
    drop(custody);
    assert_eq!(
        ended(wait).await.expect("the destroy landed"),
        MobState::Destroyed
    );
}

/// A Complete an earlier runtime durably began is observed as a Stop is:
/// owed until it lands, then the wait ends with Completed.
#[tokio::test(start_paused = true)]
async fn a_complete_still_owed_is_observed_like_a_stop() {
    let (storage, mob_id) = storage_with_an_interrupted(
        PlacedCompletionLifecycleIntentKind::Complete,
        "owed-complete",
    )
    .await;
    let held = HeldRecoveryDriver::of(&mob_id);
    let resumed = cold_resume(storage).await;

    let observation = resumed.lifecycle_observation();
    assert_eq!(observation.phase, MobState::Running);
    assert_eq!(
        observation.owed_intent,
        Some(PlacedCompletionLifecycleIntentKind::Complete),
        "the handle says the mob still owes its Complete"
    );
    let wait = wait_until_nothing_is_owed(&resumed);
    tokio::time::sleep(Duration::from_mins(1)).await;
    assert!(
        !wait.is_finished(),
        "the wait is pending while the Complete is owed"
    );

    held.release();
    assert_eq!(
        ended(wait).await.expect("the Complete landed"),
        MobState::Completed,
        "the wait returns the phase the landed Complete leaves"
    );
    let observation = resumed.lifecycle_observation();
    assert_eq!(observation.phase, MobState::Completed);
    assert_eq!(
        observation.owed_intent, None,
        "a landed Complete is no longer owed"
    );
}

/// A Stop this runtime began and that is parked on a member's end of turn
/// is observed as a recovered one is: the machine state is the same, so the
/// handle says Running with an owed Stop, the wait is pending, and it ends
/// with Stopped once the member's turn ends and the Stop lands.
#[tokio::test]
async fn a_live_stop_parked_on_a_member_is_owed_like_a_recovered_one() {
    let (handle, service) =
        create_test_mob(with_unique_mob_id(sample_definition(), "owed-live-stop")).await;
    let identity = AgentIdentity::from("slow-wind-down");
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
    service.set_session_active(&session, true).await;
    service.hold_wind_down_after_interrupt(&session).await;
    // The member's kickoff turn is running: the runtime has its run current,
    // so the Stop parks on a turn it interrupted.
    tokio::time::timeout(
        Duration::from_secs(10),
        service.wait_keep_alive_turn_entered(&session),
    )
    .await
    .expect("the kickoff turn starts");

    let stop = tokio::spawn({
        let handle = handle.clone();
        async move { handle.stop().await.map(|_| ()) }
    });
    tokio::time::timeout(
        Duration::from_secs(10),
        crate::runtime::actor::stop_idle_wait_probe::all_waiting(std::slice::from_ref(&session)),
    )
    .await
    .expect("the stop awaits the interrupted member's end of turn");

    assert_eq!(
        published_facts(&handle),
        (
            MobPhase::Running,
            Some(PlacedCompletionLifecycleIntentKind::Stop),
            false
        ),
        "a parked Stop is the machine state a recovered Stop has"
    );
    let observation = handle.lifecycle_observation();
    assert_eq!(observation.phase, MobState::Running);
    assert_eq!(
        observation.owed_intent,
        Some(PlacedCompletionLifecycleIntentKind::Stop),
        "a Stop parked on a member is owed"
    );
    let wait = wait_until_nothing_is_owed(&handle);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !wait.is_finished(),
        "the wait is pending while the parked Stop is owed"
    );

    service.set_session_active(&session, false).await;
    assert_eq!(
        ended(wait).await.expect("the parked Stop landed"),
        MobState::Stopped
    );
    tokio::time::timeout(Duration::from_secs(10), stop)
        .await
        .expect("the stop completes once the turn ends")
        .expect("stop task")
        .expect("stop");
    assert_eq!(handle.lifecycle_observation().owed_intent, None);
}

const WAITERS: usize = 4;

/// The actor ends while a Stop is owed: no waiter hangs, and each gets a
/// typed answer that depends on how the actor ended. An actor whose task
/// ended without a record answers the closed-actor error `status` reports;
/// one that fail-stopped answers the recorded cause `status` reports; one
/// that a graceful Shutdown ended answers Stopped, as `status` does, because
/// the Shutdown leaves that actor's machine in the state a landed Stop
/// leaves (the log still owes the Stop).
#[tokio::test(start_paused = true)]
async fn the_wait_for_an_owed_intent_ends_typed_when_the_actor_ends() {
    // The actor's task ends without a record while waiters are parked.
    let (storage, mob_id) = storage_with_an_interrupted(
        PlacedCompletionLifecycleIntentKind::Stop,
        "owed-stop-lost-actor",
    )
    .await;
    let _held = HeldRecoveryDriver::of(&mob_id);
    let resumed = cold_resume(storage).await;
    assert_eq!(
        resumed.lifecycle_observation().owed_intent,
        Some(PlacedCompletionLifecycleIntentKind::Stop)
    );
    let waits = (0..WAITERS)
        .map(|_| wait_until_nothing_is_owed(&resumed))
        .collect::<Vec<_>>();
    tokio::time::sleep(Duration::from_mins(1)).await;
    assert!(
        waits.iter().all(|wait| !wait.is_finished()),
        "every waiter is pending while the Stop is owed"
    );
    let mut task = resumed.actor_task_watch();
    crate::runtime::actor::panic_actor_loop_for_test(&mob_id);
    // Any wake takes the loop to its next turn, where the seam panics.
    let _phase = resumed
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::QueryPhase { reply_tx })
        .await;
    assert_eq!(task.wait().await, crate::runtime::MobActorTaskWait::Lost);
    let reported = resumed
        .status()
        .await
        .expect_err("a lost actor's live phase is not lifecycle truth");
    for wait in waits {
        let error = ended(wait)
            .await
            .expect_err("a Stop still owed by an actor that ended has not landed");
        assert!(
            matches!(error, MobError::Internal(_)),
            "no cause was recorded, so the waiter gets the closed-actor error: {error:?}"
        );
        assert_eq!(
            error.to_string(),
            reported.to_string(),
            "the waiter's error is the one status() reports"
        );
    }
    let late = ended(wait_until_nothing_is_owed(&resumed))
        .await
        .expect_err("a wait that starts after the actor ended is answered the same");
    assert_eq!(late.to_string(), reported.to_string());
    assert_eq!(
        resumed.lifecycle_observation().owed_intent,
        Some(PlacedCompletionLifecycleIntentKind::Stop),
        "the Stop stays owed: the next resume drives it"
    );

    // The actor fail-stops in its startup recovery, with a recorded cause.
    let (storage, mob_id) = storage_with_an_interrupted(
        PlacedCompletionLifecycleIntentKind::Stop,
        "owed-stop-fail-stopped-actor",
    )
    .await;
    crate::runtime::actor::fail_stop_next_actor_start_for_test(&mob_id);
    let resumed = cold_resume(storage).await;
    let mut task = resumed.actor_task_watch();
    assert_eq!(
        task.wait().await,
        crate::runtime::MobActorTaskWait::Returned
    );
    let reported = match resumed.status().await {
        Err(MobError::SharedLifecycleFailure(cause)) => cause,
        other => panic!("a fail-stopped actor reports why, typed: {other:?}"),
    };
    assert_eq!(
        resumed.lifecycle_observation().owed_intent,
        Some(PlacedCompletionLifecycleIntentKind::Stop),
        "the fail-stopped actor left its recovered Stop owed"
    );
    let waits = (0..WAITERS)
        .map(|_| wait_until_nothing_is_owed(&resumed))
        .collect::<Vec<_>>();
    for wait in waits {
        match ended(wait).await {
            Err(MobError::SharedLifecycleFailure(cause)) => assert!(
                Arc::ptr_eq(&cause, &reported),
                "the waiter's failure is the cause status() reports: {cause:?} vs {reported:?}"
            ),
            other => panic!("a waiter on a fail-stopped actor gets why it stopped: {other:?}"),
        }
    }

    // The runtime is shut down while waiters are parked. A Shutdown leaves
    // this actor's machine Stopped, which `status` reports from then on, so
    // every waiter is answered with that phase; the durable log still owes
    // the Stop, and the next resume drives it.
    let (storage, mob_id) = storage_with_an_interrupted(
        PlacedCompletionLifecycleIntentKind::Stop,
        "owed-stop-shut-down-actor",
    )
    .await;
    let _held = HeldRecoveryDriver::of(&mob_id);
    let resumed = cold_resume(storage.clone()).await;
    let waits = (0..WAITERS)
        .map(|_| wait_until_nothing_is_owed(&resumed))
        .collect::<Vec<_>>();
    tokio::time::sleep(Duration::from_mins(1)).await;
    assert!(
        waits.iter().all(|wait| !wait.is_finished()),
        "every waiter is pending while the Stop is owed"
    );
    resumed.shutdown().await.expect("shut the runtime down");
    let reported = resumed.status().await.expect("a shut down mob's phase");
    assert_eq!(reported, MobState::Stopped);
    for wait in waits {
        assert_eq!(
            ended(wait)
                .await
                .expect("a shut down actor answers its waiters as status() does"),
            reported
        );
    }
    let observation = resumed.lifecycle_observation();
    assert_eq!(
        (observation.phase, observation.owed_intent),
        (MobState::Stopped, None),
        "the ended actor's handle reads the state a landed Stop leaves"
    );
    assert!(
        !storage
            .events
            .replay_all()
            .await
            .expect("replay the log")
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobStopped)),
        "the Shutdown recorded no landed Stop: the log still owes it"
    );
    // The next resume reports the Stop as owed again.
    let _held = HeldRecoveryDriver::of(&mob_id);
    let next = cold_resume(storage).await;
    let observation = next.lifecycle_observation();
    assert_eq!(
        (observation.phase, observation.owed_intent),
        (
            MobState::Running,
            Some(PlacedCompletionLifecycleIntentKind::Stop)
        ),
        "the next resume comes back Running with the Stop owed"
    );
}

/// The name of an intent kind, as a row reports it.
fn kind_name(intent: PlacedCompletionLifecycleIntentKind) -> &'static str {
    match intent {
        PlacedCompletionLifecycleIntentKind::Stop => "Stop",
        PlacedCompletionLifecycleIntentKind::Reset => "Reset",
        PlacedCompletionLifecycleIntentKind::Complete => "Complete",
        PlacedCompletionLifecycleIntentKind::RetireAll => "RetireAll",
        PlacedCompletionLifecycleIntentKind::Destroy => "Destroy",
    }
}

/// A graceful Shutdown over an owed Complete, Reset, retirement of every
/// member or not yet admitted Destroy ends the actor in the terminal phase
/// Stopped with that intent still owed. `status` answers Stopped; the wait
/// cannot say "nothing owed", so every waiter gets the closed-channel error,
/// and the ended actor's handle still reports the intent.
#[tokio::test(start_paused = true)]
async fn a_shutdown_over_another_owed_intent_ends_the_wait_with_the_closed_channel_error() {
    let mut seen = Vec::new();
    let mut expected = Vec::new();
    for intent in [
        PlacedCompletionLifecycleIntentKind::Complete,
        PlacedCompletionLifecycleIntentKind::Reset,
        PlacedCompletionLifecycleIntentKind::RetireAll,
        PlacedCompletionLifecycleIntentKind::Destroy,
    ] {
        let name = kind_name(intent);
        let (storage, mob_id) =
            storage_with_an_interrupted(intent, &format!("shutdown-over-{name}")).await;
        let _held = HeldRecoveryDriver::of(&mob_id);
        let resumed = cold_resume(storage).await;
        let before = resumed.lifecycle_observation();
        let waits = (0..WAITERS)
            .map(|_| wait_until_nothing_is_owed(&resumed))
            .collect::<Vec<_>>();
        tokio::time::sleep(Duration::from_mins(1)).await;
        let pending = waits.iter().all(|wait| !wait.is_finished());
        let shutdown = resumed.shutdown().await;
        let status = resumed.status().await;
        let mut answers = Vec::new();
        for wait in waits {
            answers.push(format!("{:?}", ended(wait).await));
        }
        answers.dedup();
        let after = resumed.lifecycle_observation();
        seen.push(format!(
            "{name}: before ({:?}, owed {:?}), waiters pending {pending}, shutdown {shutdown:?}, \
             status {status:?}, waiters {answers:?}, after ({:?}, owed {:?}, failure {:?})",
            before.phase, before.owed_intent, after.phase, after.owed_intent, after.failure
        ));
        expected.push(format!(
            "{name}: before (Running, owed Some({name})), waiters pending true, shutdown Ok(()), \
             status Ok(Stopped), waiters [\"Err(ActorCommandChannelClosed)\"], after (Stopped, \
             owed Some({name}), failure None)"
        ));
    }
    assert_eq!(seen, expected);
}

/// A Reset, a retirement of every member and a not yet admitted Destroy an
/// earlier runtime durably began are owed at a handle like a Stop, and the
/// resume's recovery drives each as before: once its driver goes on the
/// intent lands and the wait ends with the phase it leaves.
#[tokio::test(start_paused = true)]
async fn a_recovered_reset_retire_all_or_destroy_is_owed_until_its_driver_lands_it() {
    let mut seen = Vec::new();
    let mut expected = Vec::new();
    for (intent, lands_as) in [
        (PlacedCompletionLifecycleIntentKind::Reset, "Ok(Running)"),
        (
            PlacedCompletionLifecycleIntentKind::RetireAll,
            "Ok(Running)",
        ),
        (
            PlacedCompletionLifecycleIntentKind::Destroy,
            "Ok(Destroyed)",
        ),
    ] {
        let name = kind_name(intent);
        let (storage, mob_id) =
            storage_with_an_interrupted(intent, &format!("recovered-{name}")).await;
        let held = HeldRecoveryDriver::of(&mob_id);
        let resumed = cold_resume(storage).await;
        let before = resumed.lifecycle_observation();
        let wait = wait_until_nothing_is_owed(&resumed);
        tokio::time::sleep(Duration::from_mins(1)).await;
        let pending = !wait.is_finished();
        held.release();
        let landed = ended(wait).await;
        let after = resumed.lifecycle_observation();
        seen.push(format!(
            "{name}: before ({:?}, owed {:?}), wait pending {pending}, landed {landed:?}, after \
             owed {:?}",
            before.phase, before.owed_intent, after.owed_intent
        ));
        expected.push(format!(
            "{name}: before (Running, owed Some({name})), wait pending true, landed {lands_as}, \
             after owed None"
        ));
    }
    assert_eq!(seen, expected);
}

/// Inside the pre-activation hook of a resume the read handle does not yet
/// report the intent the log leaves open: the resume applies it to the
/// machine after the hook. The same read handle, kept past the hook, and
/// the handle the resume returns report it.
#[tokio::test(start_paused = true)]
async fn the_pre_activation_hook_reads_a_recovered_intent_only_after_the_resume_returns() {
    let (storage, mob_id) = storage_with_an_interrupted(
        PlacedCompletionLifecycleIntentKind::Stop,
        "owed-stop-in-hook",
    )
    .await;
    let _held = HeldRecoveryDriver::of(&mob_id);
    let in_hook = Arc::new(std::sync::Mutex::new(None));
    let record = Arc::clone(&in_hook);
    let hook: MobBeforeActivation = Arc::new(move |read_handle| {
        let record = Arc::clone(&record);
        Box::pin(async move {
            let observation = read_handle.lifecycle_observation();
            *record
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some((observation.phase, observation.owed_intent, read_handle));
            Ok(())
        })
    });
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let resumed = MobBuilder::for_resume(storage)
        .with_session_service(service)
        .before_activation(hook)
        .resume()
        .await
        .expect("resume the mob");

    let (phase, owed_intent, kept_read_handle) = in_hook
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .expect("the hook ran once");
    assert_eq!(
        (phase, owed_intent),
        (MobState::Running, None),
        "inside the hook the recovered Stop is not published yet"
    );
    assert_eq!(
        kept_read_handle.lifecycle_observation().owed_intent,
        Some(PlacedCompletionLifecycleIntentKind::Stop),
        "the read handle kept past the hook reports the owed Stop"
    );
    assert_eq!(
        resumed.lifecycle_observation().owed_intent,
        Some(PlacedCompletionLifecycleIntentKind::Stop),
        "the handle the resume returns reports the owed Stop"
    );
}

/// A Reset a resume recovered on a mob whose admission is then closed
/// cannot run on that actor: the actor refuses a Reset once its admission
/// is closed, and the recovery driver stops trying. The actor stays alive
/// and healthy, the Reset stays owed, and the wait stays pending until the
/// actor ends; it then ends with the closed-channel error.
#[tokio::test(start_paused = true)]
async fn a_recovered_reset_after_admission_closed_keeps_the_wait_pending_until_the_actor_ends() {
    let (storage, mob_id) = storage_with_an_interrupted(
        PlacedCompletionLifecycleIntentKind::Reset,
        "owed-reset-admission-closed",
    )
    .await;
    let held = HeldRecoveryDriver::of(&mob_id);
    let resumed = cold_resume(storage.clone()).await;
    assert_eq!(
        resumed.lifecycle_observation().owed_intent,
        Some(PlacedCompletionLifecycleIntentKind::Reset)
    );
    resumed
        .close_admission(crate::runtime::admission_close::MobAdmissionCloseReason::StandDown)
        .await
        .expect("the idle mob closes its admission");
    let resets_before = mob_resets(&storage).await;
    let wait = wait_until_nothing_is_owed(&resumed);
    // The driver goes on, is refused once, and returns for good.
    held.release();
    tokio::time::sleep(Duration::from_mins(10)).await;
    assert!(
        !wait.is_finished(),
        "nothing drives the Reset on this actor any more: the wait is pending"
    );
    assert_eq!(
        mob_resets(&storage).await,
        resets_before,
        "the refused Reset did not run"
    );
    let observation = resumed.lifecycle_observation();
    assert_eq!(
        (observation.phase, observation.owed_intent),
        (
            MobState::Running,
            Some(PlacedCompletionLifecycleIntentKind::Reset)
        ),
        "the Reset stays owed"
    );
    assert!(
        observation.failure.is_none(),
        "the actor is alive and healthy: {:?}",
        observation.failure
    );
    assert!(
        !resumed.command_tx.is_closed(),
        "the actor still takes commands"
    );

    resumed.shutdown().await.expect("shut the runtime down");
    assert!(
        matches!(ended(wait).await, Err(MobError::ActorCommandChannelClosed)),
        "the wait ends when the actor ends, with the closed-channel error"
    );
}

async fn mob_resets(storage: &MobStorage) -> usize {
    storage
        .events
        .replay_all()
        .await
        .expect("replay the log")
        .iter()
        .filter(|event| matches!(event.kind, MobEventKind::MobReset))
        .count()
}

/// A Stop whose caller got a refusal back (here a member's turn that never
/// ends trips the Stop's hang guard) stays owed with nothing driving it:
/// the wait is pending, also after the member's turn has ended, until a
/// Stop is issued again and lands.
#[tokio::test(start_paused = true)]
async fn a_refused_stop_stays_owed_until_a_stop_is_issued_again() {
    let (handle, service) =
        create_test_mob(with_unique_mob_id(sample_definition(), "owed-refused-stop")).await;
    let identity = AgentIdentity::from("never-winds-down");
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
    service.set_session_active(&session, true).await;
    service.hold_wind_down_after_interrupt(&session).await;
    // The member's kickoff turn is running: the runtime has its run current,
    // so the first Stop ends that run. A Stop that lands while the kickoff
    // input is still queued cancels nothing, and the run that starts once the
    // refused Stop releases its hold is one the Stop issued again waits on.
    // No timeout: the clock is paused.
    service.wait_keep_alive_turn_entered(&session).await;

    let refused = handle
        .stop()
        .await
        .expect_err("a member turn that never ends refuses the Stop at its hang guard");
    assert!(
        matches!(refused, MobError::LifecycleOperationProgressStalled { .. }),
        "the Stop is refused at the hang guard: {refused:?}"
    );
    assert_eq!(
        published_facts(&handle),
        (
            MobPhase::Running,
            Some(PlacedCompletionLifecycleIntentKind::Stop),
            false
        ),
        "the refused Stop leaves its intent open"
    );
    let wait = wait_until_nothing_is_owed(&handle);
    tokio::time::sleep(Duration::from_mins(10)).await;
    assert!(!wait.is_finished(), "the refused Stop is owed");

    service.set_session_active(&session, false).await;
    tokio::time::sleep(Duration::from_mins(10)).await;
    assert!(
        !wait.is_finished(),
        "nothing issues the Stop again by itself: the wait is still pending"
    );

    handle.stop().await.expect("the Stop is issued again");
    assert_eq!(
        ended(wait).await.expect("the Stop landed"),
        MobState::Stopped
    );
}

fn machine_state(
    phase: MobPhase,
    intent: Option<PlacedCompletionLifecycleIntentKind>,
    destroy_admitted: bool,
) -> MobMachineState {
    MobMachineState {
        lifecycle_phase: phase,
        placed_completion_lifecycle_intent: intent,
        placed_completion_lifecycle_quiescing: intent.is_some(),
        destroy_admitted,
        ..MobMachineState::default()
    }
}

/// The source of `source` with comments and string literals blanked, so a
/// guard reads code only.
fn code_only(source: &str) -> String {
    let mut code = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        code.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = ' ';
                for skipped in chars.by_ref() {
                    if previous == '*' && skipped == '/' {
                        break;
                    }
                    if skipped == '\n' {
                        code.push('\n');
                    }
                    previous = skipped;
                }
            }
            '"' => {
                code.push('"');
                while let Some(skipped) = chars.next() {
                    match skipped {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        '\n' => code.push('\n'),
                        _ => {}
                    }
                }
                code.push('"');
            }
            other => code.push(other),
        }
    }
    code
}

/// One rule says which lifecycle intent a machine state owes: the table
/// pins it over every phase, open intent and destroy admission, and the
/// resume's recovery driver decides by a call of it, with no match of its
/// own over the machine's open intent.
#[test]
fn the_recovery_driver_and_the_read_share_one_rule() {
    use PlacedCompletionLifecycleIntentKind::{Complete, Destroy, Reset, RetireAll, Stop};
    const INTENTS: [Option<PlacedCompletionLifecycleIntentKind>; 6] = [
        None,
        Some(Stop),
        Some(Reset),
        Some(Complete),
        Some(RetireAll),
        Some(Destroy),
    ];
    // (phase, destroy admitted, what is owed for each open intent of INTENTS)
    let table: [(
        MobPhase,
        bool,
        [Option<PlacedCompletionLifecycleIntentKind>; 6],
    ); 8] = [
        // An open intent is owed until its phase is reached.
        (
            MobPhase::Running,
            false,
            [
                None,
                Some(Stop),
                Some(Reset),
                Some(Complete),
                Some(RetireAll),
                Some(Destroy),
            ],
        ),
        // A landed Stop keeps its intent open and owes nothing.
        (
            MobPhase::Stopped,
            false,
            [
                None,
                None,
                Some(Reset),
                Some(Complete),
                Some(RetireAll),
                Some(Destroy),
            ],
        ),
        // A landed Complete keeps its intent open and owes nothing.
        (
            MobPhase::Completed,
            false,
            [
                None,
                Some(Stop),
                Some(Reset),
                None,
                Some(RetireAll),
                Some(Destroy),
            ],
        ),
        // A destroyed mob owes nothing.
        (MobPhase::Destroyed, false, [None; 6]),
        (MobPhase::Destroyed, true, [None; 6]),
        // An admitted destroy is owed as Destroy by the admission alone. (The
        // machine admits a destroy only under an open Destroy intent; the
        // rule does not lean on that.)
        (MobPhase::Running, true, [Some(Destroy); 6]),
        (MobPhase::Stopped, true, [Some(Destroy); 6]),
        (MobPhase::Completed, true, [Some(Destroy); 6]),
    ];
    let mut wrong_answers = Vec::new();
    for (phase, destroy_admitted, owed) in table {
        for (intent, expected) in INTENTS.into_iter().zip(owed) {
            let answer = owed_lifecycle_intent(&machine_state(phase, intent, destroy_admitted));
            if answer != expected {
                wrong_answers.push(format!(
                    "phase {phase:?}, open intent {intent:?}, destroy admitted \
                     {destroy_admitted}: owed {answer:?}, expected {expected:?}"
                ));
            }
        }
    }

    // The resume's recovery decides what it drives by that function.
    let builder = include_str!("../builder.rs");
    let decision = "let recovered_lifecycle_retry_intent =";
    assert_eq!(
        builder.matches(decision).count(),
        1,
        "the resume decides once which recovered lifecycle intent it drives"
    );
    let from_decision = &builder[builder.find(decision).expect("the decision exists")..];
    let driver_start = from_decision
        .find("drive_recovered_placed_lifecycle_intent(")
        .expect("the recovery driver is started after the decision");
    let decision_to_driver = code_only(&from_decision[..driver_start]);
    let statement = &decision_to_driver[..decision_to_driver
        .find(';')
        .expect("the decision is a statement")];
    let mut second_rules = Vec::new();
    if !statement.contains("lifecycle_observation::owed_lifecycle_intent(")
        || statement.contains("match")
    {
        second_rules.push(format!(
            "the decision is not a call of the one rule: {}",
            statement.split_whitespace().collect::<Vec<_>>().join(" ")
        ));
    }
    for machine_fact in [
        "placed_completion_lifecycle_intent",
        "lifecycle_phase",
        "destroy_admitted",
    ] {
        if decision_to_driver.contains(machine_fact) {
            second_rules.push(format!(
                "between its decision and its driver the resume reads `{machine_fact}` itself"
            ));
        }
    }

    assert!(
        wrong_answers.is_empty() && second_rules.is_empty(),
        "the rule answers wrongly for {} machine states: {wrong_answers:#?}\nthe recovery driver \
         keeps a rule of its own: {second_rules:#?}",
        wrong_answers.len()
    );
}
