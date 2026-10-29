//! Seated members get the edges their definition declares, as a spawn of
//! each does. One seat batch wires the members it seats to each other and to
//! the members seated before it, without waking anyone; the members trust
//! each other once woken; a member on network comms is joined by the machine
//! edge alone until its boot wake registers its endpoint and installs the
//! trust of both ends; and a rotation's next incarnation is wired again. A
//! member exempt from the definition's wiring rules (`DefinitionWiring::Exempt`)
//! takes none of those edges, and no other member's seat, spawn or rotation
//! is wired to it by them. These rows run on the combined candidate: the
//! machine, the wake path and the boot path together.

use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::actor::member_wake_turn::probe_member_wake_restores_for_test;
use crate::runtime::{DormantSeatOutcome, MemberRestorePolicy};

const STEP: Duration = Duration::from_mins(1);

/// The sample definition with a `member` and a `helper` profile (each a
/// worker otherwise) that `role_wiring` pairs.
fn member_helper_definition() -> MobDefinition {
    let mut definition = sample_definition();
    let worker = definition
        .profiles
        .get(&ProfileName::from("worker"))
        .cloned()
        .expect("the worker profile");
    definition
        .profiles
        .insert(ProfileName::from("member"), worker.clone());
    definition
        .profiles
        .insert(ProfileName::from("helper"), worker);
    definition.wiring.role_wiring = vec![RoleWiringRule {
        a: ProfileName::from("member"),
        b: ProfileName::from("helper"),
    }];
    definition
}

/// A fresh turn-driven spec of `profile`: seated, it owes no kickoff.
fn spec(profile: &str, identity: &AgentIdentity) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new(profile, identity.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

/// An unordered identity pair, as MobMachine orders an edge.
fn pair(a: &AgentIdentity, b: &AgentIdentity) -> (String, String) {
    let (a, b) = (a.to_string(), b.to_string());
    if a <= b { (a, b) } else { (b, a) }
}

/// A running mob over the mock durable session service. Its seats record
/// each member's endpoint from its session-scoped comms key, or, on network
/// comms, record none.
struct WiringMob {
    keys: tempfile::TempDir,
    handle: MobHandle,
    service: Arc<MockSessionService>,
    events: Arc<dyn MobEventStore>,
    runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
}

impl WiringMob {
    async fn new(label: &str, definition: MobDefinition, network_comms: bool) -> Self {
        Self::build(label, definition, network_comms, None).await
    }

    /// A mob on session-scoped comms keys under the restore `policy`.
    async fn with_restore_policy(
        label: &str,
        definition: MobDefinition,
        policy: MemberRestorePolicy,
    ) -> Self {
        Self::build(label, definition, false, Some(policy)).await
    }

    async fn build(
        label: &str,
        definition: MobDefinition,
        network_comms: bool,
        policy: Option<MemberRestorePolicy>,
    ) -> Self {
        let keys = tempfile::tempdir().expect("a comms identity root");
        let definition = with_unique_mob_id(definition, label);
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let runtime_metadata = storage.runtime_metadata.clone();
        let builder = MobBuilder::new(definition, storage).with_session_service(service.clone());
        let builder = if network_comms {
            builder.with_network_member_comms()
        } else {
            builder.with_member_comms_identity_root(keys.path())
        };
        let builder = match policy {
            Some(policy) => builder.with_member_restore_policy(policy),
            None => builder,
        };
        let handle = builder.create().await.expect("create the mob");
        Self {
            keys,
            handle,
            service,
            events,
            runtime_metadata,
        }
    }

    /// Crash the mob, restart its session service and resume it from its
    /// journal and runtime metadata, on session-scoped comms keys.
    async fn restart(&mut self) {
        crash_stop_and_release_routes(self.handle.clone()).await;
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        self.service = restarted;
        self.handle = tokio::time::timeout(
            STEP,
            Box::pin(
                MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
                    self.events.clone(),
                    self.runtime_metadata.clone(),
                ))
                .with_session_service(self.service.clone())
                .with_member_comms_identity_root(self.keys.path())
                .notify_orchestrator_on_resume(false)
                .resume(),
            ),
        )
        .await
        .expect("the restart answers")
        .expect("the restart succeeds");
    }

    /// Seat `specs` in one call; every one is seated.
    async fn seat(&self, specs: Vec<SpawnMemberSpec>) {
        let outcomes = tokio::time::timeout(STEP, self.handle.seat_dormant_members(specs))
            .await
            .expect("the seat answers")
            .expect("the seat commits");
        assert!(
            outcomes
                .iter()
                .all(|outcome| matches!(outcome, DormantSeatOutcome::Seated { .. })),
            "every spec is seated: {outcomes:?}"
        );
    }

    /// MobMachine's member edges.
    fn edges(&self) -> BTreeSet<(String, String)> {
        self.handle
            .machine_state_watch_rx
            .borrow()
            .wiring_edges
            .iter()
            .map(|edge| (edge.a.0.clone(), edge.b.0.clone()))
            .collect()
    }

    /// The journal's member wiring rows, in order.
    async fn wired_rows(&self) -> Vec<(String, String)> {
        self.events
            .replay_all()
            .await
            .expect("read the journal")
            .into_iter()
            .flat_map(|event| match event.kind {
                MobEventKind::MembersWired { a, b } => vec![pair(&a, &b)],
                MobEventKind::MembersWiredBatch { edges } => {
                    edges.iter().map(|edge| pair(&edge.a, &edge.b)).collect()
                }
                _ => Vec::new(),
            })
            .collect()
    }

    fn phase(&self, identity: &AgentIdentity) -> Option<dsl::MemberMaterializationPhase> {
        self.handle.member_materialization_for_test(identity)
    }

    /// The members the roster projects `identity` wired to.
    async fn wired_to(&self, identity: &AgentIdentity) -> BTreeSet<AgentIdentity> {
        self.handle
            .get_member(identity)
            .await
            .expect("read the roster")
            .expect("a member")
            .wired_to
    }

    /// The peer id MobMachine records for `identity`'s endpoint.
    fn recorded_peer_id(&self, identity: &AgentIdentity) -> Option<String> {
        self.handle
            .machine_state_watch_rx
            .borrow()
            .member_peer_ids
            .get(&dsl::AgentIdentity::from_domain(identity))
            .map(|peer_id| peer_id.0.clone())
    }

    /// The peer ids `identity`'s comms runtime trusts; none while it has no
    /// comms runtime.
    async fn trusted(&self, identity: &AgentIdentity) -> BTreeSet<String> {
        let Some(session) = self.handle.resolve_bridge_session_id(identity).await else {
            return BTreeSet::new();
        };
        let Some(comms) = self.service.comms_runtime(&session).await else {
            return BTreeSet::new();
        };
        comms
            .peers()
            .await
            .into_iter()
            .map(|peer| peer.peer_id.to_string())
            .collect()
    }

    /// Whether `a` and `b` are live and each trusts the other's recorded
    /// endpoint.
    async fn trust_each_other(&self, a: &AgentIdentity, b: &AgentIdentity) -> bool {
        let live = Some(dsl::MemberMaterializationPhase::Live);
        if self.phase(a) != live || self.phase(b) != live {
            return false;
        }
        let (Some(a_peer), Some(b_peer)) = (self.recorded_peer_id(a), self.recorded_peer_id(b))
        else {
            return false;
        };
        self.trusted(a).await.contains(&b_peer) && self.trusted(b).await.contains(&a_peer)
    }

    async fn wake(&self, identity: &AgentIdentity) {
        tokio::time::timeout(STEP, self.handle.ensure_member_live(identity))
            .await
            .expect("the wake answers")
            .expect("the member wakes");
    }
}

/// One seat batch of a member and a helper under `role_wiring` records
/// their declared edge in MobMachine and one MembersWired row, as spawning
/// them would, and wakes neither: both stay Dormant and nothing is built. A
/// re-sent seat answers both AlreadySeated and wires nothing again, so an
/// edge the host unwired stays unwired.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_seat_batch_wires_a_member_and_a_helper_and_wakes_neither() {
    let mob = WiringMob::new("seat-wiring-batch", member_helper_definition(), false).await;
    let member = AgentIdentity::from("seat-member");
    let helper = AgentIdentity::from("seat-helper");
    mob.seat(vec![spec("member", &member), spec("helper", &helper)])
        .await;

    assert_eq!(
        mob.edges(),
        BTreeSet::from([pair(&member, &helper)]),
        "the declared edge is MobMachine's"
    );
    assert_eq!(
        mob.wired_rows().await,
        vec![pair(&member, &helper)],
        "one MembersWired row"
    );
    assert_eq!(
        mob.wired_to(&member).await,
        BTreeSet::from([helper.clone()])
    );
    assert_eq!(
        mob.wired_to(&helper).await,
        BTreeSet::from([member.clone()])
    );
    for identity in [&member, &helper] {
        assert_eq!(
            mob.phase(identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity} stays Dormant"
        );
    }
    assert!(
        mob.service.recorded_create_requests().await.is_empty(),
        "the wiring builds nothing"
    );
    assert_eq!(mob.service.active_session_count().await, 0);

    mob.handle
        .unwire(member.clone(), helper.clone())
        .await
        .expect("the host unwires the pair");
    let again = mob
        .handle
        .seat_dormant_members(vec![spec("member", &member), spec("helper", &helper)])
        .await
        .expect("the re-sent seat answers");
    assert!(
        again
            .iter()
            .all(|outcome| matches!(outcome, DormantSeatOutcome::AlreadySeated { .. })),
        "{again:?}"
    );
    assert!(
        mob.edges().is_empty(),
        "an already-seated pair is not wired again"
    );
    assert_eq!(mob.wired_rows().await.len(), 1);
}

/// Seated members that wake trust each other: each one's wake realizes its
/// own trust from MobMachine's declared edge, so once both are live the
/// member trusts the helper's endpoint and the helper the member's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn woken_seated_members_trust_each_other() {
    let mob = WiringMob::new("seat-wiring-woken", member_helper_definition(), false).await;
    let member = AgentIdentity::from("seat-member");
    let helper = AgentIdentity::from("seat-helper");
    mob.seat(vec![spec("member", &member), spec("helper", &helper)])
        .await;
    mob.wake(&member).await;
    mob.wake(&helper).await;

    assert!(
        mob.trust_each_other(&member, &helper).await,
        "the woken members trust each other: the member trusts {:?}, the helper {:?}",
        mob.trusted(&member).await,
        mob.trusted(&helper).await
    );
    assert_eq!(mob.edges(), BTreeSet::from([pair(&member, &helper)]));
    assert_eq!(mob.wired_rows().await, vec![pair(&member, &helper)]);
}

/// A helper seated in a later batch is wired to the member seated before
/// it, as a spawn of the helper would be; the member seated alone had no
/// declared peer to wire to.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_helper_seated_later_is_wired_to_the_seated_member() {
    let mob = WiringMob::new("seat-wiring-later", member_helper_definition(), false).await;
    let member = AgentIdentity::from("seat-member");
    let helper = AgentIdentity::from("seat-helper");
    mob.seat(vec![spec("member", &member)]).await;
    assert!(mob.edges().is_empty(), "no declared peer is seated yet");

    mob.seat(vec![spec("helper", &helper)]).await;
    assert_eq!(mob.edges(), BTreeSet::from([pair(&member, &helper)]));
    assert_eq!(mob.wired_rows().await, vec![pair(&member, &helper)]);
    for identity in [&member, &helper] {
        assert_eq!(
            mob.phase(identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity} stays Dormant"
        );
    }
}

/// Under `auto_wire_orchestrator`, a seated worker is wired to the
/// orchestrator exactly as a spawned worker is: the edge and its row, the
/// roster's projection on both ends, and the live orchestrator's trust in
/// the worker, here in its recorded endpoint while the worker stays Dormant.
/// The worker's wake then has it trust the orchestrator.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seated_worker_is_wired_to_the_orchestrator_as_a_spawned_one_is() {
    let mob = WiringMob::new(
        "seat-wiring-orchestrator",
        sample_definition_with_auto_wire(),
        false,
    )
    .await;
    let lead = AgentIdentity::from("seat-lead");
    let spawned = AgentIdentity::from("seat-spawned");
    let seated = AgentIdentity::from("seat-seated");
    mob.handle
        .spawn_spec(spec("lead", &lead))
        .await
        .expect("spawn the orchestrator");
    mob.handle
        .spawn_spec(spec("worker", &spawned))
        .await
        .expect("spawn a worker");
    assert_eq!(
        mob.wired_rows().await,
        vec![pair(&spawned, &lead)],
        "the spawn wires its worker to the orchestrator"
    );
    let spawned_peer = mob
        .recorded_peer_id(&spawned)
        .expect("the spawned worker's endpoint");
    assert!(mob.trusted(&lead).await.contains(&spawned_peer));

    mob.seat(vec![spec("worker", &seated)]).await;
    assert_eq!(
        mob.edges(),
        BTreeSet::from([pair(&spawned, &lead), pair(&seated, &lead)]),
        "the seat wires its worker to the orchestrator as the spawn did"
    );
    assert_eq!(
        mob.wired_rows().await,
        vec![pair(&spawned, &lead), pair(&seated, &lead)]
    );
    assert_eq!(mob.wired_to(&seated).await, BTreeSet::from([lead.clone()]));
    assert!(mob.wired_to(&lead).await.contains(&seated));
    assert_eq!(
        mob.phase(&seated),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the seated worker stays Dormant"
    );
    let seated_peer = mob
        .recorded_peer_id(&seated)
        .expect("the seat recorded the worker's endpoint");
    assert!(
        mob.trusted(&lead).await.contains(&seated_peer),
        "the live orchestrator trusts the seated worker's recorded endpoint"
    );

    mob.wake(&seated).await;
    let lead_peer = mob
        .recorded_peer_id(&lead)
        .expect("the orchestrator's endpoint");
    assert!(
        mob.trusted(&seated).await.contains(&lead_peer),
        "the woken worker trusts the orchestrator"
    );
}

/// On network comms a seat records no endpoint: the member and the helper
/// are seated owed theirs, and their declared edge is MobMachine's alone,
/// with its row and no trust on either end (neither has an endpoint to
/// trust, nor a comms runtime). Their boot wakes register their endpoints
/// and realize the edge: once both are live, each trusts the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_network_seat_pair_is_wired_and_trusts_each_other_after_its_boot_wakes() {
    let mob = WiringMob::new("seat-wiring-network", member_helper_definition(), true).await;
    let member = AgentIdentity::from("seat-member");
    let helper = AgentIdentity::from("seat-helper");
    let probe =
        probe_member_wake_restores_for_test(mob.handle.mob_id(), [member.clone(), helper.clone()]);
    mob.seat(vec![spec("member", &member), spec("helper", &helper)])
        .await;

    assert_eq!(mob.edges(), BTreeSet::from([pair(&member, &helper)]));
    assert_eq!(mob.wired_rows().await, vec![pair(&member, &helper)]);
    // Both boot wakes start, and are held at their restore.
    tokio::time::timeout(STEP, probe.wait_for_started(2))
        .await
        .expect("both boot wakes start");
    for identity in [&member, &helper] {
        assert_eq!(
            mob.recorded_peer_id(identity),
            None,
            "{identity} has no endpoint before its wake"
        );
        assert!(
            mob.trusted(identity).await.is_empty(),
            "{identity} trusts nothing before its wake"
        );
    }

    probe.release(&member);
    probe.release(&helper);
    let deadline = tokio::time::Instant::now() + STEP;
    while !mob.trust_each_other(&member, &helper).await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the woken pair never trusted each other: member {:?} trusts {:?}, helper {:?} trusts {:?}",
            mob.phase(&member),
            mob.trusted(&member).await,
            mob.phase(&helper),
            mob.trusted(&helper).await
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(mob.edges(), BTreeSet::from([pair(&member, &helper)]));
}

/// A rotation's next incarnation holds none of its predecessor's edges
/// (CommitMemberRotation ends them): it is wired to the edges its definition
/// declares, as a spawn of it would be, journaled again, and the live helper
/// trusts the new incarnation's recorded endpoint while it stays Dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotated_member_is_wired_again_to_its_declared_edges() {
    let mob = WiringMob::new("seat-wiring-rotation", member_helper_definition(), false).await;
    let member = AgentIdentity::from("seat-member");
    let helper = AgentIdentity::from("seat-helper");
    mob.handle
        .spawn_spec(spec("helper", &helper))
        .await
        .expect("spawn the helper");
    mob.handle
        .spawn_spec(spec("member", &member))
        .await
        .expect("spawn the member");
    assert_eq!(mob.edges(), BTreeSet::from([pair(&member, &helper)]));
    let old_peer = mob
        .recorded_peer_id(&member)
        .expect("the member's endpoint");

    tokio::time::timeout(STEP, mob.handle.rotate_member(spec("member", &member)))
        .await
        .expect("the rotation answers")
        .expect("the rotation succeeds");
    assert_eq!(
        mob.edges(),
        BTreeSet::from([pair(&member, &helper)]),
        "the next incarnation is wired to its declared edge"
    );
    assert_eq!(
        mob.wired_rows().await,
        vec![pair(&member, &helper), pair(&member, &helper)],
        "the rotation journals the edge again"
    );
    assert_eq!(
        mob.phase(&member),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
    let new_peer = mob
        .recorded_peer_id(&member)
        .expect("the rotation recorded the next incarnation's endpoint");
    assert_ne!(new_peer, old_peer, "the next incarnation has its own key");
    assert!(
        mob.trusted(&helper).await.contains(&new_peer),
        "the live helper trusts the next incarnation's recorded endpoint"
    );
}

/// Under `auto_wire_orchestrator`, workers seated in one call and the
/// orchestrator seated in a later one end fully wired: the orchestrator's
/// seat wires it to every member that auto-wires to it, as each worker's own
/// seat would have wired it to an orchestrator seated first. Nothing wakes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workers_seated_before_the_orchestrator_are_wired_to_it_at_its_seat() {
    let mob = WiringMob::new(
        "seat-wiring-orchestrator-later",
        sample_definition_with_auto_wire(),
        false,
    )
    .await;
    let lead = AgentIdentity::from("seat-lead");
    let first = AgentIdentity::from("seat-worker-1");
    let second = AgentIdentity::from("seat-worker-2");
    mob.seat(vec![spec("worker", &first), spec("worker", &second)])
        .await;
    assert!(mob.edges().is_empty(), "no orchestrator is seated yet");

    mob.seat(vec![spec("lead", &lead)]).await;
    let expected = BTreeSet::from([pair(&first, &lead), pair(&second, &lead)]);
    assert_eq!(
        mob.edges(),
        expected,
        "the orchestrator's seat wires every worker seated before it"
    );
    let rows = mob.wired_rows().await;
    assert_eq!(rows.len(), 2, "one MembersWired row per edge: {rows:?}");
    assert_eq!(rows.into_iter().collect::<BTreeSet<_>>(), expected);
    assert_eq!(
        mob.wired_to(&lead).await,
        BTreeSet::from([first.clone(), second.clone()])
    );
    for identity in [&lead, &first, &second] {
        assert_eq!(
            mob.phase(identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity} stays Dormant"
        );
    }
    assert!(
        mob.service.recorded_create_requests().await.is_empty(),
        "the wiring builds nothing"
    );
}

/// Rotating the orchestrator (a host's reset of it) re-wires every worker
/// to its next incarnation: CommitMemberRotation ends the retired
/// incarnation's edges, and the next one is wired to every member that
/// auto-wires to it, journaled again, with each live worker trusting the
/// next incarnation's recorded endpoint while it stays Dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotating_the_orchestrator_rewires_every_worker_to_its_next_incarnation() {
    let mob = WiringMob::new(
        "seat-wiring-orchestrator-rotation",
        sample_definition_with_auto_wire(),
        false,
    )
    .await;
    let lead = AgentIdentity::from("seat-lead");
    let first = AgentIdentity::from("seat-worker-1");
    let second = AgentIdentity::from("seat-worker-2");
    for (profile, identity) in [("lead", &lead), ("worker", &first), ("worker", &second)] {
        mob.handle
            .spawn_spec(spec(profile, identity))
            .await
            .expect("spawn a member");
    }
    let expected = BTreeSet::from([pair(&first, &lead), pair(&second, &lead)]);
    assert_eq!(mob.edges(), expected, "the spawned workers auto-wire");
    let old_peer = mob
        .recorded_peer_id(&lead)
        .expect("the orchestrator's endpoint");

    tokio::time::timeout(STEP, mob.handle.rotate_member(spec("lead", &lead)))
        .await
        .expect("the rotation answers")
        .expect("the rotation succeeds");
    assert_eq!(
        mob.edges(),
        expected,
        "every worker is wired to the orchestrator's next incarnation"
    );
    assert_eq!(
        mob.wired_rows().await.len(),
        4,
        "the rotation journals both edges again"
    );
    assert_eq!(
        mob.wired_to(&lead).await,
        BTreeSet::from([first.clone(), second.clone()])
    );
    assert_eq!(
        mob.phase(&lead),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
    let new_peer = mob
        .recorded_peer_id(&lead)
        .expect("the rotation recorded the next incarnation's endpoint");
    assert_ne!(new_peer, old_peer, "the next incarnation has its own key");
    for worker in [&first, &second] {
        assert!(
            mob.trusted(worker).await.contains(&new_peer),
            "the live {worker} trusts the next incarnation's recorded endpoint"
        );
    }
}

/// A seat chunk journals its seats and their declared edges in one append,
/// and so does a rotation its next incarnation and its edges: a seat is
/// never durable without the edges its definition declares. (No fault seam
/// reaches between two appends of one step on an atomic store, so this row
/// reads the appends' row kinds.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_step_journals_its_members_and_their_declared_edges_in_one_append() {
    let mob = WiringMob::new("seat-wiring-one-append", member_helper_definition(), false).await;
    let member = AgentIdentity::from("seat-member");
    let helper = AgentIdentity::from("seat-helper");
    mob.seat(vec![spec("member", &member), spec("helper", &helper)])
        .await;
    assert_eq!(
        crate::runtime::actor::seat_step_appends_for_test(mob.handle.mob_id()),
        vec![vec!["MemberSpawned", "MemberSpawned", "MembersWired"]],
        "the chunk's one append carries both seats and their declared edge"
    );

    tokio::time::timeout(STEP, mob.handle.rotate_member(spec("member", &member)))
        .await
        .expect("the rotation answers")
        .expect("the rotation succeeds");
    assert_eq!(
        crate::runtime::actor::seat_step_appends_for_test(mob.handle.mob_id())
            .last()
            .cloned(),
        Some(vec!["MemberRetired", "MemberSpawned", "MembersWired"]),
        "the rotation's one append carries the next incarnation and its declared edge"
    );
    assert_eq!(mob.edges(), BTreeSet::from([pair(&member, &helper)]));
}

/// A member the verified pass of a seat chunk records Broken moves the
/// topology epoch, so the chunk's live ends trust their counterparts before
/// that pass, under the permissions the chunk minted. Here a live
/// orchestrator, a worker seated earlier on a session that is then lost, and
/// a chunk that re-sends that worker (already seated) with two new workers:
/// the pass records the lost worker Broken, and the orchestrator trusts both
/// new workers at once, with no repair owed for their edges.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_the_verified_pass_breaks_leaves_the_healthy_seats_of_its_chunk_trusted() {
    let mob = WiringMob::with_restore_policy(
        "seat-wiring-verified-pass",
        sample_definition_with_auto_wire(),
        MemberRestorePolicy::OnDemandVerified,
    )
    .await;
    let lead = AgentIdentity::from("seat-lead");
    let lost = AgentIdentity::from("seat-lost");
    let first = AgentIdentity::from("seat-worker-1");
    let second = AgentIdentity::from("seat-worker-2");
    mob.handle
        .spawn_spec(spec("lead", &lead))
        .await
        .expect("spawn the orchestrator");
    let lost_session = SessionId::new();
    mob.service
        .replace_live_session(Session::with_id(lost_session.clone()))
        .await;
    let resume_lost = || {
        let mut spec = spec("worker", &lost);
        spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
            bridge_session_id: lost_session.clone(),
            resume_from_role: None,
        };
        spec
    };
    mob.seat(vec![resume_lost()]).await;
    mob.service.delete_persisted_session(&lost_session).await;

    let outcomes = tokio::time::timeout(
        STEP,
        mob.handle.seat_dormant_members(vec![
            resume_lost(),
            spec("worker", &first),
            spec("worker", &second),
        ]),
    )
    .await
    .expect("the seat answers")
    .expect("the seat commits");
    assert!(
        matches!(outcomes[0], DormantSeatOutcome::AlreadySeated { .. })
            && outcomes[1..]
                .iter()
                .all(|outcome| matches!(outcome, DormantSeatOutcome::Seated { .. })),
        "{outcomes:?}"
    );
    assert!(
        mob.handle
            .machine_state_watch_rx
            .borrow()
            .member_restore_failures
            .contains_key(&dsl::AgentIdentity::from_domain(&lost)),
        "the verified pass records the worker whose session is lost Broken"
    );
    let trusted = mob.trusted(&lead).await;
    for worker in [&first, &second] {
        let peer = mob
            .recorded_peer_id(worker)
            .expect("the seat recorded the worker's endpoint");
        assert!(
            trusted.contains(&peer),
            "the live orchestrator trusts {worker} at once: {trusted:?}"
        );
    }
    assert!(
        mob.handle
            .machine_state_watch_rx
            .borrow()
            .owed_edge_repairs
            .is_empty(),
        "no repair is owed for the new workers' edges"
    );
}

/// A declared edge whose live end the seat step cannot resolve (the live
/// orchestrator's comms runtime reports no public key) is still MobMachine's
/// edge, staged in the step with its row, and its failed realization is owed
/// a repair in MobMachine rather than only logged. Once the orchestrator's
/// end resolves again, the repair realizes the edge: the orchestrator trusts
/// the seated worker's recorded endpoint, and nothing more is journaled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declared_edge_whose_realization_fails_at_its_seat_is_owed_a_repair() {
    let mob = WiringMob::new(
        "seat-wiring-owed-repair",
        sample_definition_with_auto_wire(),
        false,
    )
    .await;
    let lead = AgentIdentity::from("seat-lead");
    let seated = AgentIdentity::from("seat-worker");
    mob.handle
        .spawn_spec(spec("lead", &lead))
        .await
        .expect("spawn the orchestrator");
    let lead_comms = test_comms_name_for(mob.handle.mob_id(), "lead", lead.as_str());
    mob.service
        .set_comms_behavior(
            &lead_comms,
            MockCommsBehavior {
                missing_public_key: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    mob.seat(vec![spec("worker", &seated)]).await;
    let edge = dsl::WiringEdge::new(
        dsl::AgentIdentity::from_domain(&seated),
        dsl::AgentIdentity::from_domain(&lead),
    );
    assert_eq!(
        mob.edges(),
        BTreeSet::from([pair(&seated, &lead)]),
        "the declared edge is MobMachine's"
    );
    assert_eq!(
        mob.wired_rows().await,
        vec![pair(&seated, &lead)],
        "the edge is journaled with its seat"
    );
    assert!(
        mob.handle
            .machine_state_watch_rx
            .borrow()
            .owed_edge_repairs
            .contains(&edge),
        "the edge whose realization failed is owed a repair"
    );
    let seated_peer = mob
        .recorded_peer_id(&seated)
        .expect("the seat recorded the worker's endpoint");

    mob.service
        .set_comms_behavior(&lead_comms, MockCommsBehavior::default())
        .await;
    let mut state = mob.handle.machine_state_watch_rx.clone();
    tokio::time::timeout(
        STEP,
        state.wait_for(|state| !state.owed_edge_repairs.contains(&edge)),
    )
    .await
    .expect("the repair realizes the edge")
    .expect("the machine state stays published");
    assert!(
        mob.trusted(&lead).await.contains(&seated_peer),
        "the orchestrator trusts the seated worker's recorded endpoint"
    );
    assert_eq!(
        mob.wired_rows().await,
        vec![pair(&seated, &lead)],
        "the repair journals nothing more"
    );
}

/// An edge MobMachine no longer holds when a woken member's topology step
/// reaches it is not restored. The woken member's wake pauses between its
/// first chunk and its second (one edge a chunk), and meanwhile its second
/// neighbour is rotated, which ends that neighbour's edges: the second chunk
/// restores nothing. The woken member trusts its first neighbour only, the
/// edge to the rotated neighbour is neither wired nor journaled again, and no
/// repair is owed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_woken_members_edge_ended_during_its_topology_step_is_not_restored() {
    let mob = WiringMob::new("wake-topology-ended-edge", sample_definition(), false).await;
    let woken = AgentIdentity::from("wake-member");
    let first = AgentIdentity::from("wake-peer-1");
    let second = AgentIdentity::from("wake-peer-2");
    mob.seat(vec![
        spec("worker", &woken),
        spec("worker", &first),
        spec("worker", &second),
    ])
    .await;
    for peer in [&first, &second] {
        tokio::time::timeout(STEP, mob.handle.wire(woken.clone(), peer.clone()))
            .await
            .expect("the wire answers")
            .expect("wire two dormant members");
    }
    assert_eq!(
        mob.wired_rows().await,
        vec![pair(&woken, &first), pair(&woken, &second)]
    );
    crate::runtime::actor::wake_topology::set_wake_topology_chunk_for_test(mob.handle.mob_id(), 1);
    let crate::runtime::actor::member_wake_turn::WakeTopologyYieldHold { entered, release } =
        crate::runtime::actor::member_wake_turn::hold_next_wake_topology_yield_for_test(&woken);
    let wake = tokio::spawn({
        let handle = mob.handle.clone();
        let woken = woken.clone();
        async move { handle.ensure_member_live(&woken).await }
    });
    tokio::time::timeout(STEP, entered)
        .await
        .expect("the wake pauses after its first chunk")
        .expect("the pause is reached");

    tokio::time::timeout(STEP, mob.handle.rotate_member(spec("worker", &second)))
        .await
        .expect("the rotation answers")
        .expect("the rotation succeeds");
    assert_eq!(
        mob.edges(),
        BTreeSet::from([pair(&woken, &first)]),
        "the rotation ended the second neighbour's edge"
    );
    let _ = release.send(());
    tokio::time::timeout(STEP, wake)
        .await
        .expect("the wake answers")
        .expect("the wake task finishes")
        .expect("the member wakes");

    assert_eq!(
        mob.edges(),
        BTreeSet::from([pair(&woken, &first)]),
        "the ended edge is not restored"
    );
    assert_eq!(
        mob.wired_rows().await,
        vec![pair(&woken, &first), pair(&woken, &second)],
        "no edge is journaled again"
    );
    assert!(
        mob.handle
            .machine_state_watch_rx
            .borrow()
            .owed_edge_repairs
            .is_empty(),
        "no repair is owed"
    );
    let trusted = mob.trusted(&woken).await;
    let first_peer = mob
        .recorded_peer_id(&first)
        .expect("the first neighbour's endpoint");
    let second_peer = mob
        .recorded_peer_id(&second)
        .expect("the rotated neighbour's endpoint");
    assert!(
        trusted.contains(&first_peer),
        "the woken member trusts its first neighbour: {trusted:?}"
    );
    assert!(
        !trusted.contains(&second_peer),
        "the woken member does not trust the rotated neighbour: {trusted:?}"
    );
}

// ---------------------------------------------------------------------------
// Members exempt from the definition's wiring rules
// (`DefinitionWiring::Exempt`): neither wired by those rules when they are
// seated, nor a target of another member's seat, spawn or rotation.
// ---------------------------------------------------------------------------

/// The sample definition under `auto_wire_orchestrator` (orchestrator
/// `lead`) with a `reviewer` profile (a worker otherwise) that a
/// `role_wiring` rule pairs with `worker`.
fn orchestrated_reviewer_definition() -> MobDefinition {
    let mut definition = sample_definition_with_auto_wire();
    let worker = definition
        .profiles
        .get(&ProfileName::from("worker"))
        .cloned()
        .expect("the worker profile");
    definition
        .profiles
        .insert(ProfileName::from("reviewer"), worker);
    definition.wiring.role_wiring = vec![RoleWiringRule {
        a: ProfileName::from("worker"),
        b: ProfileName::from("reviewer"),
    }];
    definition
}

/// `spec`, exempt from the definition's wiring rules.
fn exempt(spec: SpawnMemberSpec) -> SpawnMemberSpec {
    spec.with_definition_wiring(crate::DefinitionWiring::Exempt)
}

impl WiringMob {
    /// Whether MobMachine records `identity` exempt from the definition's
    /// wiring rules.
    fn machine_exempt(&self, identity: &AgentIdentity) -> bool {
        self.handle
            .machine_state_watch_rx
            .borrow()
            .member_definition_wiring_exempt
            .contains(&dsl::AgentIdentity::from_domain(identity))
    }

    /// The definition wiring `identity`'s roster entry records.
    async fn roster_definition_wiring(&self, identity: &AgentIdentity) -> crate::DefinitionWiring {
        self.handle
            .get_member(identity)
            .await
            .expect("read the roster")
            .expect("a member")
            .definition_wiring
    }

    /// The `mob.peer_added` notices `members` sent, counted together.
    async fn peer_added_notices(&self, members: &[&AgentIdentity]) -> usize {
        let mut count = 0;
        for member in members {
            if let Some(session) = self.handle.resolve_bridge_session_id(member).await {
                count += self
                    .service
                    .sent_intents(&session)
                    .await
                    .iter()
                    .filter(|intent| intent.as_str() == "mob.peer_added")
                    .count();
            }
        }
        count
    }

    /// Fork `source` into `child` with `spec`, retried while the runtime
    /// still settles the source's last turn (a typed busy refusal).
    async fn fork_when_idle(&self, source: &AgentIdentity, child_spec: SpawnMemberSpec) {
        for _ in 0..50 {
            match tokio::time::timeout(
                STEP,
                self.handle.fork_member(source, child_spec.clone(), None),
            )
            .await
            .expect("the fork answers")
            {
                Ok(_) => return,
                Err(MobError::ForkSourceUnavailable {
                    cause: crate::error::ForkSourceUnavailableCause::Running,
                    ..
                }) => tokio::time::sleep(Duration::from_millis(100)).await,
                Err(error) => panic!("the fork of {source} is seated: {error}"),
            }
        }
        panic!("the source {source} never settled for its fork");
    }
}

/// Spawn `identity` of `profile` with a first turn, turn-driven, and wait
/// until its session holds no admitted input, so it can be forked.
async fn spawn_settled(handle: &MobHandle, profile: &str, identity: &AgentIdentity) {
    let source = handle
        .spawn_with_options(
            ProfileName::from(profile),
            identity.clone(),
            Some(ContentInput::Text("source context".to_string())),
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn a fork source");
    let session = source
        .bridge_session_id()
        .expect("a session-backed fork source")
        .clone();
    wait_for_fork_source_settled(handle, &session).await;
}

/// R1: a member spawned or seated exempt from the definition's wiring rules
/// takes none of the edges they declare for its role (the orchestrator's
/// auto-wire and the worker-reviewer rule): no edge, no MembersWired row and
/// no `mob.peer_added` notice, and its roster entry and MobMachine record
/// the exemption.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_exempt_member_takes_no_declared_edge_when_it_is_spawned_or_seated() {
    let mob = WiringMob::new("exempt-seated", orchestrated_reviewer_definition(), false).await;
    let lead = AgentIdentity::from("exempt-lead");
    let reviewer = AgentIdentity::from("exempt-reviewer");
    let spawned = AgentIdentity::from("exempt-spawned-worker");
    let seated = AgentIdentity::from("exempt-seated-worker");
    for (profile, identity) in [("lead", &lead), ("reviewer", &reviewer)] {
        mob.handle
            .spawn_spec(spec(profile, identity))
            .await
            .expect("spawn a member");
    }
    let edges = BTreeSet::from([pair(&reviewer, &lead)]);
    assert_eq!(
        mob.edges(),
        edges,
        "the reviewer auto-wires to the orchestrator"
    );
    let rows = mob.wired_rows().await;
    let notices = mob.peer_added_notices(&[&lead, &reviewer]).await;

    mob.handle
        .spawn_spec(exempt(spec("worker", &spawned)))
        .await
        .expect("spawn an exempt worker");
    mob.seat(vec![exempt(spec("worker", &seated))]).await;

    assert_eq!(
        mob.edges(),
        edges,
        "neither exempt worker takes a declared edge"
    );
    assert_eq!(
        mob.wired_rows().await,
        rows,
        "no MembersWired row names them"
    );
    assert_eq!(
        mob.peer_added_notices(&[&lead, &reviewer, &spawned]).await,
        notices,
        "no mob.peer_added notice is sent for them"
    );
    for identity in [&spawned, &seated] {
        assert!(mob.wired_to(identity).await.is_empty(), "{identity}");
        assert!(mob.machine_exempt(identity), "{identity}");
        assert_eq!(
            mob.roster_definition_wiring(identity).await,
            crate::DefinitionWiring::Exempt,
            "{identity}"
        );
    }
    for identity in [&lead, &reviewer] {
        assert!(!mob.machine_exempt(identity), "{identity}");
        assert_eq!(
            mob.roster_definition_wiring(identity).await,
            crate::DefinitionWiring::Applies,
            "{identity}"
        );
    }
}

/// R2: an exempt member is no target of a later member's declared edges. A
/// worker spawned after an exempt member of the orchestrator's role is
/// wired to the real orchestrator only, and a reviewer spawned after an
/// exempt worker is wired to the bound worker only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn later_members_are_not_wired_to_an_exempt_member() {
    let mob = WiringMob::new("exempt-later", orchestrated_reviewer_definition(), false).await;
    let lead = AgentIdentity::from("exempt-lead");
    let side_lead = AgentIdentity::from("exempt-lead-side");
    let exempt_worker = AgentIdentity::from("exempt-worker");
    let worker = AgentIdentity::from("bound-worker");
    let reviewer = AgentIdentity::from("bound-reviewer");
    for member in [
        spec("lead", &lead),
        exempt(spec("lead", &side_lead)),
        exempt(spec("worker", &exempt_worker)),
        spec("worker", &worker),
        spec("reviewer", &reviewer),
    ] {
        mob.handle.spawn_spec(member).await.expect("spawn a member");
    }
    assert_eq!(
        mob.edges(),
        BTreeSet::from([
            pair(&worker, &lead),
            pair(&reviewer, &lead),
            pair(&reviewer, &worker),
        ]),
        "the later members are wired to the real orchestrator and the bound worker only"
    );
    assert!(mob.wired_to(&side_lead).await.is_empty());
    assert!(mob.wired_to(&exempt_worker).await.is_empty());
}

/// Core 0.9.7 seat wiring: an identity seated after an exempt member of the
/// orchestrator's role is wired to the real orchestrator only. The seat's
/// active index (`seated_declared_pairs`) leaves the exempt member out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_after_an_exempt_orchestrator_member_is_not_wired_to_it() {
    let mob = WiringMob::new(
        "exempt-seat-after",
        sample_definition_with_auto_wire(),
        false,
    )
    .await;
    let lead = AgentIdentity::from("exempt-lead");
    let side_lead = AgentIdentity::from("exempt-lead-side");
    let seated = AgentIdentity::from("seat-worker");
    mob.handle
        .spawn_spec(spec("lead", &lead))
        .await
        .expect("spawn the orchestrator");
    mob.handle
        .spawn_spec(exempt(spec("lead", &side_lead)))
        .await
        .expect("spawn an exempt member of the orchestrator's role");

    mob.seat(vec![spec("worker", &seated)]).await;
    assert_eq!(
        mob.edges(),
        BTreeSet::from([pair(&seated, &lead)]),
        "the seated worker is wired to the real orchestrator only"
    );
    assert!(mob.wired_to(&side_lead).await.is_empty());
}

/// Core 0.9.7 rotation wiring: the next incarnation of a rotated worker is
/// wired to the real orchestrator only, never to an exempt member of the
/// orchestrator's role; and an exempt member rotated with an exempt spec
/// stays exempt and unwired.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_is_not_wired_to_an_exempt_orchestrator_member() {
    let mob = WiringMob::new("exempt-rotation", sample_definition_with_auto_wire(), false).await;
    let lead = AgentIdentity::from("exempt-lead");
    let side_lead = AgentIdentity::from("exempt-lead-side");
    let worker = AgentIdentity::from("rotated-worker");
    for member in [
        spec("lead", &lead),
        exempt(spec("lead", &side_lead)),
        spec("worker", &worker),
    ] {
        mob.handle.spawn_spec(member).await.expect("spawn a member");
    }
    let edges = BTreeSet::from([pair(&worker, &lead)]);
    assert_eq!(mob.edges(), edges);

    tokio::time::timeout(STEP, mob.handle.rotate_member(spec("worker", &worker)))
        .await
        .expect("the rotation answers")
        .expect("the rotation succeeds");
    assert_eq!(
        mob.edges(),
        edges,
        "the next incarnation is wired to the real orchestrator only"
    );

    tokio::time::timeout(
        STEP,
        mob.handle.rotate_member(exempt(spec("lead", &side_lead))),
    )
    .await
    .expect("the rotation answers")
    .expect("the rotation succeeds");
    assert!(
        mob.machine_exempt(&side_lead),
        "the exempt next incarnation is exempt"
    );
    assert!(mob.wired_to(&side_lead).await.is_empty());
    assert_eq!(mob.edges(), edges);
}

/// Core 0.9.7 seat wiring, both ways: an orchestrator seated after the
/// workers that auto-wire to it is wired to every one of them except an
/// exempt worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_orchestrator_seat_is_not_wired_to_an_exempt_member_of_another_profile() {
    let mob = WiringMob::new(
        "exempt-orchestrator-seat",
        sample_definition_with_auto_wire(),
        false,
    )
    .await;
    let lead = AgentIdentity::from("seat-lead");
    let worker = AgentIdentity::from("seat-worker");
    let exempt_worker = AgentIdentity::from("seat-exempt-worker");
    mob.seat(vec![
        spec("worker", &worker),
        exempt(spec("worker", &exempt_worker)),
    ])
    .await;
    assert!(mob.edges().is_empty(), "no orchestrator is seated yet");

    mob.seat(vec![spec("lead", &lead)]).await;
    assert_eq!(
        mob.edges(),
        BTreeSet::from([pair(&worker, &lead)]),
        "the orchestrator's seat wires it to the bound worker only"
    );
    assert!(mob.wired_to(&exempt_worker).await.is_empty());
}

/// R3: a durable fork seated exempt (as a console side chat is) is unwired
/// whatever its source's role, its source keeps its edges, and a worker
/// seated after it is not wired to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_exempt_fork_of_any_role_is_unwired_and_no_later_seat_targets_it() {
    let mob = WiringMob::new("exempt-fork", sample_definition_with_auto_wire(), false).await;
    let lead = AgentIdentity::from("fork-lead");
    let worker = AgentIdentity::from("fork-worker");
    spawn_settled(&mob.handle, "lead", &lead).await;
    spawn_settled(&mob.handle, "worker", &worker).await;
    let before = mob.edges();
    assert_eq!(before, BTreeSet::from([pair(&worker, &lead)]));

    let lead_child = AgentIdentity::from("fork-lead-side");
    let worker_child = AgentIdentity::from("fork-worker-side");
    for (source, child, profile) in [
        (&lead, &lead_child, "lead"),
        (&worker, &worker_child, "worker"),
    ] {
        mob.fork_when_idle(source, exempt(spec(profile, child)))
            .await;
        assert!(mob.wired_to(child).await.is_empty(), "{child} has no peer");
        assert!(mob.machine_exempt(child), "{child}");
        assert_eq!(
            mob.roster_definition_wiring(child).await,
            crate::DefinitionWiring::Exempt,
            "{child}"
        );
    }
    assert_eq!(
        mob.edges(),
        before,
        "neither fork changes its source's edges"
    );

    let seated = AgentIdentity::from("fork-later-worker");
    mob.seat(vec![spec("worker", &seated)]).await;
    assert_eq!(
        mob.edges(),
        BTreeSet::from([pair(&worker, &lead), pair(&seated, &lead)]),
        "the later worker is wired to the real orchestrator only"
    );
}

/// R4: the exemption is durable. A respawn of an exempt member keeps it,
/// unwired; after a restart the member is still exempt (its roster entry
/// from its `MemberSpawned` row, MobMachine from the row's replay), and a
/// worker seated after the restart is not wired to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_exempt_member_stays_exempt_across_a_respawn_and_a_restart() {
    let mut mob = WiringMob::new("exempt-durable", sample_definition_with_auto_wire(), false).await;
    let lead = AgentIdentity::from("durable-lead");
    let side_lead = AgentIdentity::from("durable-lead-side");
    mob.handle
        .spawn_spec(spec("lead", &lead))
        .await
        .expect("spawn the orchestrator");
    mob.handle
        .spawn_spec(exempt(spec("lead", &side_lead)))
        .await
        .expect("spawn an exempt member of the orchestrator's role");

    tokio::time::timeout(STEP, mob.handle.respawn(side_lead.clone(), None))
        .await
        .expect("the respawn answers")
        .expect("the respawn succeeds");
    assert!(mob.machine_exempt(&side_lead), "the successor is exempt");
    assert_eq!(
        mob.roster_definition_wiring(&side_lead).await,
        crate::DefinitionWiring::Exempt
    );
    assert!(mob.wired_to(&side_lead).await.is_empty());

    mob.restart().await;
    assert!(
        mob.machine_exempt(&side_lead),
        "the replay of its row records the member exempt again"
    );
    assert!(!mob.machine_exempt(&lead));
    assert_eq!(
        mob.roster_definition_wiring(&side_lead).await,
        crate::DefinitionWiring::Exempt
    );

    let seated = AgentIdentity::from("durable-seat-worker");
    mob.seat(vec![spec("worker", &seated)]).await;
    assert_eq!(
        mob.edges(),
        BTreeSet::from([pair(&seated, &lead)]),
        "a worker seated after the restart is wired to the real orchestrator only"
    );
    assert!(mob.wired_to(&side_lead).await.is_empty());
}

/// R5: explicit wiring is unaffected. The host wires an exempt member with
/// `MobHandle::wire`, and its respawn restores that edge, as for any member,
/// while the member stays exempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_exempt_member_takes_explicit_wiring_and_its_respawn_restores_it() {
    let mob = WiringMob::new("exempt-explicit", sample_definition_with_auto_wire(), false).await;
    let lead = AgentIdentity::from("explicit-lead");
    let side_worker = AgentIdentity::from("explicit-exempt-worker");
    mob.handle
        .spawn_spec(spec("lead", &lead))
        .await
        .expect("spawn the orchestrator");
    mob.handle
        .spawn_spec(exempt(spec("worker", &side_worker)))
        .await
        .expect("spawn an exempt worker");
    assert!(
        mob.edges().is_empty(),
        "the exempt worker takes no declared edge"
    );

    mob.handle
        .wire(side_worker.clone(), lead.clone())
        .await
        .expect("the host wires the exempt worker explicitly");
    let edges = BTreeSet::from([pair(&side_worker, &lead)]);
    assert_eq!(mob.edges(), edges);

    tokio::time::timeout(STEP, mob.handle.respawn(side_worker.clone(), None))
        .await
        .expect("the respawn answers")
        .expect("the respawn succeeds");
    assert_eq!(mob.edges(), edges, "the respawn restores the explicit edge");
    assert!(mob.machine_exempt(&side_worker), "the successor is exempt");
}
