//! Rotation targets: the rotation a retirement's start row records (so a
//! restarted mob finishes an interrupted rotation itself), the same-session
//! successor (`respawn_member`, and `respawn` of an identity entry's member)
//! that seats the next incarnation on the member's own session in one machine
//! step, the declared role migration and runtime-mode restamp it carries, the
//! edges it keeps across a restart, the refusals it answers typed before
//! anything starts, and the scoped release MobMachine refuses for a stale
//! scope. They run on the combined candidate: the machine, the actor, the
//! journal and the replay together. The queued input handoff of a
//! same-session successor whose session commits each round is a row of the
//! stop fixture (`host_human_input`), which has a runtime whose sessions
//! commit each round; the other branch is a row here, over the mock's
//! runtime, which keeps no persistence store.

use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::dormant_seat::fresh_seat_session_id;
use crate::runtime::{IdentityEntryOrigin, IdentityEntryRead, MemberRespawn, ReleaseOutcome};

const STEP: Duration = Duration::from_mins(1);

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

/// A turn-driven worker spec: it owes no kickoff.
fn worker_spec(name: &str) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", id(name));
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

/// `worker_spec` acting for the identity entry of its name.
fn entry_spec(name: &str) -> SpawnMemberSpec {
    worker_spec(name).with_entry_custody(true)
}

/// A running mob over the mock durable session service and in-memory
/// storage, with session-scoped member comms identities. A restart resumes
/// the same storage.
struct RotationMob {
    keys: tempfile::TempDir,
    storage: MobStorage,
    handle: MobHandle,
    service: Arc<MockSessionService>,
}

impl RotationMob {
    async fn new(label: &str) -> Self {
        Self::new_with(label, |builder| builder).await
    }

    /// [`Self::new`], with `configure` applied to the mob's builder.
    async fn new_with(label: &str, configure: impl FnOnce(MobBuilder) -> MobBuilder) -> Self {
        Self::of_definition(label, sample_definition(), configure).await
    }

    /// A mob of `definition`, with `configure` applied to its builder after
    /// the fixture's own settings (so it can replace them).
    async fn of_definition(
        label: &str,
        definition: MobDefinition,
        configure: impl FnOnce(MobBuilder) -> MobBuilder,
    ) -> Self {
        let keys = tempfile::tempdir().expect("a comms identity root");
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let storage = MobStorage::in_memory();
        let handle = configure(
            MobBuilder::new(with_unique_mob_id(definition, label), storage.clone())
                .with_session_service(service.clone())
                .with_member_comms_identity_root(keys.path()),
        )
        .create()
        .await
        .expect("create the mob");
        Self {
            keys,
            storage,
            handle,
            service,
        }
    }

    /// Crash the mob, restart its session service and resume its storage,
    /// with no host record of anything in flight.
    async fn crash_and_restart(&mut self) {
        self.crash_and_restart_host_pending(&[]).await;
    }

    /// [`Self::crash_and_restart`] under a host that passes `host_pending`
    /// host-pending ([`MobBuilder::with_host_pending_members`]): it keeps its
    /// own record of their preparation.
    async fn crash_and_restart_host_pending(&mut self, host_pending: &[&str]) {
        let host_pending: Vec<AgentIdentity> = host_pending.iter().copied().map(id).collect();
        self.crash_and_restart_with(|builder| builder.with_host_pending_members(host_pending))
            .await;
    }

    /// [`Self::crash_and_restart`], with `configure` applied to the resume's
    /// builder after the fixture's own settings (a mob on network comms
    /// resumes on them).
    async fn crash_and_restart_with(&mut self, configure: impl FnOnce(MobBuilder) -> MobBuilder) {
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        crash_stop_and_release_routes(self.handle.clone()).await;
        self.service = restarted;
        self.handle = tokio::time::timeout(
            STEP,
            Box::pin(
                configure(
                    MobBuilder::for_resume(self.storage.clone())
                        .with_session_service(self.service.clone())
                        .with_member_comms_identity_root(self.keys.path())
                        .notify_orchestrator_on_resume(false),
                )
                .resume(),
            ),
        )
        .await
        .expect("the restart answers")
        .expect("the restart succeeds");
    }

    async fn session(&self, name: &str) -> SessionId {
        self.handle
            .resolve_bridge_session_id(&id(name))
            .await
            .unwrap_or_else(|| panic!("{name} is bound to a session"))
    }

    async fn generation(&self, name: &str) -> Option<Generation> {
        self.handle
            .get_member(&id(name))
            .await
            .expect("roster read")
            .map(|entry| entry.generation)
    }

    /// Wait, within one step, until `name` is seated at `generation`.
    async fn seated_at(&self, name: &str, generation: Generation) {
        let deadline = tokio::time::Instant::now() + STEP;
        while self.generation(name).await != Some(generation) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{name} never reached generation {generation}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn journal(&self) -> Vec<MobEvent> {
        self.storage
            .events
            .poll(0, usize::MAX)
            .await
            .expect("read the journal")
    }
}

/// The rotation `name`'s retirement start row records for `generation`.
fn recorded_rotation(
    journal: &[MobEvent],
    name: &str,
    generation: Generation,
) -> Option<crate::runtime::MemberRotationRecord> {
    journal.iter().find_map(|event| match &event.kind {
        MobEventKind::MemberRetirementStarted {
            agent_identity,
            generation: started,
            ..
        } if agent_identity.as_str() == name && *started == generation => {
            event.kind.retirement_rotation().cloned()
        }
        _ => None,
    })
}

/// The incarnation the row right after `name`'s `MemberRetired` row of
/// `generation` seats, with the session it names: the rotation's terminal
/// batch holds both rows, in that order.
fn successor_in_the_terminal_batch(
    journal: &[MobEvent],
    name: &str,
    generation: Generation,
) -> Option<(Generation, ProfileName, Option<SessionId>)> {
    let retired = journal.iter().position(|event| {
        matches!(
            &event.kind,
            MobEventKind::MemberRetired {
                agent_identity,
                generation: retired,
                ..
            } if agent_identity.as_str() == name && *retired == generation
        )
    })?;
    match &journal.get(retired + 1)?.kind {
        MobEventKind::MemberSpawned(spawned) if spawned.agent_identity.as_str() == name => Some((
            spawned.generation,
            spawned.role.clone(),
            spawned
                .bridge_member_ref
                .as_ref()
                .and_then(MemberRef::bridge_session_id)
                .cloned(),
        )),
        _ => None,
    }
}

/// How many `MemberRetired` rows the journal holds for `name`.
fn retirements_of(journal: &[MobEvent], name: &str) -> usize {
    journal
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::MemberRetired { agent_identity, .. } if agent_identity.as_str() == name
            )
        })
        .count()
}

/// A crash between a rotation's durable start and its terminal step: the
/// start row records the rotation (its target, next session and what its
/// next incarnation is seated with), so the restarted mob records the intent
/// again (`RecoverMemberRotationIntent`) and its own re-drive seats the next
/// incarnation on the name-based session the rotation targeted. No host
/// record and no re-issued call: the identity is rotated, never retired, and
/// the journal holds the retirement only in the batch that seats its
/// successor.
///
/// RED: drop the replay's `RecoverMemberRotationIntent` (builder,
/// `recover_pending_member_retirements`) or the start row's rotation record
/// (`append_retirement_started_event_for_entry`): the restart replays the
/// start as a plain retirement and its re-drive retires the identity, which
/// is then no member at all and never reaches generation 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_between_rotation_start_and_terminal_finishes_the_rotation_with_no_host_record() {
    let mut mob = RotationMob::new("rotation-recorded").await;
    mob.handle
        .spawn_spec(worker_spec("rotated"))
        .await
        .expect("spawn the member");
    let target = fresh_seat_session_id(
        mob.handle.mob_id(),
        mob.handle.incarnation(),
        &id("rotated"),
        Generation::new(1),
    );
    let hold = crate::runtime::actor::retirement_stage_hold::install(
        &id("rotated"),
        "retire-runtime-quiesce",
    );
    let rotating = {
        let handle = mob.handle.clone();
        tokio::spawn(async move { handle.rotate_member(worker_spec("rotated")).await })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the rotation's retirement starts");
    let started = recorded_rotation(&mob.journal().await, "rotated", Generation::new(0))
        .expect("the start row records the rotation");
    assert_eq!(
        (
            started.target(),
            started.session_id(),
            started.next_generation()
        ),
        (
            crate::runtime::RotationTarget::FreshSession,
            &target,
            Generation::new(1)
        )
    );

    mob.crash_and_restart().await;
    hold.release();
    let _ = tokio::time::timeout(STEP, rotating).await;

    mob.seated_at("rotated", Generation::new(1)).await;
    assert_eq!(mob.session("rotated").await, target);
    let journal = mob.journal().await;
    assert_eq!(retirements_of(&journal, "rotated"), 1, "rotated once");
    assert_eq!(
        successor_in_the_terminal_batch(&journal, "rotated", Generation::new(0)),
        Some((
            Generation::new(1),
            ProfileName::from("worker"),
            Some(target)
        )),
        "the retirement is journaled only with its successor"
    );
}

/// `respawn` of an identity entry's member is a same-session successor: one
/// machine step whose `MemberRetired` and `MemberSpawned` rows are one batch
/// naming the member's own session, so the identity is never unseated. While
/// the respawn's retirement is held at the session release, the member is
/// still seated, Retiring, on its kept binding; the successor keeps the
/// entry. A crash while a second entry's respawn is held there is finished
/// by the restarted mob on the same session, from the start row's record.
///
/// RED: route `respawn` of an entry back through the retire-then-spawn
/// helper (drop the entry dispatch in `handle_respawn`): the respawn retires
/// the member and spawns its successor on a new session, the retirement row
/// stands alone, and the hold at the session release is never reached.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respawn_is_one_machine_step_and_never_leaves_the_identity_unseated() {
    let mut mob = RotationMob::new("same-session-respawn").await;
    for name in ["respawned-entry", "crashed-entry"] {
        mob.handle
            .spawn_spec(entry_spec(name))
            .await
            .expect("the host declares a spawned entry");
    }
    let session = mob.session("respawned-entry").await;
    let dsl_identity = dsl::AgentIdentity::from_domain(&id("respawned-entry"));

    let hold = crate::runtime::actor::retirement_stage_hold::install(
        &id("respawned-entry"),
        "retire-session-release",
    );
    let respawning = {
        let handle = mob.handle.clone();
        tokio::spawn(async move { handle.respawn(id("respawned-entry"), None).await })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the respawn's retirement reaches the session release");
    {
        let state = mob.handle.machine_state_watch_rx.borrow();
        assert!(
            state.identity_to_runtime.contains_key(&dsl_identity),
            "the identity stays seated while its successor is pending"
        );
        assert_eq!(
            state.member_session_bindings.get(&dsl_identity),
            Some(&dsl::SessionId::from_domain(&session)),
            "the binding is kept"
        );
        assert_eq!(
            state.member_rotation_targets.get(&dsl_identity),
            Some(&dsl::RotationTarget::SameSession)
        );
    }
    hold.release();
    let receipt = tokio::time::timeout(STEP, respawning)
        .await
        .expect("the respawn answers")
        .expect("the respawn task")
        .expect("the host respawns its entry");
    assert_eq!(receipt.identity, id("respawned-entry"));
    assert_eq!(
        mob.generation("respawned-entry").await,
        Some(Generation::new(1))
    );
    assert_eq!(mob.session("respawned-entry").await, session);
    assert!(
        matches!(
            mob.handle.identity_entry(&id("respawned-entry")),
            Ok(IdentityEntryRead::Entry(view)) if view.origin == IdentityEntryOrigin::Declared
        ),
        "the successor keeps the entry"
    );
    let journal = mob.journal().await;
    assert_eq!(
        successor_in_the_terminal_batch(&journal, "respawned-entry", Generation::new(0)),
        Some((
            Generation::new(1),
            ProfileName::from("worker"),
            Some(session.clone())
        )),
        "one batch seats the successor on the same session"
    );
    assert_eq!(
        recorded_rotation(&journal, "respawned-entry", Generation::new(0))
            .map(|record| record.target()),
        Some(crate::runtime::RotationTarget::SameSession)
    );

    // A crash with the second entry's respawn held at the session release.
    let crashed_session = mob.session("crashed-entry").await;
    let hold = crate::runtime::actor::retirement_stage_hold::install(
        &id("crashed-entry"),
        "retire-session-release",
    );
    let crashed = {
        let handle = mob.handle.clone();
        tokio::spawn(async move { handle.respawn(id("crashed-entry"), None).await })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the second respawn reaches the session release");
    mob.crash_and_restart().await;
    hold.release();
    let _ = tokio::time::timeout(STEP, crashed).await;
    mob.seated_at("crashed-entry", Generation::new(1)).await;
    assert_eq!(mob.session("crashed-entry").await, crashed_session);
    let journal = mob.journal().await;
    assert_eq!(retirements_of(&journal, "crashed-entry"), 1);
    assert_eq!(
        successor_in_the_terminal_batch(&journal, "crashed-entry", Generation::new(0)),
        Some((
            Generation::new(1),
            ProfileName::from("worker"),
            Some(crashed_session)
        ))
    );
}

/// A host's preparation mark outlasts the mob's own same-session terminal:
/// the host wakes first. A crash interrupts the respawn of a member, and the
/// host passes the member host-pending at the restart (a host that cannot
/// read its own records holds every identity this way). The restarted mob
/// finishes the respawn itself (a same-session rotation is the mob's own)
/// and seats the successor Dormant at the next generation on the member's
/// own session, with the mark kept. Every wake of the successor is refused
/// `HostPreparationPending` until the host's own seat step for it: a
/// `seat_dormant_members` of the identity on its kept session answers
/// `AlreadySeated` and ends the mark, after which the successor wakes Live.
///
/// RED: let the same-session terminal end the mark (remove the identity
/// from `member_host_preparation_causes` in
/// `CommitMemberRotationSameSession`): the mark is gone after the restart,
/// and the successor wakes before the host's seat step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_pending_successor_waits_for_the_host_seat_step() {
    let mut mob = RotationMob::new("host-mark-kept").await;
    mob.handle
        .spawn_spec(worker_spec("host-held"))
        .await
        .expect("spawn the member");
    let session = mob.session("host-held").await;
    let dsl_identity = dsl::AgentIdentity::from_domain(&id("host-held"));
    let hold = crate::runtime::actor::retirement_stage_hold::install(
        &id("host-held"),
        "retire-session-release",
    );
    let respawning = {
        let handle = mob.handle.clone();
        tokio::spawn(async move {
            handle
                .respawn_member(id("host-held"), MemberRespawn::new())
                .await
        })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the respawn's retirement reaches the session release");
    mob.crash_and_restart_host_pending(&["host-held"]).await;
    hold.release();
    let _ = tokio::time::timeout(STEP, respawning).await;

    // The restarted mob finished the respawn on the member's own session
    // and kept the host's mark.
    mob.seated_at("host-held", Generation::new(1)).await;
    assert_eq!(mob.session("host-held").await, session);
    assert!(
        mob.handle
            .machine_state_watch_rx
            .borrow()
            .member_host_preparation_causes
            .contains_key(&dsl_identity),
        "the mob's own same-session terminal keeps the host's mark"
    );
    assert_eq!(
        mob.handle.member_materialization_for_test(&id("host-held")),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the successor is seated Dormant"
    );
    let refused = tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id("host-held")))
        .await
        .expect("the wake answers");
    assert!(
        matches!(
            refused,
            Err(MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::HostPreparationPending,
                ..
            })
        ),
        "the successor's wake waits for the host's seat step: {refused:?}"
    );
    assert_eq!(
        mob.handle.member_materialization_for_test(&id("host-held")),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "a refused wake leaves the successor Dormant"
    );

    // The host's seat step: the identity on its kept session is already
    // seated, and the seat ends the mark.
    let mut seat = worker_spec("host-held");
    seat.launch_mode = crate::launch::MemberLaunchMode::Resume {
        bridge_session_id: session.clone(),
        resume_from_role: None,
    };
    let outcomes = tokio::time::timeout(STEP, mob.handle.seat_dormant_members(vec![seat]))
        .await
        .expect("the seat answers")
        .expect("the host's seat step commits");
    assert!(
        matches!(
            outcomes.as_slice(),
            [crate::runtime::DormantSeatOutcome::AlreadySeated { session_id, .. }]
                if session_id == &session
        ),
        "{outcomes:?}"
    );
    assert!(
        !mob.handle
            .machine_state_watch_rx
            .borrow()
            .member_host_preparation_causes
            .contains_key(&dsl_identity),
        "the host's seat step ends the mark"
    );
    tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id("host-held")))
        .await
        .expect("the wake answers")
        .expect("the successor wakes once the host's seat step ended the mark");
    assert_eq!(
        mob.handle.member_materialization_for_test(&id("host-held")),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    assert_eq!(
        mob.generation("host-held").await,
        Some(Generation::new(1)),
        "the seat step seats nothing new"
    );
}

/// A release scoped to a member incarnation that is no longer current is
/// refused by Core: after a same-session successor replaced the incarnation
/// a scope was captured at (same session, newer runtime id and fence),
/// `release_member_at_scope` answers `Stale` and the live successor stays
/// live; the successor's own scope releases it.
///
/// RED: drop the `scope_current` guard of MobMachine's
/// `ClassifyMemberRelease` (and its Stale twin): the stale scope is
/// classified a release and closes the successor's registration, so the
/// successor is Dormant after the stale call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_release_of_a_stale_scope_is_refused_by_core() {
    let mob = RotationMob::new("scoped-release").await;
    mob.handle
        .spawn_spec(worker_spec("scoped"))
        .await
        .expect("spawn the member");
    let stale = mob
        .handle
        .capture_member_delivery_scope(&id("scoped"))
        .await
        .expect("capture the member's scope");
    let receipt = tokio::time::timeout(
        STEP,
        mob.handle
            .respawn_member(id("scoped"), MemberRespawn::new().with_wake(true)),
    )
    .await
    .expect("the respawn answers")
    .expect("the member is respawned and woken");
    assert!(
        receipt.wake_error().is_none(),
        "the respawn's wake succeeded: {:?}",
        receipt.wake_error()
    );
    assert_eq!(
        mob.handle.member_materialization_for_test(&id("scoped")),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    assert_eq!(stale.session_id(), &mob.session("scoped").await);

    assert_eq!(
        mob.handle
            .release_member_at_scope(&stale)
            .await
            .expect("the stale release answers"),
        ReleaseOutcome::Stale
    );
    assert_eq!(
        mob.handle.member_materialization_for_test(&id("scoped")),
        Some(dsl::MemberMaterializationPhase::Live),
        "a stale scope releases nothing"
    );

    let current = mob
        .handle
        .capture_member_delivery_scope(&id("scoped"))
        .await
        .expect("capture the successor's scope");
    assert_eq!(
        mob.handle
            .release_member_at_scope(&current)
            .await
            .expect("the current release answers"),
        ReleaseOutcome::Released
    );
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.handle.member_materialization_for_test(&id("scoped"))
        != Some(dsl::MemberMaterializationPhase::Dormant)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the current scope's release never reached Dormant"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A declared role migration is one same-session step: `respawn_member`
/// naming another profile seats the next incarnation under that profile on
/// the member's own session, its retirement and its successor journaled in
/// one batch, and the successor wakes under the new profile.
///
/// RED: drop `MemberRespawn::profile` from the plan
/// (`plan_member_respawn` seats the member's own role): the successor keeps
/// the previous profile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declared_migration_restamps_on_the_same_session_in_one_step() {
    let mob = RotationMob::new("declared-migration").await;
    mob.handle
        .spawn_spec(worker_spec("migrating"))
        .await
        .expect("spawn the member");
    let session = mob.session("migrating").await;
    let receipt = tokio::time::timeout(
        STEP,
        mob.handle.respawn_member(
            id("migrating"),
            MemberRespawn::new().with_profile("lead").with_wake(true),
        ),
    )
    .await
    .expect("the migration answers")
    .expect("the member migrates");
    assert!(
        receipt.wake_error().is_none(),
        "the migration's wake succeeded: {:?}",
        receipt.wake_error()
    );
    let entry = mob
        .handle
        .get_member(&id("migrating"))
        .await
        .expect("roster read")
        .expect("the member is seated");
    assert_eq!(
        (entry.role.clone(), entry.generation, entry.runtime_mode),
        (
            ProfileName::from("lead"),
            Generation::new(1),
            crate::MobRuntimeMode::TurnDriven
        ),
        "the successor takes the declared role and keeps the member's runtime mode"
    );
    assert_eq!(mob.session("migrating").await, session);
    assert_eq!(
        mob.handle.member_materialization_for_test(&id("migrating")),
        Some(dsl::MemberMaterializationPhase::Live),
        "the successor wakes under the new role"
    );
    assert_eq!(
        successor_in_the_terminal_batch(&mob.journal().await, "migrating", Generation::new(0)),
        Some((Generation::new(1), ProfileName::from("lead"), Some(session))),
        "the retirement and the restamped successor are one batch on the same session"
    );
}

/// The kept topology of `kept-respawned` and `kept-peer`: MobMachine holds
/// the member edge and the external edge, and the roster holds the member
/// edge both ways and the external peer's recorded descriptor.
async fn assert_kept_topology(mob: &RotationMob, external: &TrustedPeerDescriptor, when: &str) {
    let respawned = dsl::AgentIdentity::from_domain(&id("kept-respawned"));
    let edge = dsl::WiringEdge::new(
        respawned.clone(),
        dsl::AgentIdentity::from_domain(&id("kept-peer")),
    );
    {
        let state = mob.handle.machine_state_watch_rx.borrow();
        assert!(
            state.wiring_edges.contains(&edge),
            "MobMachine holds the member edge {when}"
        );
        assert!(
            state
                .external_peer_edges
                .iter()
                .any(|kept| kept.local == respawned
                    && kept.endpoint == dsl::ExternalPeerEndpoint::from(external)),
            "MobMachine holds the external edge {when}"
        );
    }
    for (member, other) in [
        ("kept-respawned", "kept-peer"),
        ("kept-peer", "kept-respawned"),
    ] {
        let entry = mob
            .handle
            .get_member(&id(member))
            .await
            .expect("roster read")
            .expect("the member is seated");
        assert!(
            entry.wired_to.contains(&id(other)),
            "the roster wires {member} to {other} {when}: {:?}",
            entry.wired_to
        );
    }
    let entry = mob
        .handle
        .get_member(&id("kept-respawned"))
        .await
        .expect("roster read")
        .expect("the successor is seated");
    assert_eq!(
        entry
            .external_peer_specs
            .get(&AgentIdentity::from(external.name.as_str()))
            .cloned(),
        Some(external.clone()),
        "the roster keeps the external peer's recorded descriptor {when}"
    );
}

/// A message from `from` to `to` over their edge is carried.
async fn send_over_kept_edge(mob: &RotationMob, from: &str, to: &str) {
    let body = format!("a message from {from} to {to}");
    let receipt = tokio::time::timeout(
        STEP,
        mob.handle.send_peer_message(
            id(from),
            id(to),
            body.as_str(),
            meerkat_core::types::HandlingMode::Queue,
        ),
    )
    .await
    .expect("the send answers")
    .expect("the edge carries the message");
    assert_eq!(receipt.to, id(to));
}

/// A same-session successor keeps its edges across restarts. The terminal
/// batch records the member's wiring edges and external peer edges again,
/// right after its successor's row: one batch, at consecutive cursors,
/// `MemberRetired`, `MemberSpawned` on the same session, one
/// `MembersWiredBatch` with the member edge and one `ExternalPeerWired`. So
/// the replay, which ends the retired incarnation's edges with its
/// `MemberRetired` row, restores exactly the topology the terminal kept.
/// Before the restart and after it, MobMachine holds both edges and the
/// roster holds the member edge both ways and the external peer's
/// descriptor. After the restart the peer, woken alone, messages the
/// Dormant successor at its durable endpoint, and the successor, once
/// woken, messages the peer back. A second restart, which replays rows that
/// were themselves replayed, keeps the same topology. (A peer message over
/// the kept edge waking the Dormant successor and reaching its session is a
/// row of the wake fixture, whose comms deliver: this mock's do not.)
///
/// RED: drop the kept-topology rows from the same-session terminal's batch
/// (`kept_topology_rows` in `commit_member_rotation`): the batch ends with
/// the successor's row, after the restart neither edge is held, and a
/// message between the two members is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_same_session_successor_keeps_its_edges_across_a_restart() {
    let mut mob = RotationMob::new("kept-edges").await;
    for name in ["kept-respawned", "kept-peer"] {
        mob.handle
            .spawn_spec(worker_spec(name))
            .await
            .expect("spawn the member");
    }
    mob.handle
        .wire(id("kept-respawned"), id("kept-peer"))
        .await
        .expect("wire the members");
    let external = test_trusted_peer_descriptor(
        "remote-mob/worker/kept-external",
        "inproc://remote-mob/worker/kept-external",
    );
    mob.handle
        .wire(id("kept-respawned"), PeerTarget::External(external.clone()))
        .await
        .expect("wire the external peer");
    let session = mob.session("kept-respawned").await;
    tokio::time::timeout(
        STEP,
        mob.handle
            .respawn_member(id("kept-respawned"), MemberRespawn::new()),
    )
    .await
    .expect("the respawn answers")
    .expect("the member is respawned on its own session");

    // The terminal batch, at consecutive cursors.
    let journal = mob.journal().await;
    let retired = journal
        .iter()
        .position(|event| {
            matches!(
                &event.kind,
                MobEventKind::MemberRetired {
                    agent_identity,
                    generation,
                    ..
                } if agent_identity.as_str() == "kept-respawned"
                    && *generation == Generation::new(0)
            )
        })
        .expect("the retirement's row");
    let batch = journal
        .get(retired..retired + 4)
        .expect("the terminal batch holds four rows");
    assert!(
        batch
            .windows(2)
            .all(|pair| pair[1].cursor == pair[0].cursor + 1),
        "the terminal batch is one append, at consecutive cursors: {batch:?}"
    );
    assert!(
        matches!(
            &batch[1].kind,
            MobEventKind::MemberSpawned(spawned)
                if spawned.agent_identity.as_str() == "kept-respawned"
                    && spawned.generation == Generation::new(1)
                    && spawned
                        .bridge_member_ref
                        .as_ref()
                        .and_then(MemberRef::bridge_session_id)
                        == Some(&session)
        ),
        "the successor's row follows on the same session: {batch:?}"
    );
    assert!(
        matches!(
            &batch[2].kind,
            MobEventKind::MembersWiredBatch { edges }
                if *edges
                    == [crate::event::MemberWireEdge {
                        a: id("kept-peer"),
                        b: id("kept-respawned"),
                    }]
        ),
        "the member edge is recorded again: {batch:?}"
    );
    assert!(
        matches!(
            &batch[3].kind,
            MobEventKind::ExternalPeerWired { local, spec }
                if local.as_str() == "kept-respawned" && spec == &external
        ),
        "the external edge is recorded again: {batch:?}"
    );
    assert_kept_topology(&mob, &external, "before the restart").await;

    mob.crash_and_restart().await;
    assert_kept_topology(&mob, &external, "after the restart").await;
    let entry = mob
        .handle
        .get_member(&id("kept-respawned"))
        .await
        .expect("roster read")
        .expect("the successor is seated");
    assert_eq!(entry.generation, Generation::new(1));
    assert_eq!(mob.session("kept-respawned").await, session);

    // The peer, woken alone, messages the Dormant successor; the successor,
    // once woken, messages it back.
    tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id("kept-peer")))
        .await
        .expect("the wake answers")
        .expect("the peer wakes");
    assert_eq!(
        mob.handle
            .member_materialization_for_test(&id("kept-respawned")),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the successor is still Dormant"
    );
    send_over_kept_edge(&mob, "kept-peer", "kept-respawned").await;
    tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id("kept-respawned")))
        .await
        .expect("the wake answers")
        .expect("the successor wakes");
    send_over_kept_edge(&mob, "kept-respawned", "kept-peer").await;

    // A second restart replays the rows the first replay kept.
    mob.crash_and_restart().await;
    assert_kept_topology(&mob, &external, "after a second restart").await;
}

/// A declared role migration whose successor no build created yet survives a
/// restart: `respawn_member` naming another profile, with no wake, seats the
/// next incarnation Dormant under that profile on the member's own session,
/// and its row records the role the session was last built under. After a
/// restart the successor's first wake declares that role and comes up Live
/// under the new profile on the same session.
///
/// RED: drop the replay of the successor's declared predecessor role (the
/// roster's `resume_from_roles`, which the restore reads): the first build
/// after the restart refuses the undeclared role change
/// (`MemberRoleMigrationRequired`) and the wake fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declared_migration_survives_a_restart_before_its_first_build() {
    let mut mob = RotationMob::new("migration-restart").await;
    mob.handle
        .spawn_spec(worker_spec("migrated-late"))
        .await
        .expect("spawn the member");
    let session = mob.session("migrated-late").await;
    tokio::time::timeout(
        STEP,
        mob.handle.respawn_member(
            id("migrated-late"),
            MemberRespawn::new().with_profile("lead"),
        ),
    )
    .await
    .expect("the migration answers")
    .expect("the member migrates");
    assert_eq!(
        mob.handle
            .member_materialization_for_test(&id("migrated-late")),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the successor is seated Dormant, no build created yet"
    );

    mob.crash_and_restart().await;
    tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id("migrated-late")))
        .await
        .expect("the wake answers")
        .expect("the successor's first build declares the migration");
    let entry = mob
        .handle
        .get_member(&id("migrated-late"))
        .await
        .expect("roster read")
        .expect("the successor is seated");
    assert_eq!(
        (entry.role.clone(), entry.generation),
        (ProfileName::from("lead"), Generation::new(1))
    );
    assert_eq!(mob.session("migrated-late").await, session);
    assert_eq!(
        mob.handle
            .member_materialization_for_test(&id("migrated-late")),
        Some(dsl::MemberMaterializationPhase::Live),
        "the successor wakes under the new role after the restart"
    );
}

/// The sample definition with the roles of a role move under declared
/// wiring: `member`, wired to `helper`, and `steward`, wired to `auditor`
/// (`role_wiring`), each a worker otherwise.
fn role_move_definition() -> MobDefinition {
    let mut definition = sample_definition();
    let worker = definition
        .profiles
        .get(&ProfileName::from("worker"))
        .cloned()
        .expect("the worker profile");
    for role in ["member", "helper", "steward", "auditor"] {
        definition
            .profiles
            .insert(ProfileName::from(role), worker.clone());
    }
    definition.wiring.role_wiring = vec![
        RoleWiringRule {
            a: ProfileName::from("member"),
            b: ProfileName::from("helper"),
        },
        RoleWiringRule {
            a: ProfileName::from("steward"),
            b: ProfileName::from("auditor"),
        },
    ];
    definition
}

/// A turn-driven spec of `name` under `role`: it owes no kickoff.
fn role_spec(role: &str, name: &str) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new(role, id(name));
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

/// Whether MobMachine holds the member edge between `a` and `b`.
fn edge_held(mob: &RotationMob, a: &str, b: &str) -> bool {
    let edge = dsl::WiringEdge::new(
        dsl::AgentIdentity::from_domain(&id(a)),
        dsl::AgentIdentity::from_domain(&id(b)),
    );
    mob.handle
        .machine_state_watch_rx
        .borrow()
        .wiring_edges
        .contains(&edge)
}

/// Whether a member wiring row after `name`'s `MemberRetired` row of
/// `generation` joins `name` and `peer`.
fn wired_after_retirement(
    journal: &[MobEvent],
    name: &str,
    generation: Generation,
    peer: &str,
) -> bool {
    let Some(retired) = journal.iter().position(|event| {
        matches!(
            &event.kind,
            MobEventKind::MemberRetired {
                agent_identity,
                generation: retired,
                ..
            } if agent_identity.as_str() == name && *retired == generation
        )
    }) else {
        return false;
    };
    let joins = |a: &AgentIdentity, b: &AgentIdentity| {
        (a.as_str() == name && b.as_str() == peer) || (a.as_str() == peer && b.as_str() == name)
    };
    journal[retired..].iter().any(|event| match &event.kind {
        MobEventKind::MembersWired { a, b } => joins(a, b),
        MobEventKind::MembersWiredBatch { edges } => {
            edges.iter().any(|edge| joins(&edge.a, &edge.b))
        }
        _ => false,
    })
}

/// The name under which `holder`'s comms runtime trusts the endpoint
/// MobMachine records for `peer`; `None` while either is missing.
async fn trusted_name(mob: &RotationMob, holder: &str, peer: &str) -> Option<String> {
    let peer_id = mob
        .handle
        .machine_state_watch_rx
        .borrow()
        .member_peer_ids
        .get(&dsl::AgentIdentity::from_domain(&id(peer)))
        .map(|peer_id| peer_id.0.clone())?;
    let peer_id = meerkat_core::comms::PeerId::parse(&peer_id).ok()?;
    let session = mob.handle.resolve_bridge_session_id(&id(holder)).await?;
    let comms = mob.service.comms_runtime(&session).await?;
    comms
        .peers()
        .await
        .into_iter()
        .find(|entry| entry.peer_id == peer_id)
        .map(|entry| entry.name.as_str().to_string())
}

/// Wait, within one step, until `holder` trusts the endpoint MobMachine
/// records for `peer`, and return the name it trusts it under.
async fn trusts_under(mob: &RotationMob, holder: &str, peer: &str) -> String {
    let deadline = tokio::time::Instant::now() + STEP;
    loop {
        if let Some(name) = trusted_name(mob, holder, peer).await {
            return name;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{holder} never trusted the endpoint recorded for {peer}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Wait, within one step, until `name`'s retirement settles stuck at the
/// kept session's release.
async fn stuck_at_session_release(mob: &RotationMob, name: &str) {
    let deadline = tokio::time::Instant::now() + STEP;
    loop {
        let settlement = mob
            .handle
            .retirement_settlement(&id(name))
            .map(|watch| watch.current());
        if matches!(
            &settlement,
            Some(crate::runtime::RetirementSettlement::Stuck { stage, .. })
                if stage.as_str() == "retire-session-release"
        ) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{name}'s retirement never settled stuck at the session release: {settlement:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// MobMachine holds `name`'s same-session move to `role`, stuck: the role
/// it started with is pinned, and no endpoint is recorded (on network comms
/// the successor's first wake registers one).
fn assert_started_role(mob: &RotationMob, name: &str, role: &str) {
    let dsl_identity = dsl::AgentIdentity::from_domain(&id(name));
    let state = mob.handle.machine_state_watch_rx.borrow();
    assert_eq!(
        state.member_rotation_targets.get(&dsl_identity),
        Some(&dsl::RotationTarget::SameSession),
        "{name}'s same-session rotation stands"
    );
    assert_eq!(
        state
            .member_rotation_profile_names
            .get(&dsl_identity)
            .map(String::as_str),
        Some(role),
        "MobMachine pins the role {name}'s move started with"
    );
    assert!(
        !state
            .member_rotation_peer_endpoints
            .contains_key(&dsl_identity),
        "on network comms {name}'s move records no endpoint"
    );
}

/// A re-issue of `name`'s stuck move to `steward` under any other role (its
/// own, implicitly and by name, or a third one) is refused retryable before
/// anything runs, naming the started role: no row is written, the role stays
/// pinned and the move stays stuck.
async fn assert_reissues_refused(mob: &RotationMob, name: &str) {
    let journal_before = mob.journal().await.len();
    for request in [
        MemberRespawn::new(),
        MemberRespawn::new().with_profile("member"),
        MemberRespawn::new().with_profile("helper"),
    ] {
        let refused = tokio::time::timeout(STEP, mob.handle.respawn_member(id(name), request))
            .await
            .expect("the re-issue answers");
        assert!(
            matches!(
                &refused,
                Err(MobError::LifecycleOperationPending { intent })
                    if intent.contains("under role steward")
            ),
            "a re-issue of {name}'s move under another role is refused: {refused:?}"
        );
    }
    assert_eq!(
        mob.journal().await.len(),
        journal_before,
        "no row was written"
    );
    assert_started_role(mob, name, "steward");
    assert!(
        matches!(
            mob.handle
                .retirement_settlement(&id(name))
                .map(|watch| watch.current()),
            Some(crate::runtime::RetirementSettlement::Stuck { stage, .. })
                if stage.as_str() == "retire-session-release"
        ),
        "{name}'s move stays stuck"
    );
}

/// `name`'s move to `steward` finished on `session`: one batch seats the
/// successor at generation 1 under `steward` on the same session and wires
/// its declared edge to the auditor; MobMachine holds no rotation of it, and
/// holds that edge but not its own role's edge to the helper. Once both ends
/// are woken (the successor's first wake registers its endpoint under the
/// new role's comms name), the auditor trusts it under that name, MobMachine
/// records the endpoint under it, and a message crosses the edge both ways.
/// A message over the ended edge is refused.
async fn assert_moved(mob: &RotationMob, name: &str, session: &SessionId) {
    let journal = mob.journal().await;
    assert_eq!(
        successor_in_the_terminal_batch(&journal, name, Generation::new(0)),
        Some((
            Generation::new(1),
            ProfileName::from("steward"),
            Some(session.clone())
        )),
        "one batch seats {name} under steward on its own session"
    );
    assert!(
        wired_after_retirement(&journal, name, Generation::new(0), "mover-auditor"),
        "the move journals {name}'s declared edge to the auditor"
    );
    assert_eq!(&mob.session(name).await, session);
    let dsl_identity = dsl::AgentIdentity::from_domain(&id(name));
    {
        let state = mob.handle.machine_state_watch_rx.borrow();
        assert_eq!(
            state
                .member_profile_names
                .get(&dsl_identity)
                .map(String::as_str),
            Some("steward"),
            "MobMachine seats {name} under the started role"
        );
        assert!(
            !state.member_rotation_targets.contains_key(&dsl_identity)
                && !state
                    .member_rotation_profile_names
                    .contains_key(&dsl_identity),
            "MobMachine holds no rotation of {name}"
        );
    }
    assert!(
        edge_held(mob, name, "mover-auditor"),
        "{name} is wired to the auditor"
    );
    assert!(
        !edge_held(mob, name, "mover-helper"),
        "{name}'s own role's edge to the helper ended"
    );
    let wired_to = mob
        .handle
        .get_member(&id(name))
        .await
        .expect("roster read")
        .expect("the successor is seated")
        .wired_to;
    assert!(
        wired_to.contains(&id("mover-auditor")) && !wired_to.contains(&id("mover-helper")),
        "the roster wires {name} to the auditor only: {wired_to:?}"
    );

    let new_name = crate::runtime::actor::render_member_comms_name(
        mob.handle.mob_id().as_str(),
        "steward",
        name,
    )
    .expect("a member comms name");
    for member in [name, "mover-auditor"] {
        tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id(member)))
            .await
            .expect("the wake answers")
            .expect("the member wakes");
    }
    assert_eq!(
        trusts_under(mob, "mover-auditor", name).await,
        new_name,
        "the auditor trusts {name} under the new role's comms name"
    );
    trusts_under(mob, name, "mover-auditor").await;
    assert_eq!(
        mob.handle
            .machine_state_watch_rx
            .borrow()
            .member_peer_endpoints
            .get(&dsl_identity)
            .map(|endpoint| endpoint.name.0.clone()),
        Some(new_name),
        "MobMachine records {name}'s endpoint under the new role's comms name"
    );
    send_over_kept_edge(mob, "mover-auditor", name).await;
    send_over_kept_edge(mob, name, "mover-auditor").await;
    let refused = mob
        .handle
        .send_peer_message(
            id("mover-helper"),
            id(name),
            "over the ended edge",
            meerkat_core::types::HandlingMode::Queue,
        )
        .await;
    assert!(
        matches!(&refused, Err(MobError::WiringError(_))),
        "the ended edge carries nothing: {refused:?}"
    );
}

/// A role move that got stuck is finished only under the role it started
/// with, on network comms, with peers and edges, and after a restart too.
/// The definition wires the member's own role (`member`) to a helper and
/// the new one (`steward`) to an auditor. A `respawn_member` naming
/// `steward` records no endpoint (the successor's first wake registers one)
/// and MobMachine pins the started role; the kept session's release fails
/// once, so the move settles stuck at that stage. While it stands, a
/// re-issue under the member's own role, or a third one, is refused
/// retryable, naming the started role, and writes nothing. A re-issue under
/// `steward` finishes it: the successor is seated under `steward` on the same
/// session, the helper's edge has ended, the auditor's declared edge is wired
/// and journaled, and once the successor's first wake registers its endpoint
/// under the new role's comms name, the auditor trusts that name and a
/// message crosses the edge both ways. A second member's move, stuck the same
/// way before a crash, is recorded again by the restarted mob with its
/// started role pinned by the replay; the restarted mob's own re-drive sticks
/// at the same stage, and the same refusals and the same finish follow.
///
/// RED: restore the endpoint comparison in `retirement_reply_under_rotation`
/// (a re-issue of a same-session rotation that recorded no endpoint passes
/// whatever role it names): the own-role re-issue passes the re-issue check
/// and re-drives the stuck move, so it is not refused naming the started
/// role. Drop the started role from the replay arm
/// (`RecoverMemberRotationIntentSameSession`): the replay records the second
/// member's stuck move without it, which the invariant that a same-session
/// rotation holds its started role refuses, and the restart fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stuck_role_move_is_finished_only_under_the_role_it_started_with() {
    use crate::runtime::actor::retirement_stage_test_hooks as stages;

    let mut mob = RotationMob::of_definition(
        "role-move-network",
        role_move_definition(),
        MobBuilder::with_network_member_comms,
    )
    .await;
    for (role, name) in [
        ("helper", "mover-helper"),
        ("auditor", "mover-auditor"),
        ("member", "mover"),
        ("member", "mover-late"),
    ] {
        mob.handle
            .spawn_spec(role_spec(role, name))
            .await
            .expect("spawn the member");
    }
    for name in ["mover", "mover-late"] {
        assert!(
            edge_held(&mob, name, "mover-helper"),
            "{name}'s own role wires it to the helper"
        );
    }

    // The move, stuck at the kept session's release.
    let session = mob.session("mover").await;
    stages::fail_next_kept_session_release(&session);
    let stuck = tokio::time::timeout(
        STEP,
        mob.handle
            .respawn_member(id("mover"), MemberRespawn::new().with_profile("steward")),
    )
    .await
    .expect("the move answers");
    assert!(
        matches!(
            &stuck,
            Err(MobError::MemberRetirementStuck { stage, .. }) if stage == "retire-session-release"
        ),
        "the move is stuck at the kept session's release: {stuck:?}"
    );
    assert_started_role(&mob, "mover", "steward");
    assert_reissues_refused(&mob, "mover").await;
    let receipt = tokio::time::timeout(
        STEP,
        mob.handle
            .respawn_member(id("mover"), MemberRespawn::new().with_profile("steward")),
    )
    .await
    .expect("the re-issue answers")
    .expect("a re-issue under the started role finishes the move");
    assert_eq!(receipt.identity, id("mover"));
    assert_moved(&mob, "mover", &session).await;

    // The same across a restart: the second move is stuck before the crash,
    // and the restarted mob's own re-drive sticks at the same stage.
    let late_session = mob.session("mover-late").await;
    stages::fail_next_kept_session_release(&late_session);
    let stuck = tokio::time::timeout(
        STEP,
        mob.handle.respawn_member(
            id("mover-late"),
            MemberRespawn::new().with_profile("steward"),
        ),
    )
    .await
    .expect("the move answers");
    assert!(
        matches!(
            &stuck,
            Err(MobError::MemberRetirementStuck { stage, .. }) if stage == "retire-session-release"
        ),
        "the second move is stuck at the kept session's release: {stuck:?}"
    );
    stages::fail_next_kept_session_release(&late_session);
    mob.crash_and_restart_with(MobBuilder::with_network_member_comms)
        .await;
    stuck_at_session_release(&mob, "mover-late").await;
    assert!(
        !stages::kept_session_release_failure_armed(&late_session),
        "the restarted mob's own re-drive reached the kept session's release"
    );
    assert_started_role(&mob, "mover-late", "steward");
    assert_reissues_refused(&mob, "mover-late").await;
    let receipt = tokio::time::timeout(
        STEP,
        mob.handle.respawn_member(
            id("mover-late"),
            MemberRespawn::new().with_profile("steward"),
        ),
    )
    .await
    .expect("the re-issue answers")
    .expect("a re-issue under the started role finishes the move after the restart");
    assert_eq!(receipt.identity, id("mover-late"));
    assert_moved(&mob, "mover-late", &late_session).await;
}

/// A runtime-mode restamp is one same-session step: `respawn_member` naming
/// AutonomousHost seats the next incarnation as an autonomous host on the
/// member's own session, and the successor's row, in the terminal batch,
/// records the new mode for replay.
///
/// RED: drop `MemberRespawn::runtime_mode` from the plan
/// (`plan_member_respawn` seats the member's own mode): the successor stays
/// TurnDriven.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_mode_restamp_is_one_same_session_step() {
    let mob = RotationMob::new("mode-restamp").await;
    mob.handle
        .spawn_spec(worker_spec("restamped"))
        .await
        .expect("spawn the member");
    let session = mob.session("restamped").await;
    tokio::time::timeout(
        STEP,
        mob.handle.respawn_member(
            id("restamped"),
            MemberRespawn::new().with_runtime_mode(crate::MobRuntimeMode::AutonomousHost),
        ),
    )
    .await
    .expect("the restamp answers")
    .expect("the member is restamped");
    let entry = mob
        .handle
        .get_member(&id("restamped"))
        .await
        .expect("roster read")
        .expect("the successor is seated");
    assert_eq!(
        (entry.generation, entry.runtime_mode),
        (Generation::new(1), crate::MobRuntimeMode::AutonomousHost)
    );
    assert_eq!(mob.session("restamped").await, session);
    let journal = mob.journal().await;
    assert_eq!(
        successor_in_the_terminal_batch(&journal, "restamped", Generation::new(0)),
        Some((
            Generation::new(1),
            ProfileName::from("worker"),
            Some(session)
        ))
    );
    let recorded = journal.iter().find_map(|event| match &event.kind {
        MobEventKind::MemberSpawned(spawned)
            if spawned.agent_identity.as_str() == "restamped"
                && spawned.generation == Generation::new(1) =>
        {
            Some(spawned.runtime_mode)
        }
        _ => None,
    });
    assert_eq!(recorded, Some(crate::MobRuntimeMode::AutonomousHost));
}

/// The queued-input handoff's other branch: a member whose session does not
/// commit each round (its runtime keeps no persistence store, as this
/// mock's does, which the row checks first) ends every input queued behind
/// its running turn when a same-session successor releases its runtime. The
/// queued Queue-mode input's waiter is answered that its runtime terminated
/// on the member's session, never that the input was kept for the next
/// incarnation. The terminal batch seats the successor on the same session,
/// Dormant (the handover owed it no wake), and once woken the successor
/// never runs the ended input.
///
/// RED: release the kept session with `UnregisterQueuedInputs::Requeue`
/// whatever the session's commit mode (`retirement_release_kept_session`):
/// the waiter is told the input was requeued across the stop, and the
/// successor runs it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_session_successor_ends_queued_inputs_when_its_session_does_not_commit_each_round() {
    const QUEUED: &str = "queued behind the running turn";
    let mob = RotationMob::new("same-session-stop-branch").await;
    assert!(
        !mob.service
            .enable_runtime_adapter()
            .has_runtime_persistence(),
        "the branch's precondition: the runtime keeps no persistence store"
    );
    let identity = id("stop-branch");
    let mut spec = worker_spec("stop-branch");
    spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
    mob.handle
        .spawn_spec(spec)
        .await
        .expect("spawn the autonomous member");
    let session = mob.session("stop-branch").await;
    // The member's kickoff turn is running: the runtime has its run current.
    tokio::time::timeout(STEP, mob.service.wait_keep_alive_turn_entered(&session))
        .await
        .expect("the kickoff turn starts");
    let entry = mob
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member is seated");
    // An autonomous member's tracked work takes the exact runtime-input
    // lane: its inbox delivery tracks no completion.
    let queued = mob
        .handle
        .start_runtime_work_with_delivery_identity_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            WorkSpec::new(QUEUED, WorkOrigin::Internal),
            MobDeliveryIdentity::new("stop-branch-queued", Uuid::new_v4().to_string())
                .expect("a stable delivery identity"),
            BoundedResultSpec::new("queued", 1024).expect("bound"),
            crate::mob_machine::WorkContentAttribution::Conversational,
        )
        .await
        .expect("the queued input is admitted");
    let receipt = tokio::time::timeout(
        STEP,
        mob.handle
            .respawn_member(identity.clone(), MemberRespawn::new()),
    )
    .await
    .expect("the respawn answers")
    .expect("the member is respawned on its own session");
    let answer = tokio::time::timeout(
        STEP,
        queued.wait_bounded(BoundedResultSpec::new("queued", 1024).expect("bound")),
    )
    .await
    .expect("the queued input's waiter is answered");
    match answer {
        Ok(_) => panic!("the queued input ran on the released runtime"),
        Err(error) => assert!(
            matches!(
                error.failure(),
                BoundedTurnFailure::RuntimeTerminated { session_id, .. } if session_id == &session
            ),
            "a session that does not commit each round ends its queued input with its runtime: {error:?}"
        ),
    }
    assert_eq!(receipt.identity, identity);
    assert_eq!(
        mob.generation("stop-branch").await,
        Some(Generation::new(1))
    );
    assert_eq!(mob.session("stop-branch").await, session);
    assert_eq!(
        successor_in_the_terminal_batch(&mob.journal().await, "stop-branch", Generation::new(0)),
        Some((
            Generation::new(1),
            ProfileName::from("worker"),
            Some(session.clone())
        )),
        "the terminal batch seats the successor on the same session"
    );
    assert_eq!(
        mob.handle.member_materialization_for_test(&identity),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the handover owed the successor no wake"
    );
    tokio::time::timeout(STEP, mob.handle.ensure_member_live(&identity))
        .await
        .expect("the wake answers")
        .expect("the successor wakes");
    assert!(
        !mob.service
            .recorded_start_turn_prompts()
            .await
            .iter()
            .any(|(turn_session, prompt)| turn_session == &session && prompt.contains(QUEUED)),
        "the successor never runs the ended input"
    );
}

/// The rows a refused respawn leaves: the member keeps its generation and
/// no retirement of it started.
async fn assert_nothing_started(handle: &MobHandle, storage: &MobStorage, name: &str) {
    assert_eq!(
        handle
            .get_member(&id(name))
            .await
            .expect("roster read")
            .map(|entry| entry.generation),
        Some(Generation::new(0)),
        "{name} keeps its incarnation"
    );
    let journal = storage
        .events
        .poll(0, usize::MAX)
        .await
        .expect("read the journal");
    assert!(
        !journal.iter().any(|event| matches!(
            &event.kind,
            MobEventKind::MemberRetirementStarted { agent_identity, .. }
                if agent_identity.as_str() == name
        )),
        "no retirement of {name} started"
    );
}

/// `name`'s roster anchor: its generation, fence and session.
async fn roster_anchor(
    handle: &MobHandle,
    name: &str,
) -> (Generation, crate::ids::FenceToken, Option<SessionId>) {
    let entry = handle
        .get_member(&id(name))
        .await
        .expect("roster read")
        .expect("the member is seated");
    (
        entry.generation,
        entry.fence_token,
        entry.bridge_session_id().cloned(),
    )
}

/// A refused respawn of each named member left nothing behind: the journal
/// holds no new row, MobMachine holds no rotation of the member, no
/// retirement of it was published, and its roster anchor is unchanged.
async fn assert_nothing_happened(
    handle: &MobHandle,
    storage: &MobStorage,
    journal_before: usize,
    anchors_before: &[(
        &str,
        (Generation, crate::ids::FenceToken, Option<SessionId>),
    )],
) {
    let journal = storage
        .events
        .poll(0, usize::MAX)
        .await
        .expect("read the journal");
    assert_eq!(journal.len(), journal_before, "no row was written");
    for (name, before) in anchors_before {
        let dsl_identity = dsl::AgentIdentity::from_domain(&id(name));
        {
            let state = handle.machine_state_watch_rx.borrow();
            assert!(
                !state.member_rotation_targets.contains_key(&dsl_identity)
                    && !state
                        .member_rotation_session_ids
                        .contains_key(&dsl_identity),
                "MobMachine holds no rotation of {name}"
            );
        }
        assert!(
            handle.retirement_settlement(&id(name)).is_none(),
            "no retirement of {name} was published"
        );
        assert_eq!(
            &roster_anchor(handle, name).await,
            before,
            "{name}'s roster anchor is unchanged"
        );
    }
}

/// `respawn_member` of a member that is not runtime-backed (its mob has no
/// runtime) is refused typed before anything starts, and so is `respawn` of
/// an identity entry's member in the same mob: MobMachine refuses a
/// fresh-life respawn of an entry, so it is routed to the same-session path,
/// which refuses it the same way. Nothing happens: the journal holds no new
/// row, MobMachine holds no rotation of either member, no retirement of
/// either is published, and both roster anchors are unchanged. The mob's
/// definition holds no AutonomousHost profile: the builder refuses one in a
/// mob with no runtime. Both members are seated Dormant, which builds
/// nothing: a spawn's provision needs the runtime this mob does not have.
///
/// RED: drop the runtime-backed check from `plan_member_respawn`: the
/// respawn starts the member's retirement.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respawn_member_of_a_member_that_is_not_runtime_backed_is_refused_typed() {
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let keys = tempfile::tempdir().expect("a comms identity root");
    let mut definition = with_unique_mob_id(sample_definition(), "respawn-no-runtime");
    for profile in definition
        .profiles
        .values_mut()
        .filter_map(ProfileBinding::as_inline_mut)
    {
        profile.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    }
    let handle = MobBuilder::new(definition, storage.clone())
        .with_session_service(service)
        .with_member_comms_identity_root(keys.path())
        .create()
        .await
        .expect("create the mob");
    let seated = tokio::time::timeout(
        STEP,
        handle.seat_dormant_members(vec![
            worker_spec("no-runtime"),
            entry_spec("no-runtime-entry"),
        ]),
    )
    .await
    .expect("the seat answers")
    .expect("the seat batch commits");
    assert!(
        seated
            .iter()
            .all(|outcome| matches!(outcome, crate::runtime::DormantSeatOutcome::Seated { .. })),
        "the member and the host's entry are seated: {seated:?}"
    );
    let journal_before = storage
        .events
        .poll(0, usize::MAX)
        .await
        .expect("read the journal")
        .len();
    let anchors_before = [
        ("no-runtime", roster_anchor(&handle, "no-runtime").await),
        (
            "no-runtime-entry",
            roster_anchor(&handle, "no-runtime-entry").await,
        ),
    ];
    let refused = tokio::time::timeout(
        STEP,
        handle.respawn_member(id("no-runtime"), MemberRespawn::new()),
    )
    .await
    .expect("the respawn answers");
    assert!(
        matches!(
            &refused,
            Err(MobError::UnsupportedForMode { reason, .. }) if reason.contains("runtime-backed")
        ),
        "{refused:?}"
    );
    let refused_entry = tokio::time::timeout(STEP, handle.respawn(id("no-runtime-entry"), None))
        .await
        .expect("the respawn answers");
    assert!(
        matches!(
            &refused_entry,
            Err(crate::runtime::handle::MobRespawnError::Mob(
                MobError::UnsupportedForMode { reason, .. }
            )) if reason.contains("runtime-backed")
        ),
        "the entry is routed to the same-session path: {refused_entry:?}"
    );
    assert_nothing_happened(&handle, &storage, journal_before, &anchors_before).await;
}

/// `respawn_member` on a service that keeps no durable sessions is refused
/// typed before anything starts: the member's session would end with its
/// runtime.
///
/// RED: drop the durability check from `plan_member_respawn`: the respawn
/// starts the member's retirement.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respawn_member_on_a_service_without_durable_sessions_is_refused_typed() {
    let service = Arc::new(MockSessionService::new());
    service.keep_no_durable_sessions();
    let _adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(
        with_unique_mob_id(sample_definition(), "respawn-non-durable"),
        storage.clone(),
    )
    .with_session_service(service)
    .allow_ephemeral_sessions(true)
    .create()
    .await
    .expect("create the mob");
    handle
        .spawn_spec(worker_spec("non-durable"))
        .await
        .expect("spawn the member");
    let refused = tokio::time::timeout(
        STEP,
        handle.respawn_member(id("non-durable"), MemberRespawn::new()),
    )
    .await
    .expect("the respawn answers");
    assert!(
        matches!(
            &refused,
            Err(MobError::UnsupportedForMode { reason, .. }) if reason.contains("durable sessions")
        ),
        "{refused:?}"
    );
    assert_nothing_started(&handle, &storage, "non-durable").await;
}

/// `respawn_member` of a member whose session is lost is refused typed by
/// MobMachine's start verdict (`session_not_archived`), which the plan
/// probes for its observation of the session: no retirement starts, and the
/// member keeps its incarnation.
///
/// RED: drop the plan's observation (`session_archived` always false in
/// `plan_member_respawn`), or the guard: MobMachine admits the start, and
/// the respawn retires the member and seats a successor on the lost session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respawn_member_of_a_member_whose_session_is_lost_is_refused_by_the_machine() {
    let mut mob = RotationMob::new("respawn-lost-session").await;
    mob.handle
        .spawn_spec(worker_spec("lost-session"))
        .await
        .expect("spawn the member");
    let session = mob.session("lost-session").await;
    mob.crash_and_restart().await;
    mob.service.delete_persisted_session(&session).await;
    let refused = tokio::time::timeout(
        STEP,
        mob.handle
            .respawn_member(id("lost-session"), MemberRespawn::new()),
    )
    .await
    .expect("the respawn answers");
    assert!(
        matches!(
            &refused,
            Err(MobError::UnsupportedForMode { reason, .. }) if reason.contains("lost or archived")
        ),
        "{refused:?}"
    );
    assert_nothing_started(&mob.handle, &mob.storage, "lost-session").await;
}

/// `respawn_with_successor_spec` of an identity entry's member is refused
/// typed before anything starts: an entry's successor keeps its session,
/// which a successor spec (a fresh life) cannot. MobMachine owns the
/// refusal: its fresh-life respawn arm (`RespawnRunning`) admits no entry
/// (`not_an_identity_entry`), and its refusal twin answers `RespawnRefused`,
/// which the shell answers typed. The member stays the entry's.
///
/// RED: drop `RespawnRunning`'s `not_an_identity_entry` guard and its
/// refusal twin: MobMachine admits the fresh-life respawn, and the member
/// is retired and respawned on a new session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respawn_with_successor_spec_of_an_identity_entry_is_refused_typed() {
    let mob = RotationMob::new("entry-successor-spec").await;
    mob.handle
        .spawn_spec(entry_spec("entry-spec"))
        .await
        .expect("the host declares a spawned entry");
    let refused = tokio::time::timeout(
        STEP,
        mob.handle
            .respawn_with_successor_spec(worker_spec("entry-spec")),
    )
    .await
    .expect("the respawn answers");
    assert!(
        matches!(
            &refused,
            Err(crate::runtime::handle::MobRespawnError::Mob(
                MobError::UnsupportedForMode { reason, .. }
            )) if reason.contains("identity entry")
        ),
        "{refused:?}"
    );
    assert_nothing_started(&mob.handle, &mob.storage, "entry-spec").await;
    assert!(
        matches!(
            mob.handle.identity_entry(&id("entry-spec")),
            Ok(IdentityEntryRead::Entry(_))
        ),
        "the member stays the entry's"
    );
}

/// Counts the spawn customizations it is asked for.
#[derive(Default)]
struct CountingCustomizer {
    spawns: std::sync::atomic::AtomicUsize,
}

impl SpawnMemberCustomizer for CountingCustomizer {
    fn customize_spawn(
        &self,
        _ctx: &SpawnCustomizationContext,
        _spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.spawns
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

/// `respawn` of an identity entry's member in a stopped mob is refused by
/// MobMachine before anything runs: its refusal twin answers in every phase
/// an entry lives in, so the respawn takes the same-session path, whose
/// start a stopped mob refuses, and the host's spawn customizer is never
/// asked for an entry's fresh life. The journal, MobMachine's rotation facts
/// and the member's roster anchor are as they were.
///
/// RED: let the refusal twin answer in Running only: in the stopped mob the
/// routing finds no refusal, the respawn takes the fresh-life path and asks
/// the customizer for the entry's would-be successor before its admission
/// refuses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respawn_of_an_identity_entry_in_a_stopped_mob_is_refused_before_anything_runs() {
    let customizer = Arc::new(CountingCustomizer::default());
    let mob = RotationMob::new_with("entry-stopped-respawn", |builder| {
        builder
            .with_spawn_member_customizer(Arc::clone(&customizer) as Arc<dyn SpawnMemberCustomizer>)
    })
    .await;
    mob.handle
        .spawn_spec(entry_spec("stopped-entry"))
        .await
        .expect("the host declares a spawned entry");
    tokio::time::timeout(STEP, mob.handle.stop())
        .await
        .expect("the stop answers")
        .expect("stop the mob");
    let customized = customizer.spawns.load(std::sync::atomic::Ordering::SeqCst);
    let journal_before = mob.journal().await.len();
    let anchor_before = roster_anchor(&mob.handle, "stopped-entry").await;

    let refused = tokio::time::timeout(STEP, mob.handle.respawn(id("stopped-entry"), None))
        .await
        .expect("the respawn answers");
    assert!(
        matches!(
            &refused,
            Err(crate::runtime::handle::MobRespawnError::Mob(_))
        ),
        "the stopped mob refuses the entry's respawn typed: {refused:?}"
    );
    assert_eq!(
        customizer.spawns.load(std::sync::atomic::Ordering::SeqCst),
        customized,
        "the customizer is never asked for an entry's fresh life"
    );
    assert_eq!(
        mob.journal().await.len(),
        journal_before,
        "no row was written"
    );
    {
        let state = mob.handle.machine_state_watch_rx.borrow();
        let dsl_identity = dsl::AgentIdentity::from_domain(&id("stopped-entry"));
        assert!(
            !state.member_rotation_targets.contains_key(&dsl_identity)
                && !state
                    .member_rotation_session_ids
                    .contains_key(&dsl_identity),
            "MobMachine holds no rotation of the entry"
        );
    }
    assert_eq!(
        roster_anchor(&mob.handle, "stopped-entry").await,
        anchor_before,
        "the entry's roster anchor is unchanged"
    );
    assert!(
        matches!(
            mob.handle.identity_entry(&id("stopped-entry")),
            Ok(IdentityEntryRead::Entry(_))
        ),
        "the member stays the entry's"
    );
}

/// Rollback of an interrupted rotation, lane-run leg: needs the published
/// Core 0.9.7 (`meerkat-mob` 0.9.7). The Meta 0.9.6 leg needs the rotation
/// mirrored into Meta 0.9.6's own rotation record, which a later change adds;
/// until then that leg is not run.
///
/// This build writes the fixture: a persistent mob store (SQLite) at
/// `$MEERKAT_ROLLBACK_FIXTURE_DIR/interrupted-rotation/mob.sqlite` holding a
/// member whose rotation was interrupted after its retirement's durable
/// start: the start row carries the rotation record, and no terminal batch
/// follows. The lane resumes it with Core 0.9.7, which ignores the record
/// (its decoder drops the unknown key) and so ends the retirement as a plain
/// one; it checks that the resume succeeds and that a host re-issue of
/// `rotate_member` with the identity host-pending (Meta 0.9.6's record
/// re-drive) seats the next incarnation on the name-based session the
/// interrupted rotation targeted.
///
/// RED: a record the 0.9.7 decoder cannot ignore (a stored key on a public
/// field, or a new event kind) fails the 0.9.7 resume of this fixture.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published Core 0.9.7 resumes"]
async fn interrupted_rotation_under_097_is_completed_by_096() {
    let dir = rollback_fixture_dir("interrupted-rotation");
    let (handle, _service) = open_rotation_fixture(&dir, "rollback-interrupted-rotation").await;
    handle
        .spawn_spec(worker_spec("interrupted"))
        .await
        .expect("spawn the member");
    let hold = crate::runtime::actor::retirement_stage_hold::install(
        &id("interrupted"),
        "retire-runtime-quiesce",
    );
    let rotating = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.rotate_member(worker_spec("interrupted")).await })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the rotation's retirement starts");
    crash_stop_and_release_routes(handle).await;
    hold.release();
    let _ = tokio::time::timeout(STEP, rotating).await;
}

/// Rollback of same-session successor rows, lane-run leg: needs the
/// published Core 0.9.7 (`meerkat-mob` 0.9.7).
///
/// This build writes the fixture: a persistent mob store (SQLite) at
/// `$MEERKAT_ROLLBACK_FIXTURE_DIR/same-session-successor/mob.sqlite` holding
/// a member wired to a peer member and to an external peer, then respawned
/// on its own session (`respawn_member`): its preserving retirement start
/// row with the rotation record, and its terminal batch, whose
/// `MemberRetired` and `MemberSpawned` rows name the same session and whose
/// `MembersWiredBatch` and `ExternalPeerWired` rows record the kept edges
/// again. The lane resumes it with Core 0.9.7 and checks: the member is
/// listed `active` at generation 1 on the same session, wired to its peer
/// and to the external peer, a delivery wakes it, and a reset
/// (`rotate_member`) re-seats it.
///
/// RED: a successor row 0.9.7 cannot replay (a retirement and a spawn of the
/// same session that 0.9.7 reads as a collision, an edge row it refuses, or
/// a stored key it refuses) fails the resume, or leaves the member unlisted
/// or unwired.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published Core 0.9.7 resumes"]
async fn same_session_successor_rows_replay_under_097() {
    let dir = rollback_fixture_dir("same-session-successor");
    let (handle, _service) = open_rotation_fixture(&dir, "rollback-same-session-successor").await;
    for name in ["respawned", "respawned-peer"] {
        handle
            .spawn_spec(worker_spec(name))
            .await
            .expect("spawn the member");
    }
    handle
        .wire(id("respawned"), id("respawned-peer"))
        .await
        .expect("wire the members");
    handle
        .wire(
            id("respawned"),
            PeerTarget::External(test_trusted_peer_descriptor(
                "remote-mob/worker/rollback-external",
                "inproc://remote-mob/worker/rollback-external",
            )),
        )
        .await
        .expect("wire the external peer");
    tokio::time::timeout(
        STEP,
        handle.respawn_member(id("respawned"), MemberRespawn::new()),
    )
    .await
    .expect("the respawn answers")
    .expect("the member is respawned on its own session");
    handle.shutdown().await.expect("shut the fixture mob down");
}

/// Rollback of a same-session role move, lane-run leg: needs the published
/// Core 0.9.7 (`meerkat-mob` 0.9.7).
///
/// This build writes the fixture: a persistent mob store (SQLite) at
/// `$MEERKAT_ROLLBACK_FIXTURE_DIR/same-session-role-move/mob.sqlite`, whose
/// definition wires `lead` to `worker` (`role_wiring`), holding a worker
/// wired to a peer worker and then moved to `lead` on its own session
/// (`respawn_member` naming `lead`): its preserving retirement start row
/// with the rotation record, whose next endpoint is the member's own key
/// under the `lead` comms name, and its terminal batch, whose `MemberRetired`
/// and `MemberSpawned` rows name the same session, the `MemberSpawned` row
/// under `lead` with that endpoint, followed by the declared `lead` edge to
/// the peer. The build checks that shape before it shuts the mob down. The
/// lane resumes the store with Core 0.9.7, which ignores the rotation
/// record, and checks: the resume succeeds; the member is listed `active` at
/// generation 1 under `lead` on the same session, with the endpoint its
/// `MemberSpawned` row records (its own key under the new name); it is wired
/// to the peer; and a delivery to it wakes it under the new name.
///
/// RED: a role-move row 0.9.7 cannot replay (a `MemberSpawned` row under
/// another role on the session the retirement kept, read as a collision; an
/// endpoint under a new name with the retired incarnation's key, refused as
/// a reused peer id; or a stored key it refuses) fails the resume, or leaves
/// the member unlisted, under its old role, or unwired.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published Core 0.9.7 resumes"]
async fn same_session_role_move_rows_replay_under_097() {
    let dir = rollback_fixture_dir("same-session-role-move");
    let (handle, _service) = open_rotation_fixture_of(
        &dir,
        "rollback-same-session-role-move",
        sample_definition_with_cross_role_wiring(),
    )
    .await;
    for name in ["moved", "moved-peer"] {
        handle
            .spawn_spec(worker_spec(name))
            .await
            .expect("spawn the member");
    }
    handle
        .wire(id("moved"), id("moved-peer"))
        .await
        .expect("wire the members");
    let session = handle
        .resolve_bridge_session_id(&id("moved"))
        .await
        .expect("the member is bound to a session");
    let dsl_identity = dsl::AgentIdentity::from_domain(&id("moved"));
    let peer_id = handle
        .machine_state_watch_rx
        .borrow()
        .member_peer_ids
        .get(&dsl_identity)
        .cloned()
        .expect("the member's endpoint");
    tokio::time::timeout(
        STEP,
        handle.respawn_member(id("moved"), MemberRespawn::new().with_profile("lead")),
    )
    .await
    .expect("the role move answers")
    .expect("the member moves to the lead role on its own session");

    let entry = handle
        .get_member(&id("moved"))
        .await
        .expect("roster read")
        .expect("the successor is seated");
    assert_eq!(
        (entry.role.clone(), entry.generation),
        (ProfileName::from("lead"), Generation::new(1))
    );
    assert_eq!(
        handle.resolve_bridge_session_id(&id("moved")).await,
        Some(session),
        "the successor keeps the member's session"
    );
    let lead_name =
        crate::runtime::actor::render_member_comms_name(handle.mob_id().as_str(), "lead", "moved")
            .expect("a member comms name");
    {
        let state = handle.machine_state_watch_rx.borrow();
        assert_eq!(
            state
                .member_peer_endpoints
                .get(&dsl_identity)
                .map(|endpoint| (endpoint.name.0.clone(), endpoint.peer_id.clone())),
            Some((lead_name, peer_id)),
            "the successor's endpoint is the member's key under the lead comms name"
        );
        assert!(
            state.wiring_edges.contains(&dsl::WiringEdge::new(
                dsl_identity.clone(),
                dsl::AgentIdentity::from_domain(&id("moved-peer")),
            )),
            "the declared lead edge joins the peer"
        );
    }
    handle.shutdown().await.expect("shut the fixture mob down");
}

/// Rollback of an interrupted same-session role move, lane-run leg: needs the
/// published Core 0.9.7 (`meerkat-mob` 0.9.7; 0.9.8 changed no file of
/// `meerkat-mob` or `meerkat-machine-schema`, so it resumes the store the
/// same way).
///
/// This build writes the fixture: a persistent mob store (SQLite) at
/// `$MEERKAT_ROLLBACK_FIXTURE_DIR/interrupted-role-move/mob.sqlite`, whose
/// definition wires `lead` to `worker` (`role_wiring`), holding a worker
/// wired to a peer worker whose move to `lead` on its own session
/// (`respawn_member` naming `lead`) a crash interrupted at the kept session's
/// release: its preserving retirement start row with the rotation record
/// (the member's own key under the `lead` comms name), and no end of that
/// retirement and no successor. The build checks that shape after the crash.
///
/// 0.9.7 reads that start row as a plain one: its stored event decode takes
/// the fields it knows and its event kinds skip the rest, the rotation
/// record among them. Its replay restores the member Retiring under its old
/// role, with its old endpoint, on its kept binding, and its start re-drives
/// the retirement of every Retiring member as a plain one, whose end retires
/// the member and archives its session. The lane resumes the store with
/// Core 0.9.7 and checks: the resume succeeds; the re-drive settles
/// `Retired`; the member is no longer listed, and the journal ends its
/// generation 0 with one `MemberRetired` row and holds no `MemberSpawned` row
/// of its generation 1; no member is listed under `lead`; and the peer is
/// listed unwired from it. Nothing runs under the new role's comms name: the
/// identity ends whole, never split across two names.
///
/// RED: a start row 0.9.7 replays as anything but the plain retirement of
/// the old incarnation (a member kept under the new role's name, a successor
/// seated, an edge kept for it) fails those checks, and a record or key it
/// refuses fails the resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published Core 0.9.7 resumes"]
async fn an_interrupted_role_move_ends_as_a_plain_retirement_under_097() {
    let dir = rollback_fixture_dir("interrupted-role-move");
    let (handle, _service, storage) = open_rotation_fixture_with_storage(
        &dir,
        "rollback-interrupted-role-move",
        sample_definition_with_cross_role_wiring(),
    )
    .await;
    for name in ["moving", "moving-peer"] {
        handle
            .spawn_spec(worker_spec(name))
            .await
            .expect("spawn the member");
    }
    handle
        .wire(id("moving"), id("moving-peer"))
        .await
        .expect("wire the members");
    let dsl_identity = dsl::AgentIdentity::from_domain(&id("moving"));
    let peer_id = handle
        .machine_state_watch_rx
        .borrow()
        .member_peer_ids
        .get(&dsl_identity)
        .cloned()
        .expect("the member's endpoint");
    let lead_name =
        crate::runtime::actor::render_member_comms_name(handle.mob_id().as_str(), "lead", "moving")
            .expect("a member comms name");
    let hold = crate::runtime::actor::retirement_stage_hold::install(
        &id("moving"),
        "retire-session-release",
    );
    let moving = {
        let handle = handle.clone();
        tokio::spawn(async move {
            handle
                .respawn_member(id("moving"), MemberRespawn::new().with_profile("lead"))
                .await
        })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the move's retirement reaches the kept session's release");
    assert_eq!(
        handle
            .machine_state_watch_rx
            .borrow()
            .member_rotation_profile_names
            .get(&dsl_identity)
            .map(String::as_str),
        Some("lead"),
        "MobMachine holds the role the move started with"
    );
    crash_stop_and_release_routes(handle).await;
    hold.release();
    let _ = tokio::time::timeout(STEP, moving).await;

    let journal = storage
        .events
        .poll(0, usize::MAX)
        .await
        .expect("read the fixture's journal");
    let record = recorded_rotation(&journal, "moving", Generation::new(0))
        .expect("the start row records the move");
    assert_eq!(record.target(), crate::runtime::RotationTarget::SameSession);
    assert_eq!(record.successor_profile(), &ProfileName::from("lead"));
    assert_eq!(
        record
            .machine_peer_endpoint()
            .map(|endpoint| (endpoint.name.0, endpoint.peer_id)),
        Some((lead_name, peer_id)),
        "the record names the member's own key under the lead comms name"
    );
    assert_eq!(
        retirements_of(&journal, "moving"),
        0,
        "the move's retirement has no end"
    );
    assert!(
        !journal.iter().any(|event| matches!(
            &event.kind,
            MobEventKind::MemberSpawned(spawned)
                if spawned.agent_identity.as_str() == "moving"
                    && spawned.generation == Generation::new(1)
        )),
        "no successor is seated"
    );
}

fn rollback_fixture_dir(name: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(
        std::env::var_os("MEERKAT_ROLLBACK_FIXTURE_DIR")
            .expect("MEERKAT_ROLLBACK_FIXTURE_DIR names the fixture directory"),
    )
    .join(name);
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    dir
}

/// Create the fixture mob `mob_id` on a persistent store in `dir`: what the
/// published binaries resume.
async fn open_rotation_fixture(
    dir: &std::path::Path,
    mob_id: &str,
) -> (MobHandle, Arc<MockSessionService>) {
    open_rotation_fixture_of(dir, mob_id, sample_definition()).await
}

/// [`open_rotation_fixture`] of `definition`.
async fn open_rotation_fixture_of(
    dir: &std::path::Path,
    mob_id: &str,
    definition: MobDefinition,
) -> (MobHandle, Arc<MockSessionService>) {
    let (handle, service, _storage) =
        open_rotation_fixture_with_storage(dir, mob_id, definition).await;
    (handle, service)
}

/// [`open_rotation_fixture_of`], with the fixture's storage, which a build
/// reads the fixture's rows from after a crash.
async fn open_rotation_fixture_with_storage(
    dir: &std::path::Path,
    mob_id: &str,
    mut definition: MobDefinition,
) -> (MobHandle, Arc<MockSessionService>, MobStorage) {
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    definition.id = MobId::from(mob_id);
    let storage = MobStorage::persistent(dir.join("mob.sqlite")).expect("open the fixture store");
    let handle = MobBuilder::new(definition, storage.clone())
        .with_session_service(service.clone())
        .with_member_comms_identity_root(dir.join("comms-keys"))
        .create()
        .await
        .expect("create the fixture mob");
    (handle, service, storage)
}
