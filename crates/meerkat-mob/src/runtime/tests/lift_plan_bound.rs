#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! The lift of a stopped mob dispatches its member plan at most the restore
//! concurrency at a time.
//!
//! A stopped mob is restarted (every member Dormant) and lifted under the
//! Eager policy, so the lift's plan materializes every member. The plan
//! applies `PlanBootMaterialization` to the next member only while fewer
//! than the restore concurrency of its wakes are open: a member it has not
//! reached stays Dormant, with no wake attempt (so no wake bound runs for
//! it), and a delivery wakes it at once through a wake of its own. When the
//! plan reaches a member a delivery woke first, it watches that wake and
//! applies its input once the wake leaves the member Dormant again. The rows
//! hold the members' restores at the wake restore probe (a wake's cold
//! restore, and the lift's rebuild preparation of a member it rebuilds
//! through the explicit-resume work path) and read each member's phase and
//! wake attempt from the machine state the actor publishes.
//!
//! Unrun until the candidate's validation.

use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::MemberRestorePolicy;
use crate::runtime::actor::member_wake::{MemberWakeFailure, MemberWakeOutcome};
use crate::runtime::actor::member_wake_turn::probe_member_wake_restores_for_test;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Bound for one step; it only turns a hang into a failure.
const STEP: Duration = Duration::from_secs(30);

/// How long a row waits, with the bound's worth of restores held, for one
/// more to start. A lift without the bound starts every member's restore
/// within milliseconds of the first.
const SETTLE: Duration = Duration::from_millis(300);

/// The mob's restore concurrency.
const BOUND: usize = 2;

/// The members the lift's plan materializes: more than two rounds of the
/// bound.
const MEMBERS: usize = 5;

/// The wake bound of the row that lets a delivery's wake time out. The
/// delivery's wake must still be open when the plan reaches its member, after
/// the plan's first wake is released and three more run one after another,
/// so it is far above what those wakes take even on a loaded runner; the row
/// then waits about this long for the bound to end the held wake.
const WAKE_BOUND: Duration = Duration::from_secs(10);

/// The wake bound of the row whose plan wake times out at its held restore
/// while a later wake of the same member is held only briefly: long enough
/// that the later wake, released within milliseconds, never meets it.
const SUPERSEDED_WAKE_BOUND: Duration = Duration::from_secs(5);

/// The wake bound of the row whose rebuilt member's preparation is held:
/// long enough that every other member, prepared and rebuilt on its own, is
/// Live well within it even on a loaded runner.
const REBUILD_WAKE_BOUND: Duration = Duration::from_secs(5);

fn worker(index: usize) -> AgentIdentity {
    AgentIdentity::from(format!("plan-bound-{index}"))
}

/// A stopped mob of `MEMBERS` turn-driven workers after a restart: the
/// resumed handle is Stopped, every member Dormant, and its builder asked for
/// the Eager policy at the row's restore concurrency (and wake bound).
struct StoppedMob {
    handle: MobHandle,
    service: Arc<MockSessionService>,
    /// Each worker and its session, in identity order (the plan's order).
    sessions: Vec<(AgentIdentity, SessionId)>,
    /// The members' comms identity root, for members that were seated.
    _keys: Option<tempfile::TempDir>,
}

impl StoppedMob {
    /// At restore concurrency `BOUND`, with the default wake bound.
    async fn new(label: &str) -> Self {
        Self::configured(label, BOUND, None).await
    }

    /// Members that were built before the stop: the lift rebuilds each
    /// through the explicit-resume work path.
    async fn configured(label: &str, bound: usize, wake_bound: Option<Duration>) -> Self {
        Self::built(label, bound, wake_bound, false).await
    }

    /// Members seated Dormant and never built (their sessions are unborn):
    /// the lift's plan wakes each through the actor's wake path, whose cold
    /// restore creates the session, under the wake bound.
    async fn configured_seated(label: &str, bound: usize, wake_bound: Option<Duration>) -> Self {
        Self::built(label, bound, wake_bound, true).await
    }

    async fn built(label: &str, bound: usize, wake_bound: Option<Duration>, seated: bool) -> Self {
        let keys = seated.then(|| tempfile::tempdir().expect("a comms identity root"));
        let definition = with_unique_mob_id(sample_definition(), label);
        let service = Arc::new(MockSessionService::new());
        let _ = service.enable_runtime_adapter();
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let runtime_metadata = storage.runtime_metadata.clone();
        let mut builder =
            MobBuilder::new(definition, storage).with_session_service(service.clone());
        if let Some(keys) = keys.as_ref() {
            builder = builder.with_member_comms_identity_root(keys.path());
        }
        let handle = builder.create().await.expect("create the mob");
        let specs: Vec<SpawnMemberSpec> = (0..MEMBERS)
            .map(|index| {
                let mut spec = SpawnMemberSpec::new("worker", worker(index));
                spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
                spec
            })
            .collect();
        let mut sessions = Vec::with_capacity(MEMBERS);
        if seated {
            let outcomes = handle
                .seat_dormant_members(specs)
                .await
                .expect("the seat batch answers");
            for (index, outcome) in outcomes.iter().enumerate() {
                let crate::runtime::DormantSeatOutcome::Seated { session_id, .. } = outcome else {
                    panic!("every fresh spec is seated: {outcome:?}");
                };
                sessions.push((worker(index), session_id.clone()));
            }
        } else {
            for spec in specs {
                let identity = spec.identity.clone();
                handle.spawn_spec(spec).await.expect("spawn a worker");
                let session = handle
                    .resolve_bridge_session_id(&identity)
                    .await
                    .expect("a session-backed worker");
                sessions.push((identity, session));
            }
        }
        handle.stop().await.expect("stop the mob");
        let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        crash_stop_and_release_routes(handle).await;
        drop(service);
        let mut builder = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
            events,
            runtime_metadata,
        ))
        .with_session_service(restarted.clone())
        .notify_orchestrator_on_resume(false)
        .with_member_restore_policy(MemberRestorePolicy::Eager)
        .with_resume_restore_concurrency(bound);
        if let Some(keys) = keys.as_ref() {
            builder = builder.with_member_comms_identity_root(keys.path());
        }
        if let Some(wake_bound) = wake_bound {
            builder = builder.with_member_wake_timeout(wake_bound);
        }
        let handle = tokio::time::timeout(STEP, Box::pin(builder.resume()))
            .await
            .expect("the restart answers")
            .expect("the restart succeeds");
        assert_eq!(
            handle.status().await.expect("status"),
            MobState::Stopped,
            "the restarted mob is stopped"
        );
        for (identity, _) in &sessions {
            assert_eq!(
                handle.member_materialization_for_test(identity),
                Some(dsl::MemberMaterializationPhase::Dormant),
                "{identity} is Dormant after the restart"
            );
        }
        Self {
            handle,
            service: restarted,
            sessions,
            _keys: keys,
        }
    }

    /// Lift the mob in a task of its own.
    fn lift(&self) -> tokio::task::JoinHandle<Result<(), MobError>> {
        let handle = self.handle.clone();
        tokio::spawn(async move { handle.resume().await })
    }

    /// Join the lift and answer its result: the unit build's lift observer
    /// reports a stall after two seconds without progress, and the lift stays
    /// joinable. The rows read the result last, after the plan's own facts.
    async fn join_lift(
        &self,
        lift: tokio::task::JoinHandle<Result<(), MobError>>,
    ) -> Result<(), MobError> {
        let lifted = tokio::time::timeout(STEP, lift)
            .await
            .expect("the lift answers")
            .expect("lift task");
        if matches!(
            lifted,
            Err(MobError::LifecycleOperationProgressStalled { .. })
        ) {
            return tokio::time::timeout(STEP, self.handle.resume())
                .await
                .expect("the joined lift answers");
        }
        lifted
    }

    /// Each member's phase and whether it holds a wake attempt, as the actor
    /// last published them.
    fn residency(
        &self,
    ) -> BTreeMap<AgentIdentity, (Option<dsl::MemberMaterializationPhase>, bool)> {
        let state = self.handle.machine_state_watch_rx.borrow();
        self.sessions
            .iter()
            .map(|(identity, _)| {
                let id = dsl::AgentIdentity::from_domain(identity);
                (
                    identity.clone(),
                    (
                        state.member_materialization.get(&id).copied(),
                        state.member_wake_attempts.contains_key(&id),
                    ),
                )
            })
            .collect()
    }

    /// The machine's wake attempt sequence, as the actor last published it:
    /// every wake attempt opened so far.
    fn wake_attempts(&self) -> u64 {
        self.handle
            .machine_state_watch_rx
            .borrow()
            .member_wake_attempt_sequence
    }

    /// `identity`'s open wake attempt, as the actor last published it.
    fn wake_attempt_of(&self, identity: &AgentIdentity) -> Option<u64> {
        self.handle
            .machine_state_watch_rx
            .borrow()
            .member_wake_attempts
            .get(&dsl::AgentIdentity::from_domain(identity))
            .copied()
    }

    /// Wait, bounded by `STEP`, until the published state satisfies `done`;
    /// whether it did.
    async fn wait_for_state(&self, done: impl Fn(&dsl::MobMachineState) -> bool) -> bool {
        let mut published = self.handle.machine_state_watch_rx.clone();
        tokio::time::timeout(STEP, published.wait_for(done))
            .await
            .is_ok_and(|seen| seen.is_ok())
    }

    /// Wait, bounded by `STEP`, until the published state holds every member
    /// Live with no wake attempt open. A lift that answered before its plan's
    /// last wakes closed (a failed lift) is read after them.
    async fn settle_live(&self) {
        let members: Vec<dsl::AgentIdentity> = self
            .sessions
            .iter()
            .map(|(identity, _)| dsl::AgentIdentity::from_domain(identity))
            .collect();
        let _ = self
            .wait_for_state(|state| {
                members.iter().all(|id| {
                    state.member_materialization.get(id)
                        == Some(&dsl::MemberMaterializationPhase::Live)
                        && !state.member_wake_attempts.contains_key(id)
                })
            })
            .await;
    }

    /// How many times the restarted service built each member's session.
    async fn builds(&self) -> BTreeMap<AgentIdentity, usize> {
        let created = self.service.recorded_create_requests().await;
        self.sessions
            .iter()
            .map(|(identity, session)| {
                (
                    identity.clone(),
                    created
                        .iter()
                        .filter(|record| &record.session_id == session)
                        .count(),
                )
            })
            .collect()
    }
}

/// The most members the published machine state held Waking at once, from
/// now on, until the task is aborted.
fn watch_waking_peak(handle: &MobHandle) -> (Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let peak = Arc::new(AtomicUsize::new(0));
    let recorded = Arc::clone(&peak);
    let mut published = handle.machine_state_watch_rx.clone();
    let task = tokio::spawn(async move {
        loop {
            let waking = published
                .borrow_and_update()
                .member_materialization
                .values()
                .filter(|phase| matches!(phase, dsl::MemberMaterializationPhase::Waking))
                .count();
            recorded.fetch_max(waking, Ordering::SeqCst);
            if published.changed().await.is_err() {
                return;
            }
        }
    });
    (peak, task)
}

/// A lift of `MEMBERS` Dormant members that its Eager plan materializes, at
/// restore concurrency `BOUND`, every restore held until the row releases
/// it: each round starts exactly the bound's worth of the next members in
/// identity order (the last round the rest), the members the plan has not
/// reached are Dormant without a wake attempt while a round is held, no more
/// than `BOUND` members are ever Waking at once, and each member wakes under
/// exactly one attempt (none timed out and was woken again), is restored and
/// built once, and ends Live. The lift's own result is read last.
///
/// RED on the base (e294b75f29, with the probe and the row applied): the
/// lift planned every Dormant member at once, so all `MEMBERS` were Waking
/// and their restores started together: the first round counts `MEMBERS`,
/// no member is left Dormant, and the peak is `MEMBERS`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lift_dispatches_its_plan_at_most_the_restore_concurrency_at_once() {
    let mob = StoppedMob::new("lift-plan-bound").await;
    let probe = probe_member_wake_restores_for_test(
        mob.handle.mob_id(),
        mob.sessions.iter().map(|(identity, _)| identity.clone()),
    );
    let (peak, watcher) = watch_waking_peak(&mob.handle);
    let attempts_before = mob.wake_attempts();
    let lift = mob.lift();

    let mut rounds = Vec::new();
    let mut released = 0;
    while released < MEMBERS {
        let round = BOUND.min(MEMBERS - released);
        let reached = tokio::time::timeout(STEP, probe.wait_for_started(released + round))
            .await
            .is_ok();
        // A lift without the bound starts another restore while these are
        // held; a bounded one cannot until one of these wakes closes.
        let _ = tokio::time::timeout(SETTLE, probe.wait_for_started(released + round + 1)).await;
        let started = probe.started();
        rounds.push((started[released..].to_vec(), mob.residency()));
        // Released whatever started, so a failing row ends instead of
        // hanging.
        for identity in &started[released..] {
            probe.release(identity);
        }
        if !reached || started.len() == released {
            break;
        }
        released = started.len();
    }
    let lifted = mob.join_lift(lift).await;
    mob.settle_live().await;
    watcher.abort();

    let mut reached = 0;
    for (index, (started, residency)) in rounds.iter().enumerate() {
        let expected: BTreeSet<AgentIdentity> = (reached..(reached + BOUND).min(MEMBERS))
            .map(worker)
            .collect();
        assert_eq!(
            started.iter().cloned().collect::<BTreeSet<_>>(),
            expected,
            "round {index} starts the next {BOUND} members in identity order, and no more"
        );
        for (position, (identity, _)) in mob.sessions.iter().enumerate() {
            let (phase, attempt) = residency[identity];
            if position < reached {
                assert_eq!(
                    (phase, attempt),
                    (Some(dsl::MemberMaterializationPhase::Live), false),
                    "round {index}: {identity}, of an earlier round, is Live"
                );
            } else if expected.contains(identity) {
                assert_eq!(
                    (phase, attempt),
                    (Some(dsl::MemberMaterializationPhase::Waking), true),
                    "round {index}: {identity} is Waking under its own attempt"
                );
            } else {
                assert_eq!(
                    (phase, attempt),
                    (Some(dsl::MemberMaterializationPhase::Dormant), false),
                    "round {index}: {identity}, not reached yet, is Dormant with no wake attempt"
                );
            }
        }
        reached += expected.len();
    }
    assert_eq!(
        reached, MEMBERS,
        "the rounds reached every member: {rounds:?}"
    );
    assert!(
        peak.load(Ordering::SeqCst) <= BOUND,
        "no more than {BOUND} members were ever Waking at once (peak {})",
        peak.load(Ordering::SeqCst)
    );
    assert_eq!(
        probe.max_running(),
        BOUND,
        "at most {BOUND} member restores ran at once"
    );
    let mut started = probe.started();
    started.sort();
    assert_eq!(
        started,
        mob.sessions
            .iter()
            .map(|(identity, _)| identity.clone())
            .collect::<Vec<_>>(),
        "each member was restored once"
    );
    for (identity, builds) in mob.builds().await {
        assert_eq!(builds, 1, "{identity} was built once");
    }
    for (identity, (phase, attempt)) in mob.residency() {
        assert_eq!(
            (phase, attempt),
            (Some(dsl::MemberMaterializationPhase::Live), false),
            "{identity} ends Live"
        );
    }
    assert_eq!(
        mob.wake_attempts() - attempts_before,
        MEMBERS as u64,
        "each member woke under exactly one attempt: none timed out and was woken again"
    );
    assert!(lifted.is_ok(), "the lift succeeds: {lifted:?}");
}

/// While the lift's first two plan wakes are held, a peer message to the
/// last member (the `WakeMember` its inproc wake route sends) wakes it at
/// once through a wake of its own: it is Live while the plan's wakes are
/// still held. When the plan reaches it, the plan applies nothing for it:
/// it is restored and built once, by its own wake, and every member wakes
/// under exactly one attempt. The lift's own result is read last.
///
/// RED on the base (e294b75f29, with the probe and the row applied): the
/// lift planned every member at once, so the last three were Waking (or
/// already Live through the plan's own restores) before the message came,
/// not Dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_message_wakes_a_member_the_lift_has_not_reached_at_once() {
    let mob = StoppedMob::new("lift-plan-bypass").await;
    let held = [worker(0), worker(1)];
    let queued = worker(MEMBERS - 1);
    let probe = probe_member_wake_restores_for_test(mob.handle.mob_id(), held.clone());
    let attempts_before = mob.wake_attempts();
    let lift = mob.lift();
    let reached = tokio::time::timeout(STEP, probe.wait_for_started(BOUND))
        .await
        .is_ok();
    let _ = tokio::time::timeout(SETTLE, probe.wait_for_started(BOUND + 1)).await;
    let before = mob.residency();

    let answer = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| crate::runtime::state::MobCommand::WakeMember {
            agent_identity: queued.clone(),
            cause: dsl::MemberWakeCause::PeerDelivery,
            observed_missing: false,
            input_class: dsl::PeerInputClass::Content,
            sender_waking: false,
            reply_tx,
        })
        .await
        .expect("the peer wake reaches the actor");
    let woken = tokio::time::timeout(STEP, answer).await;
    let during = mob.residency();
    for identity in &held {
        probe.release(identity);
    }
    let lifted = mob.join_lift(lift).await;
    mob.settle_live().await;

    assert!(reached, "the plan's first {BOUND} restores start");
    for (identity, expected) in [
        (
            worker(0),
            (Some(dsl::MemberMaterializationPhase::Waking), true),
        ),
        (
            worker(1),
            (Some(dsl::MemberMaterializationPhase::Waking), true),
        ),
        (
            worker(2),
            (Some(dsl::MemberMaterializationPhase::Dormant), false),
        ),
        (
            worker(3),
            (Some(dsl::MemberMaterializationPhase::Dormant), false),
        ),
        (
            queued.clone(),
            (Some(dsl::MemberMaterializationPhase::Dormant), false),
        ),
    ] {
        assert_eq!(
            before[&identity], expected,
            "before the message, {identity} is as the bounded plan leaves it"
        );
    }
    assert!(
        matches!(woken, Ok(Ok(Ok(MemberWakeOutcome::Ready)))),
        "the peer message's wake makes the member ready: {woken:?}"
    );
    assert_eq!(
        during[&queued],
        (Some(dsl::MemberMaterializationPhase::Live), false),
        "the member is Live while the plan's wakes are still held"
    );
    for identity in &held {
        assert_eq!(
            during[identity],
            (Some(dsl::MemberMaterializationPhase::Waking), true),
            "{identity}'s plan wake is still held"
        );
    }
    assert_eq!(
        probe
            .started()
            .iter()
            .filter(|identity| **identity == queued)
            .count(),
        1,
        "the plan's later dispatch did not wake the member again"
    );
    for (identity, builds) in mob.builds().await {
        assert_eq!(builds, 1, "{identity} was built once");
    }
    for (identity, (phase, attempt)) in mob.residency() {
        assert_eq!(
            (phase, attempt),
            (Some(dsl::MemberMaterializationPhase::Live), false),
            "{identity} ends Live"
        );
    }
    assert_eq!(
        mob.wake_attempts() - attempts_before,
        MEMBERS as u64,
        "each member woke under exactly one attempt: the plan woke none a second time"
    );
    assert!(lifted.is_ok(), "the lift succeeds: {lifted:?}");
}

/// At restore concurrency 1, while the plan's first wake is held, a peer
/// message wakes the last member through a wake of its own, held at its
/// restore. The plan wakes the other members and reaches the last one while
/// that wake runs: it applies nothing to it and watches the wake. The wake's
/// bound then expires (the delivery is answered with the wake timeout) and
/// leaves the member Dormant again, owing nothing of its own. The plan
/// reaches it again and applies its input: the member wakes again under one
/// more attempt, held at its restore while the lift's attempt stays open,
/// and ends Live once released. The timed-out restore is ended at its start
/// before it reads anything, so no provision of it overlaps the new one. The
/// lift's own result is read last.
///
/// RED at 642f917f8d (the plan skipped a member no longer Dormant): the plan
/// applied nothing to the member and did not watch its wake, so the lift
/// finished with the other members; once that wake timed out nothing woke
/// the member again: no second restore starts, the lift's attempt is closed
/// and the member stays Dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_a_delivery_woke_first_is_planned_once_that_wake_leaves_it_dormant() {
    let mob = StoppedMob::configured("lift-plan-rewake", 1, Some(WAKE_BOUND)).await;
    let first = worker(0);
    let woken = worker(MEMBERS - 1);
    let probe =
        probe_member_wake_restores_for_test(mob.handle.mob_id(), [first.clone(), woken.clone()]);
    // The plan's later wake of the member is held at its restore too.
    probe.hold(&woken, 2);
    let attempts_before = mob.wake_attempts();
    let lift = mob.lift();

    // The plan's first wake is held at its restore; then a peer message wakes
    // the last member through a wake of its own, held at its restore too.
    let first_held = tokio::time::timeout(STEP, probe.wait_for_started(1))
        .await
        .is_ok();
    let answer = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| crate::runtime::state::MobCommand::WakeMember {
            agent_identity: woken.clone(),
            cause: dsl::MemberWakeCause::PeerDelivery,
            observed_missing: false,
            input_class: dsl::PeerInputClass::Content,
            sender_waking: false,
            reply_tx,
        })
        .await
        .expect("the peer wake reaches the actor");
    let woken_held = tokio::time::timeout(STEP, probe.wait_for_started(2))
        .await
        .is_ok();
    let own_attempt = mob.wake_attempt_of(&woken);

    // The plan wakes the other members one at a time and reaches the last
    // one while its own wake is still held.
    probe.release(&first);
    let others = &mob.sessions[..MEMBERS - 1];
    let others_live = mob
        .wait_for_state(|state| {
            others.iter().all(|(identity, _)| {
                state
                    .member_materialization
                    .get(&dsl::AgentIdentity::from_domain(identity))
                    == Some(&dsl::MemberMaterializationPhase::Live)
            })
        })
        .await;
    let at_reach = (mob.residency()[&woken], mob.wake_attempt_of(&woken));

    // The held wake's bound expires: the delivery is answered with the wake
    // timeout. Its restore is ended at its start, before it reads anything.
    let delivery = tokio::time::timeout(STEP, answer).await;
    probe.abort(&woken, 1);

    // The plan reaches the member again and applies its input: its second
    // restore starts, held, under a new attempt, and the lift waits for it.
    let replanned = tokio::time::timeout(STEP, probe.wait_for_started(MEMBERS + 1))
        .await
        .is_ok();
    let replanned_attempt = mob.wake_attempt_of(&woken);
    let lift_open = mob
        .handle
        .machine_state_watch_rx
        .borrow()
        .explicit_resume_attempt
        .is_some();
    probe.release(&woken);
    let lifted = mob.join_lift(lift).await;
    mob.settle_live().await;

    assert!(
        first_held && woken_held,
        "the plan's first restore and the delivery's restore are held"
    );
    assert!(
        own_attempt.is_some(),
        "the peer message woke the member under an attempt of its own"
    );
    assert!(
        others_live,
        "the plan woke the other members while the member's own wake was held"
    );
    assert_eq!(
        at_reach,
        (
            (Some(dsl::MemberMaterializationPhase::Waking), true),
            own_attempt
        ),
        "once the plan woke every other member, the member is still Waking under its own wake: the plan opened no wake of its own for it"
    );
    assert!(
        matches!(
            delivery,
            Ok(Ok(Err(MemberWakeFailure::Refused(
                crate::MemberWakeRefusal::WakeTimedOut
            ))))
        ),
        "the delivery's wake timed out: {delivery:?}"
    );
    assert!(
        replanned,
        "the plan applied the member's input again: its second restore started"
    );
    assert!(
        replanned_attempt.is_some() && replanned_attempt != own_attempt,
        "the member woke again under one more attempt: {own_attempt:?}, then {replanned_attempt:?}"
    );
    assert_eq!(
        probe
            .started()
            .iter()
            .filter(|identity| **identity == woken)
            .count(),
        2,
        "the member's restore started twice: the delivery's, then the plan's"
    );
    for (identity, (phase, attempt)) in mob.residency() {
        assert_eq!(
            (phase, attempt),
            (Some(dsl::MemberMaterializationPhase::Live), false),
            "{identity} ends Live"
        );
    }
    assert_eq!(
        mob.wake_attempts() - attempts_before,
        MEMBERS as u64 + 1,
        "one attempt per member, and one more for the member the plan reached again"
    );
    assert!(
        lift_open,
        "the lift waited for the member's second wake (a refused join also ends it early)"
    );
    assert!(lifted.is_ok(), "the lift succeeds: {lifted:?}");
}

/// At restore concurrency 1, over members seated Dormant and never built (so
/// the plan's wakes are the actor's cold wakes, under the wake bound), the
/// plan's wake of the first member times out at
/// its held restore, so the member is Dormant again for a retryable cause,
/// and a later wake of the member (a peer wake, or the owed retry) opens a
/// new attempt, held at its restore, before the lift handles the plan wake's
/// close: that hand-off is held at its test seam until then. MobMachine
/// settles the member's resume work only once no wake attempt of the member
/// is open, so the lift watches the later wake and settles when it closes.
/// The lift answers Ok with every member Live, and a Stop after it is
/// served.
///
/// RED before the fix: the refused settle was dropped, so the member's
/// resume work was never settled and the lift never answered (the row's
/// bound ends the wait); a Stop would have been parked behind it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lift_settles_a_member_whose_plan_wake_a_later_wake_superseded() {
    // Seated members: the plan's wakes are the actor's, under the wake bound
    // (a member the lift rebuilds through the explicit-resume work path holds
    // its attempt until that path settles it).
    let mob =
        StoppedMob::configured_seated("lift-plan-superseded", 1, Some(SUPERSEDED_WAKE_BOUND)).await;
    let member = worker(0);
    let next = worker(1);
    let probe = probe_member_wake_restores_for_test(mob.handle.mob_id(), [member.clone()]);
    // The member's later wake is held at its restore too.
    probe.hold(&member, 2);
    let (handed_off, release_hand_off) =
        crate::runtime::actor::boot_lift::hold_explicit_resume_boot_member_settle_for_test(
            mob.handle.mob_id(),
            &member,
        );
    let lift = mob.lift();

    // The plan's wake of the member is held at its restore until its bound
    // ends it; the lift's hand-off of that close is then held at the seam.
    let plan_held = tokio::time::timeout(STEP, probe.wait_for_started(1))
        .await
        .is_ok();
    let plan_attempt = mob.wake_attempt_of(&member);
    let plan_closed = tokio::time::timeout(STEP, handed_off).await.is_ok();
    // The timed-out restore ends at its start, before it reads anything.
    probe.abort(&member, 1);

    // A later wake opens a new attempt of the member, held at its restore: a
    // peer wake, which joins the owed retry's wake if that one came first.
    let answer = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| crate::runtime::state::MobCommand::WakeMember {
            agent_identity: member.clone(),
            cause: dsl::MemberWakeCause::PeerDelivery,
            observed_missing: false,
            input_class: dsl::PeerInputClass::Content,
            sender_waking: false,
            reply_tx,
        })
        .await
        .expect("the peer wake reaches the actor");
    let later_held = tokio::time::timeout(STEP, probe.wait_for_started(2))
        .await
        .is_ok();
    let later_attempt = mob.wake_attempt_of(&member);

    // The lift handles the plan wake's close while the later wake is open.
    // That handling frees the plan's one slot, so the next member's restore
    // starts once it ran.
    let _ = release_hand_off.send(());
    let handled = tokio::time::timeout(STEP, async {
        while !probe.started().contains(&next) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    let work_pending = mob
        .handle
        .machine_state_watch_rx
        .borrow()
        .explicit_resume_member_work
        .contains_key(&dsl::AgentIdentity::from_domain(&member));

    // The later wake ends Live; the lift settles the member and finishes.
    probe.release(&member);
    let woken = tokio::time::timeout(STEP, answer).await;
    let lifted = mob.join_lift(lift).await;
    mob.settle_live().await;
    let stopped = tokio::time::timeout(STEP, mob.handle.stop()).await;

    assert!(
        plan_held && plan_closed,
        "the plan's wake of the member was held at its restore, and its close reached the hand-off"
    );
    assert!(
        later_held
            && later_attempt.is_some()
            && plan_attempt.is_some()
            && later_attempt != plan_attempt,
        "a later wake of the member opened a new attempt: {plan_attempt:?}, then {later_attempt:?}"
    );
    assert!(
        handled,
        "the lift handled the plan wake's close: the next member's restore started"
    );
    assert!(
        work_pending,
        "the lift holds the member's resume work while the later wake is open"
    );
    assert!(
        matches!(woken, Ok(Ok(Ok(_)))),
        "the later wake ends Live: {woken:?}"
    );
    for (identity, (phase, attempt)) in mob.residency() {
        assert_eq!(
            (phase, attempt),
            (Some(dsl::MemberMaterializationPhase::Live), false),
            "{identity} ends Live"
        );
    }
    assert!(lifted.is_ok(), "the lift succeeds: {lifted:?}");
    assert!(
        matches!(stopped, Ok(Ok(_))),
        "a Stop after the lift is served: {stopped:?}"
    );
}

/// A member the lift rebuilds through the explicit-resume work path is
/// bounded by the mob's wake bound, as a wake is. While its rebuild
/// preparation is held, a caller that joins its attempt is answered
/// `WakeTimedOut` at the bound, and the member is Dormant again, never
/// Broken. The other member of its round is prepared and rebuilt on its
/// own, not behind it, and every other member is Live. The timeout is the
/// member's own outcome: the lift answers Ok. The member's owed retry then
/// wakes it.
///
/// RED before the fix: the attempt had no bound, so the caller waited for
/// good (the row's bound ends the wait), and a round's preparations were
/// handed back together, so the other member of the round stayed Waking
/// behind the held one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rebuilt_member_whose_preparation_hangs_times_out_alone_at_the_wake_bound() {
    let mob = StoppedMob::configured("lift-rebuild-bound", BOUND, Some(REBUILD_WAKE_BOUND)).await;
    let held = worker(0);
    let probe = probe_member_wake_restores_for_test(mob.handle.mob_id(), [held.clone()]);
    let lift = mob.lift();
    let started = tokio::time::timeout(STEP, async {
        while !probe.started().contains(&held) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();

    // The other member of the round is rebuilt while the held one waits.
    let other = dsl::AgentIdentity::from_domain(&worker(1));
    let other_live = mob
        .wait_for_state(|state| {
            state.member_materialization.get(&other) == Some(&dsl::MemberMaterializationPhase::Live)
        })
        .await;
    let joined = tokio::time::timeout(STEP, mob.handle.ensure_member_live(&held)).await;
    let others: Vec<dsl::AgentIdentity> = (1..MEMBERS)
        .map(|index| dsl::AgentIdentity::from_domain(&worker(index)))
        .collect();
    let others_live = mob
        .wait_for_state(|state| {
            others.iter().all(|id| {
                state.member_materialization.get(id) == Some(&dsl::MemberMaterializationPhase::Live)
            })
        })
        .await;
    let lifted = mob.join_lift(lift).await;
    let status = mob
        .handle
        .member_status(&held)
        .await
        .expect("the held member's status");
    // Its stale preparation ends; the owed retry wakes the member.
    probe.release(&held);
    let held_id = dsl::AgentIdentity::from_domain(&held);
    let retried = mob
        .wait_for_state(|state| {
            state.member_materialization.get(&held_id)
                == Some(&dsl::MemberMaterializationPhase::Live)
        })
        .await;

    assert!(started, "the held member's rebuild preparation started");
    assert!(
        other_live,
        "the other member of the round is rebuilt and Live while the held one waits"
    );
    assert!(
        matches!(
            joined,
            Ok(Err(MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::WakeTimedOut,
                ..
            }))
        ),
        "a caller of the held member is answered at the wake bound: {joined:?}"
    );
    assert!(others_live, "every other member is Live");
    assert!(
        lifted.is_ok(),
        "the member's timeout is its own outcome: the lift answers Ok: {lifted:?}"
    );
    assert_ne!(
        status.status,
        crate::runtime::handle::MobMemberStatus::Broken,
        "a timed-out rebuild never breaks the member: {status:?}"
    );
    assert!(retried, "the member's owed retry wakes it");
}
