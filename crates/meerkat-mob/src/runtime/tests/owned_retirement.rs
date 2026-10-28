//! A durably started member retirement is owned until it settles.
//!
//! Before this, a retirement whose stage outlived the caller's 30 s budget
//! was dropped after its durable start: the member stayed `Retiring` with no
//! owner, its session was never unregistered, and mob Shutdown never touched
//! it. Now the caller's budget only bounds the caller's wait; the retirement
//! settles on its stages' own signals, a stage failure leaves it `Stuck` in an
//! owned registry with a typed re-drive, and Shutdown interrupts and reports
//! it per member.

use super::*;
use crate::runtime::{
    MemberShutdownOutcome, MemberUnregisterSettlement, MemberUnregisterState, RetirementSettlement,
    ShutdownOptions,
};

/// Bound for test steps that must happen promptly; it only turns a hang into
/// a failure.
const STEP: Duration = Duration::from_secs(10);

async fn turn_driven_mob() -> (MobHandle, Arc<RuntimeBackedRealCommsSessionService>) {
    let mut definition = sample_definition();
    definition
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .expect("lead profile")
        .as_inline_mut()
        .unwrap()
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    create_test_mob_with_runtime_backed_real_comms(definition).await
}

async fn spawn_member(handle: &MobHandle, name: &str) -> (AgentIdentity, SessionId) {
    let identity = AgentIdentity::from(name);
    let session_id = handle
        .spawn(ProfileName::from("lead"), identity.clone(), None)
        .await
        .expect("spawn member")
        .bridge_session_id()
        .expect("session-backed member")
        .clone();
    (identity, session_id)
}

async fn roster_has(handle: &MobHandle, identity: &AgentIdentity) -> bool {
    handle.roster.read().await.get(identity).is_some()
}

async fn settle(handle: &MobHandle, identity: &AgentIdentity) -> RetirementSettlement {
    let mut watch = handle
        .retirement_settlement(identity)
        .expect("the retirement published a settlement");
    tokio::time::timeout(STEP, watch.settled())
        .await
        .expect("the retirement settles on its own signal")
        .expect("the actor settles the retirement before exiting")
}

/// Make `identity`'s retirement stuck after its durable start: the session
/// archive stage fails.
async fn stuck_retirement(
    handle: &MobHandle,
    service: &RuntimeBackedRealCommsSessionService,
    identity: &AgentIdentity,
) {
    service.set_fail_archive(true);
    let error = tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("retire answers")
        .expect_err("a failing archive stage does not retire the member");
    assert!(
        matches!(error, MobError::MemberRetirementStuck { .. }),
        "a stage failure after the durable start is stuck, not dropped: {error:?}"
    );
    match handle
        .retirement_settlement(identity)
        .expect("settlement published")
        .current()
    {
        RetirementSettlement::Stuck { stage, .. } => {
            assert!(
                stage.as_str().contains("archive"),
                "the stuck stage is named: {stage}"
            );
        }
        other => panic!("expected a stuck settlement, got {other:?}"),
    }
    assert!(
        roster_has(handle, identity).await,
        "a stuck member stays in the roster"
    );
}

/// The production shape: a retire whose turn-boundary stage outlives the caller's
/// budget. The caller gets a typed in-progress answer naming the stage; the
/// retirement stays owned and settles `Retired` once the boundary is free,
/// with no second retire.
#[tokio::test]
async fn a_retire_that_outlives_its_caller_budget_stays_owned_and_settles() {
    let (handle, service) = turn_driven_mob().await;
    let (identity, _) = spawn_member(&handle, "ops-sweeper").await;
    let gate = service.install_non_reentrant_turn_finalization_gate();
    let held = gate.lock_owned().await;

    let error = tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("the caller's wait ends at its budget")
        .expect_err("the boundary is held past the caller's budget");
    match &error {
        MobError::MemberRetirementInProgress { stage, .. } => {
            assert_ne!(
                stage, "actor_retirement_saga",
                "the stage is named: {stage}"
            );
        }
        other => panic!("expected an owned in-progress answer, got {other:?}"),
    }
    assert!(matches!(
        handle
            .retirement_settlement(&identity)
            .expect("settlement published")
            .current(),
        RetirementSettlement::InProgress { .. }
    ));

    drop(held);
    assert!(
        matches!(
            settle(&handle, &identity).await,
            RetirementSettlement::Retired
        ),
        "the owned retirement completes without a second retire"
    );
    assert!(!roster_has(&handle, &identity).await);
}

/// A stage failure after the durable start leaves the retirement stuck and
/// owned; a plain retire does not drive it, the typed re-drive does.
#[tokio::test]
async fn a_stuck_retirement_is_reported_and_redriven_explicitly() {
    let (handle, service) = turn_driven_mob().await;
    let (identity, _) = spawn_member(&handle, "stuck-sweeper").await;
    stuck_retirement(&handle, &service, &identity).await;

    service.set_fail_archive(false);
    let plain = tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("retire answers");
    assert!(
        matches!(plain, Err(MobError::MemberRetirementStuck { .. })),
        "a plain retire answers stuck instead of re-driving implicitly: {plain:?}"
    );
    assert!(roster_has(&handle, &identity).await);

    tokio::time::timeout(STEP, handle.redrive_retirement(identity.clone()))
        .await
        .expect("re-drive answers")
        .expect("the re-drive completes the retirement");
    assert!(matches!(
        settle(&handle, &identity).await,
        RetirementSettlement::Retired
    ));
    assert!(!roster_has(&handle, &identity).await);
}

/// A resume re-drives every stuck retirement the actor owns.
#[tokio::test]
async fn resume_redrives_a_stuck_retirement() {
    let (handle, service) = turn_driven_mob().await;
    let (identity, _) = spawn_member(&handle, "resume-sweeper").await;
    stuck_retirement(&handle, &service, &identity).await;
    service.set_fail_archive(false);

    let mut changes = handle.machine_state_changes();
    handle.stop().await.expect("stop");
    handle.resume().await.expect("resume");
    // The resume hands the stuck retirement back as a new owned retirement;
    // the roster removal it ends with is a published machine-state change.
    tokio::time::timeout(STEP, async {
        while roster_has(&handle, &identity).await {
            changes.changed().await.expect("the actor is alive");
        }
    })
    .await
    .expect("the resumed re-drive retires the member");
    assert!(matches!(
        handle
            .retirement_settlement(&identity)
            .expect("settlement published")
            .current(),
        RetirementSettlement::Retired
    ));
    assert!(!roster_has(&handle, &identity).await);
}

/// Shutdown does not wait behind an in-flight retirement whose stage never
/// gets its signal: it interrupts it and reports the member as interrupted at
/// the named stage, and unregisters an idle peer. The retiring member's held
/// turn-finalization boundary also holds its own runtime unregister, so the
/// caller's deadline bounds that wait.
#[tokio::test]
async fn shutdown_interrupts_an_in_flight_retirement_and_reports_it() {
    let (handle, service) = turn_driven_mob().await;
    let (identity, session_id) = spawn_member(&handle, "inflight-sweeper").await;
    let (other, _) = spawn_member(&handle, "idle-peer").await;
    // Only the retiring member's boundary is held; the idle peer's is free.
    let gate = service.install_session_turn_finalization_gate(&session_id);
    let held = gate.lock_owned().await;

    let retire_handle = handle.clone();
    let retire_identity = identity.clone();
    let retire = tokio::spawn(async move { retire_handle.retire(retire_identity).await });
    let error = tokio::time::timeout(STEP, retire)
        .await
        .expect("the caller's wait ends at its budget")
        .expect("join")
        .expect_err("the boundary is held");
    assert!(error.is_retirement_in_progress(), "{error:?}");

    let report = tokio::time::timeout(
        STEP,
        handle.shutdown_with_report(
            ShutdownOptions::default().with_deadline(Instant::now() + Duration::from_secs(2)),
        ),
    )
    .await
    .expect("shutdown completes past the blocked retirement")
    .expect("shutdown succeeds");
    drop(held);
    match report.members.get(&identity) {
        Some(MemberShutdownOutcome::RetirementInterrupted { stage }) => {
            assert!(!stage.as_str().is_empty());
        }
        other => panic!("expected the retirement reported as interrupted, got {other:?}"),
    }
    // An idle peer is never left pending behind another member's held
    // boundary (the production symptom).
    assert!(
        matches!(
            report.members.get(&other),
            Some(MemberShutdownOutcome::Unregistered)
        ),
        "the idle peer is unregistered: {report:?}"
    );
}

/// Shutdown unregisters a stuck (Retiring) member's session, which it used
/// to skip, and reports the stuck retirement.
#[tokio::test]
async fn shutdown_unregisters_a_stuck_members_session_and_reports_it() {
    let (handle, service) = turn_driven_mob().await;
    let (identity, session_id) = spawn_member(&handle, "stuck-at-shutdown").await;
    stuck_retirement(&handle, &service, &identity).await;

    let report = tokio::time::timeout(
        STEP,
        handle.shutdown_with_report(ShutdownOptions::default()),
    )
    .await
    .expect("shutdown completes")
    .expect("shutdown succeeds");
    assert!(
        matches!(
            report.members.get(&identity),
            Some(MemberShutdownOutcome::RetirementStuck { .. })
        ),
        "{report:?}"
    );
    assert!(
        !service.runtime_adapter.contains_session(&session_id).await,
        "the stuck member's runtime session is unregistered"
    );
}

/// A running turn on the retiring member is cancelled through the retire
/// cancel ladder; the retirement settles on that cancellation's convergence,
/// well inside the hang guard.
#[tokio::test]
async fn a_retirement_settles_on_its_cancel_signal_not_the_hang_guard() {
    let (handle, service) = turn_driven_mob().await;
    let (identity, session_id) = spawn_member(&handle, "busy-sweeper").await;
    // The turn holds until the session is interrupted or cancelled at a
    // boundary: exactly the retire cancel ladder's signal.
    service.reset_keep_alive_notifier(&session_id).await;
    let _turn = handle
        .member(&identity)
        .await
        .expect("member handle")
        .start_turn(
            ContentInput::Text("a turn the retire must cancel".into()),
            HandlingMode::Queue,
            crate::MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("turn admitted");
    let started = Instant::now();
    let _ = tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("retire answers");
    assert!(matches!(
        settle(&handle, &identity).await,
        RetirementSettlement::Retired
    ));
    assert!(started.elapsed() < STEP);
}

/// Capture INFO-or-more-severe events emitted on this thread while the guard
/// lives (`#[tokio::test]` runs the mob actor on the test's own thread).
fn capture_info_events() -> (
    Arc<std::sync::Mutex<Vec<BTreeMap<String, String>>>>,
    CaptureGuard,
) {
    use tracing_subscriber::layer::SubscriberExt as _;

    struct FieldVisitor<'a>(&'a mut BTreeMap<String, String>);

    impl tracing::field::Visit for FieldVisitor<'_> {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .insert(field.name().to_string(), format!("{value:?}"));
        }
    }

    struct InfoCapture(Arc<std::sync::Mutex<Vec<BTreeMap<String, String>>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for InfoCapture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if *event.metadata().level() > tracing::Level::INFO {
                return;
            }
            let mut fields = BTreeMap::new();
            event.record(&mut FieldVisitor(&mut fields));
            // Event metadata under keys no field name can take, so a failure
            // shows where each captured event came from.
            fields.insert("@level".into(), event.metadata().level().to_string());
            fields.insert("@target".into(), event.metadata().target().to_string());
            self.0.lock().expect("captured events lock").push(fields);
        }
    }

    // A second, separately registered Dispatch that lives as long as the
    // capture. With only one Dispatch registered, tracing-core 0.1.36 computes
    // a callsite's interest from the current default of the thread that
    // reaches it first (or rebuilds interest), so a concurrent test thread
    // without a subscriber caches `never` for every thread and this capture
    // sees nothing from that callsite. With two, interest comes from every
    // registered Dispatch.
    let anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let guard = tracing::subscriber::set_default(
        tracing_subscriber::registry().with(InfoCapture(Arc::clone(&captured))),
    );
    (
        captured,
        CaptureGuard {
            _default: guard,
            _anchor: anchor,
        },
    )
}

/// Keeps the capture installed, and its anchor Dispatch registered.
struct CaptureGuard {
    _default: tracing::subscriber::DefaultGuard,
    _anchor: tracing::Dispatch,
}

/// The capture sees this thread's events from a callsite that a thread
/// without a subscriber reached first, and after that thread rebuilt every
/// callsite's interest.
#[test]
fn the_capture_sees_a_callsite_first_reached_by_a_thread_without_a_subscriber() {
    fn probe(origin: &'static str) {
        tracing::info!(origin, "capture probe");
    }

    let (captured, _guard) = capture_info_events();
    std::thread::spawn(|| {
        tracing::dispatcher::with_default(&tracing::Dispatch::none(), || {
            probe("a thread without a subscriber");
            tracing::callsite::rebuild_interest_cache();
        });
    })
    .join()
    .expect("the uncaptured probe thread finishes");
    probe("this thread");
    let raw = captured.lock().expect("captured events lock").clone();
    assert!(
        raw.iter()
            .any(|fields| fields.get("origin").map(String::as_str) == Some("this thread")),
        "captured events: {raw:?}"
    );
}

/// A caller deadline shorter than a held runtime unregister returns the
/// report, naming the member `UnregisterPending`, instead of waiting out the
/// lifecycle hang guard.
#[tokio::test]
async fn a_shutdown_deadline_reports_a_held_unregister_as_pending() {
    let (handle, service) = turn_driven_mob().await;
    let (busy, _) = spawn_member(&handle, "held-turn").await;
    service.set_block_runtime_turns(true);
    // Register before the turn starts: the start is a `notify_waiters` signal.
    let turn_started = service.runtime_turn_started.notified();
    tokio::pin!(turn_started);
    turn_started.as_mut().enable();
    let _turn = handle
        .member(&busy)
        .await
        .expect("member handle")
        .start_turn(
            ContentInput::Text("a turn that holds its runtime loop".into()),
            HandlingMode::Queue,
            crate::MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("turn admitted");
    tokio::time::timeout(STEP, turn_started)
        .await
        .expect("the turn is running");

    let started = Instant::now();
    let report = tokio::time::timeout(
        STEP,
        handle.shutdown_with_report(
            ShutdownOptions::default().with_deadline(Instant::now() + Duration::from_secs(1)),
        ),
    )
    .await
    .expect("shutdown returns at the caller's deadline")
    .expect("shutdown succeeds");
    assert!(started.elapsed() < STEP);
    assert!(
        matches!(
            report.members.get(&busy),
            Some(MemberShutdownOutcome::UnregisterPending { .. })
        ),
        "{report:?}"
    );
    service.set_block_runtime_turns(false);
    service.release_runtime_turns.notify_waiters();
}

/// The pending unregister a Shutdown deadline reports carries the member's
/// exact registration's settlement, and the settlement outlives the mob's
/// actor: while the held turn runs it stays pending; once the turn ends,
/// settling it joins the runtime's coordinator for that registration and
/// proves the registration gone.
#[tokio::test]
async fn a_pending_unregister_settles_after_the_shutdown_returns() {
    let (handle, service) = turn_driven_mob().await;
    let (busy, session) = spawn_member(&handle, "held-turn").await;
    service.set_block_runtime_turns(true);
    // Register before the turn starts: the start is a `notify_waiters` signal.
    let turn_started = service.runtime_turn_started.notified();
    tokio::pin!(turn_started);
    turn_started.as_mut().enable();
    let _turn = handle
        .member(&busy)
        .await
        .expect("member handle")
        .start_turn(
            ContentInput::Text("a turn that holds its runtime loop".into()),
            HandlingMode::Queue,
            crate::MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("turn admitted");
    tokio::time::timeout(STEP, turn_started)
        .await
        .expect("the turn is running");

    let report = tokio::time::timeout(
        STEP,
        handle.shutdown_with_report(
            ShutdownOptions::default().with_deadline(Instant::now() + Duration::from_secs(1)),
        ),
    )
    .await
    .expect("shutdown returns at the caller's deadline")
    .expect("shutdown succeeds");
    let mut settlement = match report.members.get(&busy) {
        Some(MemberShutdownOutcome::UnregisterPending {
            stage, settlement, ..
        }) => {
            assert_eq!(stage, "shutdown_runtime_unregister");
            settlement.clone()
        }
        other => panic!("expected the held member's unregister pending, got {other:?}"),
    };
    assert_eq!(settlement.try_state(), MemberUnregisterState::Pending);
    assert!(
        service
            .runtime_adapter
            .current_session_registration_witness(&session)
            .await
            .is_some(),
        "the held turn keeps the member's registration current"
    );
    assert_eq!(
        settlement
            .settle_within(Instant::now() + Duration::from_millis(200))
            .await,
        MemberUnregisterState::Pending,
        "the unregister stays pending while the turn holds it"
    );

    service.set_block_runtime_turns(false);
    service.release_runtime_turns();
    assert_eq!(
        settlement.settle_within(Instant::now() + STEP).await,
        MemberUnregisterState::Settled
    );
    assert!(
        service
            .runtime_adapter
            .current_session_registration_witness(&session)
            .await
            .is_none(),
        "the member's registration is gone"
    );
    assert_eq!(
        settlement.try_state(),
        MemberUnregisterState::Settled,
        "the settled state is kept"
    );
}

/// Each member's Shutdown outcome is logged as it settles, so a process killed
/// mid-Shutdown still leaves a per-member record.
#[tokio::test]
async fn shutdown_logs_each_member_outcome_as_it_settles() {
    let (captured, _guard) = capture_info_events();
    let (handle, service) = turn_driven_mob().await;
    let (first, _) = spawn_member(&handle, "first-peer").await;
    let (stuck, _) = spawn_member(&handle, "stuck-peer").await;
    stuck_retirement(&handle, &service, &stuck).await;

    let report = tokio::time::timeout(
        STEP,
        handle.shutdown_with_report(ShutdownOptions::default()),
    )
    .await
    .expect("shutdown completes")
    .expect("shutdown succeeds");
    // Every failure below names the whole capture, not only the outcomes
    // filtered from it.
    let raw = captured.lock().expect("captured events lock").clone();
    assert!(
        matches!(
            report.members.get(&first),
            Some(MemberShutdownOutcome::Unregistered)
        ),
        "report: {report:?}; captured events: {raw:?}"
    );
    assert!(
        matches!(
            report.members.get(&stuck),
            Some(MemberShutdownOutcome::RetirementStuck { .. })
        ),
        "report: {report:?}; captured events: {raw:?}"
    );
    let outcomes = raw
        .iter()
        .filter(|fields| {
            fields.get("message").map(String::as_str) == Some("shutdown member outcome")
        })
        .filter_map(|fields| {
            Some((
                fields.get("agent_identity")?.clone(),
                fields.get("outcome")?.clone(),
            ))
        })
        .collect::<Vec<_>>();
    assert!(
        outcomes
            .iter()
            .any(|(member, outcome)| member == first.as_str() && outcome.contains("Unregistered")),
        "outcomes: {outcomes:?}; captured events: {raw:?}"
    );
    assert!(
        outcomes.iter().any(
            |(member, outcome)| member == stuck.as_str() && outcome.contains("RetirementStuck")
        ),
        "outcomes: {outcomes:?}; captured events: {raw:?}"
    );
}

/// Replaces the retry-era `test_shutdown_retry_stops_member_spawned_after_
/// unregister_admission`: a member spawned concurrently with a single-attempt
/// Shutdown is either refused, or stopped, unregistered and reported. It is
/// never left running or unreported.
#[tokio::test]
async fn a_member_spawned_racing_shutdown_is_refused_or_stopped_and_reported() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let adapter = service.enable_runtime_adapter();
    let first = AgentIdentity::from("shutdown-race-a");
    let mut first_spec = SpawnMemberSpec::new("worker", first.as_str());
    first_spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
    handle
        .spawn_spec(first_spec)
        .await
        .expect("spawn first autonomous member");

    let racer = AgentIdentity::from("shutdown-race-b");
    let spawn_handle = handle.clone();
    let racer_identity = racer.clone();
    let spawn = tokio::spawn(async move {
        let mut spec = SpawnMemberSpec::new("worker", racer_identity.as_str());
        spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
        spawn_handle.spawn_spec(spec).await
    });
    let report = tokio::time::timeout(
        STEP,
        handle.shutdown_with_report(ShutdownOptions::default()),
    )
    .await
    .expect("shutdown completes")
    .expect("shutdown succeeds");
    let spawned = tokio::time::timeout(STEP, spawn)
        .await
        .expect("the racing spawn answers")
        .expect("join");

    assert!(
        matches!(
            report.members.get(&first),
            Some(MemberShutdownOutcome::Unregistered)
        ),
        "{report:?}"
    );
    match spawned {
        Err(_) => {
            // Refused: the actor admitted Shutdown first.
        }
        Ok(_) => {
            assert!(
                matches!(
                    report.members.get(&racer),
                    Some(MemberShutdownOutcome::Unregistered)
                ),
                "a racer that spawned before Shutdown is reported: {report:?}"
            );
        }
    }
    // Whichever way the race went, no member session is left running.
    let sessions = service
        .session_comms_names
        .read()
        .await
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for session_id in sessions {
        assert!(
            !adapter.contains_session(&session_id).await,
            "no member session is left registered after Shutdown: {session_id}"
        );
    }
}

/// One session's held registration transaction does not freeze Shutdown: the
/// teardown runs off the actor loop (a status query is answered meanwhile),
/// the other sessions unregister, and the held one is reported pending at its
/// admission within the caller's deadline.
#[tokio::test]
async fn shutdown_teardown_progresses_past_a_held_registration_transaction() {
    let (handle, service) = turn_driven_mob().await;
    let (held, held_session) = spawn_member(&handle, "held-registration").await;
    let (free, _) = spawn_member(&handle, "free-peer").await;
    let guard = service
        .runtime_adapter
        .hold_session_registration_transaction_for_test(&held_session)
        .await;
    let mut parked = handle
        .lifecycle_observations
        .shutdown_teardown_parked
        .subscribe();

    let shutdown_handle = handle.clone();
    let shutdown = tokio::spawn(async move {
        shutdown_handle
            .shutdown_with_report(
                ShutdownOptions::default().with_deadline(Instant::now() + Duration::from_secs(3)),
            )
            .await
    });
    tokio::time::timeout(STEP, parked.wait_for(|parked| *parked))
        .await
        .expect("the shutdown parks on its teardown")
        .expect("the probe outlives the test");
    // The actor serves a status query while the teardown runs.
    let status = tokio::time::timeout(STEP, handle.status())
        .await
        .expect("a status query is answered during the shutdown teardown");
    status.expect("the actor answers its status query");
    assert!(
        !shutdown.is_finished(),
        "the teardown is still waiting on the held session"
    );

    let report = tokio::time::timeout(STEP, shutdown)
        .await
        .expect("shutdown returns at the caller's deadline")
        .expect("join")
        .expect("shutdown succeeds");
    drop(guard);
    assert!(
        matches!(
            report.members.get(&free),
            Some(MemberShutdownOutcome::Unregistered)
        ),
        "{report:?}"
    );
    match report.members.get(&held) {
        Some(MemberShutdownOutcome::UnregisterPending { stage, .. }) => {
            assert_eq!(stage, "registration_transaction_admission");
        }
        other => panic!("expected the held session pending at admission, got {other:?}"),
    }
}

/// When Shutdown's deadline passes before the member's exact unregister is
/// admitted, its settlement admits that unregister itself, through the same
/// exact registration, once the registration transaction is free, and
/// proves the registration gone.
#[tokio::test]
async fn a_pending_unregister_admission_settles_once_the_registration_is_free() {
    let (handle, service) = turn_driven_mob().await;
    let (held, held_session) = spawn_member(&handle, "held-registration").await;
    let guard = service
        .runtime_adapter
        .hold_session_registration_transaction_for_test(&held_session)
        .await;
    let report = tokio::time::timeout(
        STEP,
        handle.shutdown_with_report(
            ShutdownOptions::default().with_deadline(Instant::now() + Duration::from_secs(1)),
        ),
    )
    .await
    .expect("shutdown returns at the caller's deadline")
    .expect("shutdown succeeds");
    let mut settlement = match report.members.get(&held) {
        Some(MemberShutdownOutcome::UnregisterPending {
            stage, settlement, ..
        }) => {
            assert_eq!(stage, "registration_transaction_admission");
            settlement.clone()
        }
        other => panic!("expected the held session pending at admission, got {other:?}"),
    };
    assert_eq!(settlement.try_state(), MemberUnregisterState::Pending);
    assert_eq!(
        settlement
            .settle_within(Instant::now() + Duration::from_millis(200))
            .await,
        MemberUnregisterState::Pending,
        "the held registration transaction keeps the admission pending"
    );

    drop(guard);
    assert_eq!(
        settlement.settle_within(Instant::now() + STEP).await,
        MemberUnregisterState::Settled
    );
    assert!(
        service
            .runtime_adapter
            .current_session_registration_witness(&held_session)
            .await
            .is_none(),
        "the member's registration is gone"
    );
}

/// A settlement holds the mob's runtime handle only weakly. Dropping that
/// handle ends nothing: the runtime and an unregister coordinator can still
/// run through another handle, and the registration stays current there. So
/// the settlement stays pending until the exact registration has ended, and
/// settles only then.
#[tokio::test]
async fn a_dropped_runtime_handle_settles_only_once_the_registration_has_ended() {
    let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    let session = SessionId::new();
    runtime
        .register_session(session.clone())
        .await
        .expect("register");
    let registration = runtime
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let registration_use = runtime.session_registration_use(&registration).await;
    let mut settlement =
        MemberUnregisterSettlement::new(registration, registration_use, None, &runtime, None);
    let other_handle = (*runtime).clone();
    drop(runtime);

    assert_eq!(settlement.try_state(), MemberUnregisterState::Pending);
    assert_eq!(
        settlement
            .settle_within(Instant::now() + Duration::from_millis(200))
            .await,
        MemberUnregisterState::Pending,
        "a dropped handle is no evidence that the registration ended"
    );
    assert!(
        other_handle
            .current_session_registration_witness(&session)
            .await
            .is_some(),
        "the registration is still current through the other handle"
    );

    other_handle
        .unregister_session(&session)
        .await
        .expect("unregister through the other handle");
    assert_eq!(
        settlement.settle_within(Instant::now() + STEP).await,
        MemberUnregisterState::Settled
    );
}

/// A later owner's runtime executor. It never runs anything: the tests only
/// need its attachment to exist.
struct LaterOwnerExecutor;

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutor for LaterOwnerExecutor {
    async fn apply(
        &mut self,
        _run_id: meerkat_core::RunId,
        _primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::lifecycle::core_executor::CoreExecutorError,
    > {
        Err(
            meerkat_core::lifecycle::core_executor::CoreExecutorError::Internal(
                "the later owner's fixture executor runs nothing".to_string(),
            ),
        )
    }

    async fn cancel_after_boundary(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        Ok(())
    }
}

/// R1, the in-place adoption: Shutdown returned with a member's exact
/// unregister not admitted, and its registration has no runtime loop. A
/// later owner then adopts that registration in place (the runtime upgrades
/// a loop-less registration and keeps its witness), so the exact
/// registration the settlement follows is now the later owner's. Settling
/// must not unregister it.
#[tokio::test]
async fn a_pending_unregister_leaves_a_registration_a_later_owner_adopted_in_place() {
    let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    let session = SessionId::new();
    runtime
        .register_session(session.clone())
        .await
        .expect("register");
    let registration = runtime
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    assert!(
        runtime
            .current_executor_attachment_witness(&session)
            .await
            .is_none(),
        "the registration has no runtime loop when Shutdown returns"
    );
    let registration_use = runtime.session_registration_use(&registration).await;
    let mut settlement = MemberUnregisterSettlement::new(
        registration.clone(),
        registration_use,
        None,
        &runtime,
        None,
    );

    runtime
        .ensure_session_with_executor(session.clone(), Box::new(LaterOwnerExecutor))
        .await
        .expect("a later owner adopts the registration in place");
    assert_eq!(
        runtime
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&registration),
        "the in-place adoption keeps the exact registration witness"
    );
    let adopted = runtime
        .current_executor_attachment_witness(&session)
        .await
        .expect("the later owner's executor is attached");

    let state = settlement.settle_within(Instant::now() + STEP).await;
    assert_eq!(
        runtime
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&registration),
        "the later owner's registration stays registered (settlement: {state:?})"
    );
    assert_eq!(
        runtime
            .current_executor_attachment_witness(&session)
            .await
            .as_ref(),
        Some(&adopted),
        "the later owner's executor stays attached (settlement: {state:?})"
    );
    assert_eq!(state, MemberUnregisterState::HandedOver);
}

/// R1, the loop-less reuse: Shutdown returned with a member's exact
/// unregister not admitted, and its registration has no runtime loop. A
/// later owner reuses that registration as it is (`register_session` finds
/// it and returns it, no runtime loop attached) and admits an input. The
/// settlement must not unregister the later owner's registration, and the
/// later owner's input stays queued.
#[tokio::test]
async fn a_pending_unregister_leaves_a_registration_a_later_owner_reused_without_a_loop() {
    use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;

    let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    let session = SessionId::new();
    runtime
        .register_session(session.clone())
        .await
        .expect("register");
    let registration = runtime
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let registration_use = runtime.session_registration_use(&registration).await;
    let mut settlement = MemberUnregisterSettlement::new(
        registration.clone(),
        registration_use,
        None,
        &runtime,
        None,
    );

    runtime
        .register_session(session.clone())
        .await
        .expect("a later owner reuses the existing registration");
    assert_eq!(
        runtime
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&registration),
        "the reuse keeps the exact registration witness"
    );
    let input = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
        "the later owner's queued input",
        None,
    ));
    let input_id = input.id().clone();
    runtime
        .accept_input(&session, input)
        .await
        .expect("the later owner's input is admitted");

    let state = settlement.settle_within(Instant::now() + STEP).await;
    assert_eq!(
        runtime
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&registration),
        "the later owner's registration stays registered (settlement: {state:?})"
    );
    let input_state = runtime.input_state(&session, &input_id).await;
    assert!(
        matches!(
            &input_state,
            Ok(Some(stored))
                if stored.seed.phase == meerkat_runtime::InputLifecycleState::Queued
        ),
        "the later owner's input stays queued: {input_state:?} (settlement: {state:?})"
    );
    assert_eq!(state, MemberUnregisterState::HandedOver);
}

/// R1, the loop-less reuse on a runtime with a durable store: as above, but
/// the later owner's `register_session` reuses the registration through the
/// stored registration path.
#[tokio::test]
async fn a_pending_unregister_leaves_a_stored_registration_a_later_owner_reused_without_a_loop() {
    use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;

    let runtime_store: Arc<dyn meerkat_runtime::store::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let runtime = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(runtime_store, blob_store)
            .expect("construct runtime authority"),
    );
    let session = SessionId::new();
    runtime
        .register_session(session.clone())
        .await
        .expect("register");
    let registration = runtime
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let registration_use = runtime.session_registration_use(&registration).await;
    let mut settlement = MemberUnregisterSettlement::new(
        registration.clone(),
        registration_use,
        None,
        &runtime,
        None,
    );

    runtime
        .register_session(session.clone())
        .await
        .expect("a later owner reuses the existing stored registration");
    assert_eq!(
        runtime
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&registration),
        "the reuse keeps the exact registration witness"
    );
    let input = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
        "the later owner's queued input",
        None,
    ));
    let input_id = input.id().clone();
    runtime
        .accept_input(&session, input)
        .await
        .expect("the later owner's input is admitted");

    let state = settlement.settle_within(Instant::now() + STEP).await;
    assert_eq!(
        runtime
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&registration),
        "the later owner's registration stays registered (settlement: {state:?})"
    );
    let input_state = runtime.input_state(&session, &input_id).await;
    assert!(
        matches!(
            &input_state,
            Ok(Some(stored))
                if stored.seed.phase == meerkat_runtime::InputLifecycleState::Queued
        ),
        "the later owner's input stays queued: {input_state:?} (settlement: {state:?})"
    );
    assert_eq!(state, MemberUnregisterState::HandedOver);
}

/// R1, the reclaimed live attachment: Shutdown returned with a member's exact
/// unregister not admitted while its runtime loop was still attached. A later
/// owner's executor registration then takes that live committed attachment
/// over as it is (the runtime answers that the session is already claimed and
/// keeps the registration's witness). Settling must neither unregister the
/// registration nor stop its loop.
#[tokio::test]
async fn a_pending_unregister_leaves_a_live_attachment_a_later_owner_reclaimed() {
    let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    let session = SessionId::new();
    runtime
        .register_session_with_executor(session.clone(), Box::new(LaterOwnerExecutor))
        .await
        .expect("register with a runtime loop");
    let registration = runtime
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let attachment = runtime
        .current_executor_attachment_witness(&session)
        .await
        .expect("the runtime loop is attached when Shutdown returns");
    let registration_use = runtime.session_registration_use(&registration).await;
    let mut settlement = MemberUnregisterSettlement::new(
        registration.clone(),
        registration_use,
        None,
        &runtime,
        None,
    );

    runtime
        .ensure_session_with_executor(session.clone(), Box::new(LaterOwnerExecutor))
        .await
        .expect("a later owner takes the live attachment over");
    assert_eq!(
        runtime
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&registration),
        "the takeover keeps the exact registration witness"
    );
    assert_eq!(
        runtime
            .current_executor_attachment_witness(&session)
            .await
            .as_ref(),
        Some(&attachment),
        "the later owner took over the same live attachment"
    );

    let state = settlement.settle_within(Instant::now() + STEP).await;
    assert_eq!(
        runtime
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&registration),
        "the later owner's registration stays registered (settlement: {state:?})"
    );
    assert_eq!(
        runtime
            .current_executor_attachment_witness(&session)
            .await
            .as_ref(),
        Some(&attachment),
        "the later owner's loop stays attached (settlement: {state:?})"
    );
    assert_eq!(state, MemberUnregisterState::HandedOver);
}

/// R1 end to end: member m's mob stops cleanly, and its Shutdown's deadline
/// passes before m's exact unregister is admitted. The same mob is then
/// reconstructed in-process over the same stores and session service. The
/// new incarnation's Stopped start materializes nothing, so m stays seated
/// and dormant and nothing takes the old incarnation's registration over:
/// settling the old incarnation's pending unregister unregisters it. After
/// the new incarnation's explicit resume, m's first touch revives it on the
/// same session with a registration of its own.
#[tokio::test]
async fn an_old_incarnations_pending_unregister_settles_and_the_resumed_member_registers_at_its_wake()
 {
    let definition = with_unique_mob_id(sample_definition(), "r1-resumed-member");
    let mob_id = definition.id.clone();
    let service = Arc::new(MockSessionService::new());
    let adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let member = AgentIdentity::from("r1-resumed-member");
    let mut spec = SpawnMemberSpec::new("worker", member.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    handle.spawn_spec(spec).await.expect("spawn member");
    let session = handle
        .get_member(&member)
        .await
        .expect("read the member")
        .and_then(|entry| entry.bridge_session_id().cloned())
        .expect("the member has a bridge session");
    handle.stop().await.expect("persist a clean stop");
    let registration = adapter
        .current_session_registration_witness(&session)
        .await
        .expect("the member's registration is current");

    let guard = adapter
        .hold_session_registration_transaction_for_test(&session)
        .await;
    let report = tokio::time::timeout(
        STEP,
        handle.shutdown_with_report(
            ShutdownOptions::default().with_deadline(Instant::now() + Duration::from_secs(1)),
        ),
    )
    .await
    .expect("shutdown returns at the caller's deadline")
    .expect("shutdown succeeds");
    let mut settlement = match report.members.get(&member) {
        Some(MemberShutdownOutcome::UnregisterPending {
            stage, settlement, ..
        }) => {
            assert_eq!(stage, "registration_transaction_admission");
            settlement.clone()
        }
        other => panic!("expected the member's unregister pending at admission, got {other:?}"),
    };
    drop(guard);
    drop(handle);
    await_supervisor_route_release(&mob_id).await;
    let use_at_shutdown = adapter.session_registration_use(&registration).await;
    assert!(
        use_at_shutdown.is_some(),
        "the old incarnation left its registration current"
    );

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .notify_orchestrator_on_resume(false)
    .resume()
    .await
    .expect("reconstruct the stopped mob in-process");
    assert_eq!(resumed.status().await.expect("phase"), MobState::Stopped);
    assert_eq!(
        resumed.member_materialization_for_test(&member),
        Some(crate::machines::mob_machine::MemberMaterializationPhase::Dormant),
        "the Stopped start materialized nothing"
    );
    assert_eq!(
        adapter.session_registration_use(&registration).await,
        use_at_shutdown,
        "nothing took the old registration over"
    );

    let state = settlement.settle_within(Instant::now() + STEP).await;
    assert_eq!(state, MemberUnregisterState::Settled);
    assert_eq!(
        adapter.current_session_registration_witness(&session).await,
        None,
        "the old incarnation's registration is unregistered"
    );

    tokio::time::timeout(STEP, resumed.resume())
        .await
        .expect("resume is bounded")
        .expect("explicit resume of the reconstructed mob");
    tokio::time::timeout(STEP, resumed.ensure_member_live(&member))
        .await
        .expect("the wake is bounded")
        .expect("the member's first touch wakes it");
    let revived = resumed
        .get_member(&member)
        .await
        .expect("read the revived member")
        .and_then(|entry| entry.bridge_session_id().cloned());
    assert_eq!(
        revived.as_ref(),
        Some(&session),
        "the member revives on the same session"
    );
    let revived_registration = adapter
        .current_session_registration_witness(&session)
        .await
        .expect("the revived member is registered");
    assert_ne!(
        revived_registration, registration,
        "the revived member registered anew"
    );
    assert_eq!(settlement.try_state(), MemberUnregisterState::Settled);
    resumed.shutdown().await.expect("shutdown the resumed mob");
}

/// The Shutdown admission check fails closed, as the Stop arm's does: every
/// Shutdown transition carries the run-start hold its arm realizes before the
/// member interrupts, and a transition without it is refused.
#[test]
fn shutdown_admission_requires_the_run_start_hold_effect() {
    use crate::machines::mob_machine as mob_dsl;
    let mut authority = mob_dsl::MobMachineAuthority::new();
    let shutdown =
        mob_dsl::MobMachineMutator::apply(&mut authority, mob_dsl::MobMachineInput::Shutdown)
            .expect("a running mob admits Shutdown");
    crate::runtime::actor::MobActor::require_member_run_start_effect(
        &shutdown,
        Some(true),
        "shutdown_command_admission",
    )
    .expect("the Shutdown transition carries the run-start hold");

    let mut authority = mob_dsl::MobMachineAuthority::new();
    let without_hold = mob_dsl::MobMachineMutator::apply(
        &mut authority,
        mob_dsl::MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
            intent: mob_dsl::PlacedCompletionLifecycleIntentKind::Destroy,
        },
    )
    .expect("a running mob admits a Destroy quiesce");
    assert!(
        crate::runtime::actor::MobActor::require_member_run_start_effect(
            &without_hold,
            Some(true),
            "shutdown_command_admission",
        )
        .is_err(),
        "a transition without the run-start hold is refused"
    );
}

/// Shutdown holds member run starts from every phase (#1500), Completed
/// included: a Completed mob shuts down, stays Completed, and its Shutdown
/// transition carried the hold the actor requires. Dropping the hold from
/// MobMachine's ShutdownCompleted arm fails this Shutdown at its admission.
#[tokio::test]
async fn a_completed_mob_shuts_down_holding_member_run_starts() {
    use crate::machines::mob_machine as mob_dsl;
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(
            ProfileName::from("lead"),
            AgentIdentity::from("lead-completed-shutdown"),
            None,
        )
        .await
        .expect("spawn lead");
    handle.complete().await.expect("complete the mob");
    assert_eq!(handle.status().await.unwrap(), MobState::Completed);

    tokio::time::timeout(
        Duration::from_secs(10),
        handle.shutdown_with_report(ShutdownOptions::default()),
    )
    .await
    .expect("the Shutdown of a Completed mob completes")
    .expect("a Completed mob shuts down");
    assert_eq!(handle.status().await.unwrap(), MobState::Completed);

    // The machine side of the same arm: a Completed mob's Shutdown carries
    // the hold and records it.
    let mut authority = mob_dsl::MobMachineAuthority::new();
    mob_dsl::MobMachineMutator::apply(
        &mut authority,
        mob_dsl::MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
            intent: mob_dsl::PlacedCompletionLifecycleIntentKind::Complete,
        },
    )
    .expect("a running mob begins its Complete quiesce");
    mob_dsl::MobMachineMutator::apply(&mut authority, mob_dsl::MobMachineInput::Complete)
        .expect("a running mob completes");
    let shutdown =
        mob_dsl::MobMachineMutator::apply(&mut authority, mob_dsl::MobMachineInput::Shutdown)
            .expect("a Completed mob admits Shutdown");
    crate::runtime::actor::MobActor::require_member_run_start_effect(
        &shutdown,
        Some(true),
        "shutdown_command_admission",
    )
    .expect("the Completed Shutdown carries the run-start hold");
    assert_eq!(
        authority.state().lifecycle_phase,
        mob_dsl::MobPhase::Completed
    );
    assert!(authority.state().member_run_starts_held);
}

/// A task the Shutdown joins has a request to this actor outstanding when the
/// Shutdown starts. The actor keeps answering while it joins (a typed
/// refusal), so the join completes and the Shutdown finishes. The inline
/// Shutdown used to wait on the task while the task waited on the actor,
/// until the process was killed.
#[tokio::test]
async fn shutdown_completes_while_a_joined_task_awaits_the_actor() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let (observed_tx, observed_rx) = oneshot::channel();
    let spawned = handle
        .enqueue_actor_command_for_test(|reply_tx| {
            MobCommand::SpawnLiveMutationAwaitingActorForTest {
                observed_tx,
                reply_tx,
            }
        })
        .await
        .expect("enqueue the actor-awaiting task");
    tokio::time::timeout(Duration::from_secs(10), spawned)
        .await
        .expect("the task is spawned")
        .expect("spawn reply")
        .expect("spawn the actor-awaiting task");

    tokio::time::timeout(
        Duration::from_secs(10),
        handle.shutdown_with_report(ShutdownOptions::default()),
    )
    .await
    .expect("the Shutdown completes while a task it joins awaits the actor")
    .expect("shutdown");
    let observed = observed_rx
        .await
        .expect("the task reports what the actor answered");
    assert!(
        matches!(observed, Err(MobError::ActorCommandChannelClosed)),
        "the actor refused the outstanding request typed while it joined: {observed:?}"
    );
}

/// An actor completion (the result of work the actor owns) that arrives while
/// a Shutdown joins its work is honoured: the actor processes it before it
/// replies and exits. Only caller requests are refused while it joins.
#[tokio::test]
async fn a_completion_arriving_while_shutdown_joins_its_work_is_honoured() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let honoured = Arc::new(tokio::sync::watch::Sender::new(false));
    let spawned = handle
        .enqueue_actor_command_for_test(|reply_tx| {
            MobCommand::SpawnLiveMutationSendingCompletionForTest {
                honoured: Arc::clone(&honoured),
                reply_tx,
            }
        })
        .await
        .expect("enqueue the completion-sending task");
    tokio::time::timeout(STEP, spawned)
        .await
        .expect("the task is spawned")
        .expect("spawn reply")
        .expect("spawn the completion-sending task");

    tokio::time::timeout(
        STEP,
        handle.shutdown_with_report(ShutdownOptions::default()),
    )
    .await
    .expect("the Shutdown completes")
    .expect("shutdown");
    assert!(
        *honoured.borrow(),
        "the actor processed the completion before the Shutdown replied"
    );
}
