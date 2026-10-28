#![cfg(feature = "runtime-adapter")]
//! The lift's post-commit work and the work a member run can wait on.
//!
//! The lift (an explicit resume of a stopped mob) settles MobMachine's
//! topology fence when its topology reconciliation settles, before its
//! post-commit work (the members' operation bindings). Graph work a member's
//! run can wait on (a spawn, a wire, a retire) waits only for that
//! reconciliation; host repair work (a registration reload, a supervisor
//! rotation) still waits for the whole resume. These rows hold one member's
//! operation binding at a gated step and observe both sides of that split.

use super::*;
use std::sync::atomic::AtomicBool;

/// Bound for steps that must happen promptly; it only turns a hang into a
/// failure.
const STEP: Duration = Duration::from_secs(10);

/// The lift of a stopped mob whose operation binding of one member is held
/// (a gated step of its post-commit work): a Spawn is admitted and answered
/// while the binding is held, and a registration reload of the member that
/// Spawn created, which the lift owes no post-commit work (so only the
/// resume's own fence can hold it), is parked until the lift completes and
/// answered after it.
///
/// RED on the base (8086477277, with the seam and the row applied): the
/// lift's topology fence stayed up until its post-commit work finished and
/// deferred every Spawn behind it, so the bounded Spawn times out with the
/// binding still held.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_lift_binding_admits_a_spawn_and_defers_a_reload_until_the_lift_completes() {
    let mob = create_isolation_mob(2).await;
    mob.handle.stop().await.expect("stop the mob");
    let (entered, release) = crate::runtime::actor::hold_explicit_resume_member_binding_for_test(
        mob.handle.mob_id(),
        mob.member(0),
    );
    let released = Arc::new(AtomicBool::new(false));
    let lift = tokio::spawn({
        let handle = mob.handle.clone();
        let released = Arc::clone(&released);
        async move {
            let lifted = handle.resume().await;
            (lifted, released.load(Ordering::SeqCst))
        }
    });
    tokio::time::timeout(STEP, entered)
        .await
        .expect("the lift reaches the held binding")
        .expect("binding hold armed");

    // Graph work a member run can wait on: admitted while the binding is
    // held.
    let spawned_identity = AgentIdentity::from("w-spawned-during-lift");
    let spawned = tokio::time::timeout(
        STEP,
        mob.handle
            .spawn(ProfileName::from("worker"), spawned_identity.clone(), None),
    )
    .await;

    // Host repair: a reload of the member the Spawn created. The member has
    // no wiring (the definition wires no workers), no kickoff and no
    // post-commit work of this lift, and the row waits for its spawn
    // activation to leave the actor's custody, so the actor can park its
    // reload only on the resume's fence. Parking is observed structurally,
    // through the actor's parked-control probe, and one actor round trip
    // after it shows the reload is still held.
    if matches!(spawned, Ok(Ok(_))) {
        spawn_activation_released(&mob, &spawned_identity).await;
    }
    let mut parked = mob
        .handle
        .lifecycle_observations
        .parked_member_controls
        .subscribe();
    let reload = matches!(spawned, Ok(Ok(_))).then(|| {
        tokio::spawn({
            let handle = mob.handle.clone();
            let member = spawned_identity.clone();
            async move { handle.reload_member_registration(&member).await }
        })
    });
    let reload_parked = match reload {
        Some(_) => tokio::time::timeout(
            STEP,
            parked.wait_for(|parked| parked.contains_key(&spawned_identity)),
        )
        .await
        .is_ok_and(|observed| observed.is_ok()),
        None => false,
    };
    let _ = mob.handle.status().await;
    let reload_answered_while_held = reload.as_ref().is_some_and(|reload| reload.is_finished());
    // Released whatever was observed, so a failing row ends instead of
    // hanging.
    released.store(true, Ordering::SeqCst);
    let _ = release.send(());

    let spawned = spawned
        .expect("the Spawn is answered while the lift's binding is held")
        .expect("the Spawn is admitted while the lift's binding is held");
    assert!(
        spawned.bridge_session_id().is_some(),
        "the spawned member is session-backed"
    );
    assert!(
        reload_parked,
        "the actor parks the reload while the lift is in its post-commit work"
    );
    assert!(
        !reload_answered_while_held,
        "a registration reload waits for the whole resume"
    );

    // The lift answers once its binding is released. In a unit build its
    // observer reports a stall after two seconds without progress while the
    // binding is held; the operation itself stays joinable, and is joined.
    let (lifted, answered_after_release) = tokio::time::timeout(STEP, lift)
        .await
        .expect("the lift answers once the binding is released")
        .expect("lift task");
    match lifted {
        Ok(()) => assert!(
            answered_after_release,
            "the lift did not complete while its binding was held"
        ),
        Err(MobError::LifecycleOperationProgressStalled { stage, .. }) => {
            assert_eq!(
                stage, "resume_operation_bindings",
                "the lift waited only on its held binding"
            );
            tokio::time::timeout(STEP, mob.handle.resume())
                .await
                .expect("the joined lift answers")
                .expect("the lift succeeds");
        }
        Err(error) => panic!("the lift failed: {error}"),
    }
    let reloaded = tokio::time::timeout(STEP, reload.expect("the reload was sent"))
        .await
        .expect("the parked reload is answered after the lift")
        .expect("reload task");
    assert!(
        reloaded.is_ok(),
        "the parked reload runs once the lift completed: {reloaded:?}"
    );
    assert_eq!(
        mob.handle.status().await.expect("status"),
        MobState::Running,
        "the lift reached ready"
    );
    mob.handle
        .resolve_bridge_session_id(&spawned_identity)
        .await
        .expect("the member spawned during the lift is a member");
}

/// Wait until no spawn activation of `member` is in the actor's custody,
/// polling the actor's custody census under a bound.
async fn spawn_activation_released(mob: &IsolationMob, member: &AgentIdentity) {
    tokio::time::timeout(STEP, async {
        loop {
            let census = mob
                .handle
                .enqueue_actor_command_for_test(|reply_tx| {
                    MobCommand::SpawnActivationCustodyProbe { reply_tx }
                })
                .await
                .expect("custody probe enqueue")
                .await
                .expect("custody probe reply");
            if !census.members.contains(member) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the spawn activation leaves the actor's custody");
}

/// The lift of a stopped mob binds a member whose session mutation gate is
/// held while its binding runs, as a member run's finalization holds that
/// gate: the member's registration already serves its binding, so the lift
/// restores its owner context through the serving-binding path and answers
/// while the gate is still held.
///
/// RED at a4d3b5e449: the binding ran the full restore, whose local session
/// bindings and composition recovery take that gate, so the lift waited at
/// its binding of w-0 until its observer reported the stall
/// (`resume_operation_bindings`, member w-0), which the row asserts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lift_binds_a_member_whose_session_mutation_gate_is_held() {
    let mob = create_isolation_mob(2).await;
    mob.handle.stop().await.expect("stop the mob");
    let served_before = crate::runtime::actor::explicit_resume_served_bindings_for_test(
        mob.handle.mob_id(),
        mob.member(0),
    );
    let (entered, release) = crate::runtime::actor::hold_explicit_resume_member_binding_for_test(
        mob.handle.mob_id(),
        mob.member(0),
    );
    let lift = tokio::spawn({
        let handle = mob.handle.clone();
        async move { handle.resume().await }
    });
    tokio::time::timeout(STEP, entered)
        .await
        .expect("the lift reaches w-0's binding")
        .expect("binding hold armed");
    // Hold w-0's session mutation gate, as its run's finalization would, and
    // only then let its binding go.
    let gate = tokio::time::timeout(
        STEP,
        mob.adapter
            .hold_session_mutation_gate_for_test(mob.session(0)),
    )
    .await
    .expect("w-0's session mutation gate is taken")
    .expect("w-0's session is registered");
    let _ = release.send(());
    // The unit build's lift observer reports a stall after two seconds
    // without progress, and the operation stays joinable. A stall report
    // while the gate is held is joined once more, the gate still held: a
    // lift that binds w-0 without the gate answers then; one that waits on
    // the gate stalls again at w-0's binding.
    let mut lifted = tokio::time::timeout(STEP, lift)
        .await
        .expect("the lift answers or reports a stall")
        .expect("lift task");
    if matches!(
        lifted,
        Err(MobError::LifecycleOperationProgressStalled { .. })
    ) {
        lifted = tokio::time::timeout(STEP, mob.handle.resume())
            .await
            .expect("the joined lift answers or reports a stall");
    }
    drop(gate);
    match lifted {
        Ok(()) => {}
        Err(MobError::LifecycleOperationProgressStalled {
            stage, member_id, ..
        }) => {
            assert_eq!(
                (stage, member_id.as_ref().map(AgentIdentity::as_str)),
                ("resume_operation_bindings", Some("w-0")),
                "the lift stalled while w-0's session mutation gate was held, outside its binding of w-0"
            );
            panic!(
                "the lift waited at its operation binding of w-0 for w-0's held session mutation gate"
            );
        }
        Err(error) => panic!("the lift failed: {error}"),
    }
    assert_eq!(
        crate::runtime::actor::explicit_resume_served_bindings_for_test(
            mob.handle.mob_id(),
            mob.member(0)
        ),
        served_before + 1,
        "the lift bound w-0 through the serving-binding path"
    );
    assert_eq!(
        mob.handle.status().await.expect("status"),
        MobState::Running,
        "the lift reached ready"
    );
}
