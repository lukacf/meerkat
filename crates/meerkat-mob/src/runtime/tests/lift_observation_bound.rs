#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! The lift of a stopped mob observes the members it rebuilds at most its
//! restore concurrency at a time.
//!
//! A member the lift rebuilds (its live session went while the mob was
//! stopped, its durable session kept) gets one observation of its live and
//! durable session before it is provisioned again. On a host store that
//! observation loads the member's stored rows when they are not in memory
//! yet, so a lift of many members must not start every observation at once.
//! The row holds each observation at its start through the actor's
//! observation probe and counts how many run at once.
//!
//! Unrun until the candidate's validation.

use super::*;

/// Bound for one step; it only turns a hang into a failure.
const STEP: Duration = Duration::from_secs(30);

/// How long the row waits, with the bound's worth of observations held, for
/// one more to start. A lift without the bound starts every member's
/// observation within milliseconds of the first.
const SETTLE: Duration = Duration::from_millis(300);

/// The mob's restore concurrency.
const BOUND: usize = 2;

/// The members the lift rebuilds: more than two rounds of the bound.
const MEMBERS: usize = 5;

/// A lift of `MEMBERS` rebuilt members at restore concurrency `BOUND`: while
/// `BOUND` observations are held no other one starts, at most `BOUND` ever
/// run at once, each member is observed once, and every member is rebuilt.
///
/// RED on the base (e294b75f29, with the probe and the row applied): the
/// lift spawned every member's observation at once, so all `MEMBERS` are
/// held together, the first round counts `MEMBERS` and the peak is
/// `MEMBERS`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lift_observes_the_members_it_rebuilds_at_most_the_restore_concurrency_at_once() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let mut definition = with_unique_mob_id(sample_definition(), "lift-observation-bound");
    definition.orchestrator = None;
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service.clone())
        .with_resume_restore_concurrency(BOUND)
        .create()
        .await
        .expect("create the mob");
    let mut members = Vec::with_capacity(MEMBERS);
    for index in 0..MEMBERS {
        let identity = AgentIdentity::from(format!("lift-observed-{index}"));
        let mut spec = SpawnMemberSpec::new("worker", identity.clone());
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        handle.spawn_spec(spec).await.expect("spawn a worker");
        let session = handle
            .resolve_bridge_session_id(&identity)
            .await
            .expect("a session-backed worker");
        members.push((identity, session));
    }
    handle.stop().await.expect("stop the mob");
    // Each live session goes while the mob is stopped, its durable session
    // kept: the lift rebuilds every member, with one observation each.
    for (identity, session) in &members {
        MobSessionService::discard_live_session(service.as_ref(), session)
            .await
            .unwrap_or_else(|error| panic!("{identity}: discard the live session: {error}"));
    }

    let probe = crate::runtime::actor::probe_explicit_resume_observations_for_test(handle.mob_id());
    let lift = tokio::spawn({
        let handle = handle.clone();
        async move { handle.resume().await }
    });
    let reached = tokio::time::timeout(STEP, probe.wait_for_arrivals(BOUND))
        .await
        .is_ok();
    // A lift without the bound starts another observation while these are
    // held; a bounded one cannot until one of them ends.
    let _ = tokio::time::timeout(SETTLE, probe.wait_for_arrivals(BOUND + 1)).await;
    let first_round = probe.arrived();
    // Every observation goes on from here (a later one is not held), so a
    // failing row ends instead of hanging.
    probe.release(MEMBERS);

    let mut lifted = tokio::time::timeout(STEP, lift)
        .await
        .expect("the lift answers")
        .expect("lift task");
    if matches!(
        lifted,
        Err(MobError::LifecycleOperationProgressStalled { .. })
    ) {
        // The unit build's lift observer reports a stall after two seconds
        // without progress; the lift stays joinable.
        lifted = tokio::time::timeout(STEP, handle.resume())
            .await
            .expect("the joined lift answers");
    }
    lifted.expect("the lift succeeds");
    assert!(reached, "{BOUND} member observations start");
    assert_eq!(
        first_round, BOUND,
        "while {BOUND} observations were held, no other member's observation started"
    );
    assert_eq!(
        probe.max_running(),
        BOUND,
        "the lift ran at most {BOUND} member observations at once"
    );
    assert_eq!(
        probe.arrived(),
        MEMBERS,
        "the lift observed each member it rebuilt once"
    );
    for (identity, _) in &members {
        let snapshot = handle
            .member_status(identity)
            .await
            .unwrap_or_else(|error| panic!("{identity}: member status: {error}"));
        assert_eq!(
            snapshot.status,
            MobMemberStatus::Active,
            "{identity} is rebuilt: {snapshot:?}"
        );
    }
}
