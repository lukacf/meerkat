//! Races at a held bootstrap's edges.
//!
//! A roster member embodied outside a held bootstrap after its freshness
//! read (here: registered and materialized by another path in between) is
//! never reported as held success. The held pass refuses it as `NotHeld`,
//! no empty or partial held success is returned, and the member that was
//! validly created stays intact: Core keeps it live, and its view never
//! reads `broken`.
//!
//! The pass's `Activate` linearizes its choice, not each member's native
//! activation: a member whose first `activate()` poll comes after the
//! deadline expires in Core while the pass stays activated, and every wake
//! outside a held pass refuses it typed (nothing re-creates it unheld) until
//! a fresh host completion; a member whose activation Core accepted before
//! the deadline keeps its outcome when its Meta publication is delayed past
//! the deadline.
//!
//! Every assertion reads Core (the mob's roster), never a Meta copy.
//!
//! A held start queued for admission behind a shutdown's join of unrelated
//! tracked work returns at its deadline, and at once when the owner closes.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::identity_first::held_bootstrap::{HeldActivationPark, arm_held_activation_park};
use crate::identity_first::{
    AgentIdentity, ContinuityStore, HeldBootstrapRefusal, HeldBootstrapStart,
    IdentityLifecycleState, IdentityRuntimeError, LocalContinuityStore, RestoreOutcome,
};

use super::held_identity_test_support::{
    governed_runtime, held_runtime, identity_runtime, install_roster, member,
};

/// The setup deadline of the activation edge cases: room for every member to
/// reach its hold and for the pass to choose `Activate` well before it.
const HOLD: Duration = Duration::from_secs(5);

/// Whether Core's mob holds a member for `identity` now.
async fn core_holds_member(runtime: &super::UnifiedRuntime, identity: &AgentIdentity) -> bool {
    runtime
        .mob_handle()
        .get_member(&crate::member_comms_id::roster_member_id_for_identity(
            identity.as_str(),
        ))
        .await
        .expect("Core's roster read")
        .is_some()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_embodied_after_the_freshness_read_refuses_the_held_pass() {
    let dir = tempfile::tempdir().unwrap();
    let continuity = Arc::new(LocalContinuityStore::in_memory().expect("continuity store"));
    let identity = AgentIdentity::parse("held-race").unwrap();
    let roster = vec![member(&identity)];
    let mut runtime = governed_runtime(&dir, "held-race").await;
    let identity_runtime = identity_runtime(
        &runtime,
        Arc::clone(&continuity) as Arc<dyn ContinuityStore>,
        "held-race",
    );
    // Between the held pass's freshness read and its restore, another path
    // registers and materializes the roster member. The competitor is
    // bounded, so the fixture never waits forever on one an implementation
    // excludes.
    let competitor = Arc::clone(&identity_runtime);
    let competing_spec = member(&identity);
    let competing_identity = identity.clone();
    // Whether Core held the competitor's member once its bounded wake
    // returned: the member was embodied outside the held pass.
    let embodied_outside = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let embodied_flag = Arc::clone(&embodied_outside);
    identity_runtime.set_held_after_freshness_hook(Box::new(move || {
        Box::pin(async move {
            competitor.register(competing_spec).await;
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                competitor.materialize(&competing_identity),
            )
            .await;
            if matches!(
                competitor.core_live_member(&competing_identity).await,
                Ok(Some(_))
            ) {
                embodied_flag.store(true, Ordering::SeqCst);
            }
        })
    }));
    let observer = Arc::clone(&identity_runtime);
    install_roster(&mut runtime, identity_runtime, &roster);
    let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let completion = Arc::clone(&completed);
    let expected = identity.clone();
    let result = runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(move |members, _completion| {
                Box::pin(async move {
                    // Completion is only ever offered the exact, nonempty
                    // held inventory of the roster.
                    let held: Vec<_> = members.into_iter().map(|member| member.identity).collect();
                    if held != vec![expected] {
                        return Err(format!("unexpected held inventory: {held:?}"));
                    }
                    completion.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                })
            }),
        )
        .await;
    let materialized_outside = embodied_outside.load(Ordering::SeqCst);
    if materialized_outside {
        // The competitor created the member between the freshness read and
        // the pass: the held pass must refuse it (`NotHeld`), never report an
        // empty held success, and never ask the host to complete.
        assert!(
            !completed.load(std::sync::atomic::Ordering::SeqCst),
            "the host is never asked to complete an inventory it does not hold"
        );
        match result {
            Err(IdentityRuntimeError::HeldBootstrapRefused(HeldBootstrapRefusal::NotHeld {
                identity: refused,
            })) => assert_eq!(refused, identity),
            other => panic!("a member embodied outside the pass was accepted: {other:?}"),
        }
    } else {
        // An implementation that excludes the competitor until the pass has
        // settled (its bounded wait above timed out) held the member itself.
        result.expect("the held bootstrap succeeds when nothing interleaves");
        assert!(completed.load(std::sync::atomic::Ordering::SeqCst));
    }
    assert_ne!(
        observer
            .status(&identity)
            .await
            .expect("status of the roster member")
            .state,
        IdentityLifecycleState::Broken,
        "a valid member is never left broken by the held pass"
    );
    assert!(
        core_holds_member(&runtime, &identity).await,
        "the validly created member stays live in Core"
    );
    let _ = runtime.shutdown().await;
}

/// ADR case (a), with its wake continuation: the pass chooses `Activate`
/// before the deadline, and one member's first `activate()` poll is held
/// until after it. Core expires that member, whose refusal keeps the expiry
/// (and its abort's cleanup outcome). The pass's choice stays `Activate`: the
/// start reports `Activated`, and the other member, whose activation Core
/// accepted before the deadline, is published. Nothing re-creates the
/// expired member as an unheld member (a fresh session its host's completion
/// never saw): every wake outside a held pass refuses it typed, because it
/// owes its host a fresh held completion.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_first_polled_after_the_deadline_expires_while_the_pass_stays_activated() {
    let dir = tempfile::tempdir().unwrap();
    let early = AgentIdentity::parse("held-poll-early").unwrap();
    let late = AgentIdentity::parse("held-poll-late").unwrap();
    let roster = vec![member(&early), member(&late)];
    let (mut runtime, _continuity) = held_runtime(&dir, "held-poll", &roster).await;
    let (parked, release) = arm_held_activation_park(&late, HeldActivationPark::BeforeFirstPoll);
    let deadline = tokio::time::Instant::now() + HOLD;
    let bootstrap = tokio::time::timeout(
        Duration::from_mins(1),
        runtime.bootstrap_installed_identity_first_context_held_until(
            &roster,
            deadline,
            Box::new(|_members, _completion| Box::pin(async { Ok(()) })),
        ),
    );
    let releasing = async {
        tokio::time::timeout_at(deadline, parked)
            .await
            .expect("the late member parks before the deadline")
            .expect("the park reports");
        // The member parks only after the pass's `Activate`.
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep_until(deadline + Duration::from_millis(200)).await;
        let _ = release.send(());
    };
    let (started, ()) = tokio::join!(bootstrap, releasing);
    let started = started
        .expect("the activated pass completes")
        .expect("the held bootstrap starts");
    let restored = match started {
        HeldBootstrapStart::Activated(restored) => restored,
        other => {
            panic!("a member's expiry never turns the pass's Activate into an Abort: {other:?}")
        }
    };
    assert!(
        matches!(
            restored.outcomes.get(&early),
            Some(RestoreOutcome::Created { .. })
        ),
        "{:?}",
        restored.outcomes.get(&early)
    );
    match restored.outcomes.get(&late) {
        Some(RestoreOutcome::Refused(error)) => assert!(
            error
                .to_string()
                .contains("expired at its setup deadline without activation"),
            "{error}"
        ),
        other => panic!("the late member's expiry is its actual outcome: {other:?}"),
    }
    assert!(core_holds_member(&runtime, &early).await);
    // Wake continuation: a wake outside a held pass refuses the expired
    // member typed, and nothing re-creates it unheld.
    let context = runtime
        .identity_first_context
        .clone()
        .expect("installed identity context");
    match context.runtime.materialize(&late).await {
        Err(IdentityRuntimeError::HeldCompletionOwed { identity }) => assert_eq!(identity, late),
        other => panic!("a wake outside a held pass refuses the expired member: {other:?}"),
    }
    for _ in 0..30 {
        assert!(
            !core_holds_member(&runtime, &late).await,
            "the expired member is not re-created unheld"
        );
        assert_eq!(
            runtime.mob_handle().list_members().await.len(),
            1,
            "only the activated sibling is published"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        context
            .runtime
            .status(&late)
            .await
            .expect("late member status")
            .state,
        IdentityLifecycleState::Dormant,
        "Core holds no member for the expired member and no final refusal stands: it stays \
         dormant until a fresh host completion"
    );
    let _ = runtime.shutdown().await;
}

/// ADR case (b), delayed Meta publication: Core's `activate()` accepted the
/// member before the deadline and returned, and the member's Meta
/// publication (the rest of its create) is parked until after the deadline.
/// This delays only Meta's side, after Core returned, not Core's own
/// publication. The start reports `Activated` after the deadline, Core holds
/// the member, and nothing is reclassified as an expiry or rolled back.
#[tokio::test(flavor = "multi_thread")]
async fn an_activation_accepted_before_the_deadline_keeps_its_delayed_meta_publication() {
    let dir = tempfile::tempdir().unwrap();
    let identity = AgentIdentity::parse("held-late-publication").unwrap();
    let roster = vec![member(&identity)];
    let (mut runtime, _continuity) = held_runtime(&dir, "held-late-publication", &roster).await;
    let (parked, release) =
        arm_held_activation_park(&identity, HeldActivationPark::AfterNativeActivation);
    let deadline = tokio::time::Instant::now() + HOLD;
    let bootstrap = tokio::time::timeout(
        Duration::from_mins(1),
        runtime.bootstrap_installed_identity_first_context_held_until(
            &roster,
            deadline,
            Box::new(|_members, _completion| Box::pin(async { Ok(()) })),
        ),
    );
    let releasing = async {
        tokio::time::timeout_at(deadline, parked)
            .await
            .expect("Core accepts the activation before the deadline")
            .expect("the park reports");
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep_until(deadline + Duration::from_millis(200)).await;
        let _ = release.send(());
    };
    let (started, ()) = tokio::join!(bootstrap, releasing);
    let started = started
        .expect("the activated pass completes")
        .expect("the held bootstrap starts");
    assert!(
        tokio::time::Instant::now() > deadline,
        "the Meta publication finished after the deadline"
    );
    let restored = match started {
        HeldBootstrapStart::Activated(restored) => restored,
        other => panic!("an accepted activation is never reported as an abort: {other:?}"),
    };
    assert!(
        matches!(
            restored.outcomes.get(&identity),
            Some(RestoreOutcome::Created { .. })
        ),
        "{:?}",
        restored.outcomes.get(&identity)
    );
    assert!(core_holds_member(&runtime, &identity).await);
    assert_eq!(runtime.mob_handle().list_members().await.len(), 1);
    let _ = runtime.shutdown().await;
}

/// The bounded start's admission into the identity runtime's tracked
/// foreground owner answers its deadline and the owner's closure. A
/// shutdown's join holds the owner's lock while it waits for unrelated
/// tracked work; a held start queued behind it returns at its deadline with
/// nothing admitted and the host never asked, and one still queued when the
/// owner closes returns at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_start_queued_behind_a_shutdown_join_returns_at_its_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let identity = AgentIdentity::parse("held-admission").unwrap();
    let roster = vec![member(&identity)];
    let continuity: Arc<dyn ContinuityStore> =
        Arc::new(LocalContinuityStore::in_memory().expect("continuity store"));
    let mut runtime = governed_runtime(&dir, "held-admission").await;
    let identity_runtime = identity_runtime(&runtime, continuity, "held-admission");
    install_roster(&mut runtime, Arc::clone(&identity_runtime), &roster);
    // Unrelated tracked work that only the test ends.
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let unrelated = {
        let identity_runtime = Arc::clone(&identity_runtime);
        tokio::spawn(async move {
            identity_runtime
                .run_tracked_foreground(async move {
                    let _ = entered_tx.send(());
                    let _ = release_rx.await;
                    Ok::<(), IdentityRuntimeError>(())
                })
                .await
        })
    };
    entered_rx.await.expect("the unrelated work is admitted");
    // The shutdown's join, polled once: it holds the owner's lock while it
    // waits for that work.
    let mut joining = Box::pin({
        let identity_runtime = Arc::clone(&identity_runtime);
        async move { identity_runtime.join_foreground_operations().await }
    });
    assert!(
        futures::poll!(joining.as_mut()).is_pending(),
        "the join waits for the unrelated work"
    );
    let joining = tokio::spawn(joining);
    let asked = Arc::new(AtomicUsize::new(0));
    let completion = |asked: &Arc<AtomicUsize>| -> super::HeldIdentityCompletion {
        let asked = Arc::clone(asked);
        Box::new(move |_members, _completion| {
            Box::pin(async move {
                asked.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        })
    };

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let start = tokio::time::timeout_at(
        deadline + Duration::from_secs(5),
        runtime.bootstrap_installed_identity_first_context_held_until(
            &roster,
            deadline,
            completion(&asked),
        ),
    )
    .await
    .expect("the start is bounded by its deadline")
    .expect("the held bootstrap starts");
    assert!(tokio::time::Instant::now() >= deadline);
    let HeldBootstrapStart::Unsettled(settlement) = start else {
        panic!("a start that was never admitted activates nothing");
    };
    match settlement.settled().await {
        Err(IdentityRuntimeError::HeldBootstrapRefused(HeldBootstrapRefusal::DeadlineElapsed)) => {}
        other => panic!("the queued start ends at its deadline: {other:?}"),
    }

    // A second start, queued behind the same join, when the owner closes.
    let closing = Arc::clone(&identity_runtime);
    let closer = async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        closing.close_foreground_operations();
        tokio::time::Instant::now()
    };
    let second = tokio::time::timeout(
        Duration::from_secs(10),
        runtime.bootstrap_installed_identity_first_context_held_until(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            completion(&asked),
        ),
    );
    let (second, closed_at) = tokio::join!(second, closer);
    let second = second
        .expect("the owner's closure ends the queued start")
        .expect("the held bootstrap starts");
    assert!(
        closed_at.elapsed() < Duration::from_secs(2),
        "the queued start returns at the closure, not at its deadline"
    );
    let HeldBootstrapStart::Unsettled(settlement) = second else {
        panic!("a start refused at the closure activates nothing");
    };
    match settlement.settled().await {
        Err(IdentityRuntimeError::Internal(reason)) if reason.contains("shutting down") => {}
        other => panic!("the queued start is refused by the closure: {other:?}"),
    }
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the host was never asked");

    let _ = release_tx.send(());
    unrelated
        .await
        .expect("unrelated task")
        .expect("unrelated work");
    joining.await.expect("join task");
    let _ = runtime.shutdown().await;
}
