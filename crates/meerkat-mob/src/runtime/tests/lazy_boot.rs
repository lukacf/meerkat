//! Boot-side rows of member residency: the lazy start, reads that never
//! wake, the verified pass, wiring with dormant members (external peers
//! included), retiring and destroying dormant members, seats, rotation and
//! the kickoff a rotation owes. They run on the combined candidate: the
//! machine, the wake path and the boot path together.

use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::dormant_seat::fresh_seat_session_id;
use crate::runtime::{DormantSeatOutcome, MemberRestorePolicy, MobBootSetBasis};

const LAZY_STEP: Duration = Duration::from_secs(60);

fn worker(index: usize) -> AgentIdentity {
    AgentIdentity::from(format!("lazy-{index:02}"))
}

fn turn_driven(identity: &AgentIdentity) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", identity.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

/// `sample_definition` with an externally addressable worker. A delivery
/// through a member handle (`MemberHandle::send`) is External-origin work,
/// which MobMachine refuses for a member whose profile is not externally
/// addressable, Dormant or Live.
fn addressable_worker_definition() -> MobDefinition {
    let mut definition = sample_definition();
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline_mut()
        .expect("inline worker profile")
        .external_addressable = true;
    definition
}

fn phase_of(
    handle: &MobHandle,
    identity: &AgentIdentity,
) -> Option<dsl::MemberMaterializationPhase> {
    handle.member_materialization_for_test(identity)
}

/// A member's kickoff phase, as its status reads it.
async fn kickoff_phase_of(
    handle: &MobHandle,
    identity: &AgentIdentity,
) -> Option<crate::roster::MobMemberKickoffPhase> {
    handle
        .member_status(identity)
        .await
        .expect("the member's status")
        .kickoff
        .map(|kickoff| kickoff.phase)
}

/// A mob whose turn-driven workers were spawned on a mock durable session
/// service and crash-stopped, with the durable state a restart finds.
struct CrashedMob {
    events: Arc<dyn MobEventStore>,
    runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    /// The restarted service: durable session state only.
    service: Arc<MockSessionService>,
    /// Each worker and its session, in spawn order.
    sessions: Vec<(AgentIdentity, SessionId)>,
}

impl CrashedMob {
    /// Spawn `members` workers (the first two wired when `wire`), then crash
    /// (or, when `stop`, stop first) and restart the session service.
    async fn new(label: &str, members: usize, wire: bool, stop: bool) -> Self {
        Self::with_definition(sample_definition(), label, members, wire, stop).await
    }

    /// [`Self::new`] over `definition`.
    async fn with_definition(
        definition: MobDefinition,
        label: &str,
        members: usize,
        wire: bool,
        stop: bool,
    ) -> Self {
        let definition = with_unique_mob_id(definition, label);
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let runtime_metadata = storage.runtime_metadata.clone();
        let handle = MobBuilder::new(definition, storage)
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create the mob");
        let mut sessions = Vec::with_capacity(members);
        for index in 0..members {
            let identity = worker(index);
            handle
                .spawn_spec(turn_driven(&identity))
                .await
                .expect("spawn a worker");
            let session = handle
                .resolve_bridge_session_id(&identity)
                .await
                .expect("a session-backed worker");
            sessions.push((identity, session));
        }
        if wire {
            handle
                .wire(worker(0), worker(1))
                .await
                .expect("wire the first two workers");
        }
        if stop {
            handle.stop().await.expect("stop before the restart");
        }
        let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        crash_stop_and_release_routes(handle).await;
        drop(service);
        Self {
            events,
            runtime_metadata,
            service: restarted,
            sessions,
        }
    }

    fn builder(&self) -> MobBuilder {
        MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
            self.events.clone(),
            self.runtime_metadata.clone(),
        ))
        .with_session_service(self.service.clone())
        .notify_orchestrator_on_resume(false)
    }

    async fn resume(&self, builder: MobBuilder) -> MobHandle {
        tokio::time::timeout(LAZY_STEP, Box::pin(builder.resume()))
            .await
            .expect("the resume answers")
            .expect("the resume succeeds")
    }

    /// The sessions the restarted service built an agent for, in order.
    async fn materialized(&self) -> Vec<SessionId> {
        self.service
            .recorded_create_requests()
            .await
            .into_iter()
            .map(|record| record.session_id)
            .collect()
    }

    fn session(&self, index: usize) -> SessionId {
        self.sessions[index].1.clone()
    }
}

/// A crash-path start under the default policy materializes no member
/// that holds no work, reads every member Dormant, and its plan was based on
/// an observed store. One host delivery to one member then materializes
/// exactly that member, on its own session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crash_path_start_materializes_no_member_and_one_delivery_wakes_one() {
    // The host delivery below goes through a member handle: the worker is
    // externally addressable.
    let mob = CrashedMob::with_definition(
        addressable_worker_definition(),
        "lazy-crash-start",
        6,
        false,
        false,
    )
    .await;
    let handle = mob.resume(mob.builder()).await;
    assert!(
        mob.materialized().await.is_empty(),
        "the start built no member: {:?}",
        mob.materialized().await
    );
    assert_eq!(mob.service.active_session_count().await, 0);
    for (identity, _) in &mob.sessions {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity}"
        );
    }
    assert_eq!(handle.boot_set_basis(), MobBootSetBasis::Observed);

    let target = worker(3);
    handle
        .member(&target)
        .await
        .expect("a member handle")
        .send("first touch", HandlingMode::Queue)
        .await
        .expect("the delivery wakes the member and is accepted");
    assert_eq!(mob.materialized().await, vec![mob.session(3)]);
    assert_eq!(
        phase_of(&handle, &target),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    for (identity, _) in mob
        .sessions
        .iter()
        .filter(|(identity, _)| identity != &target)
    {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity} stays dormant"
        );
    }
}

/// The lift of a stopped mob plans its members exactly as a crash-path
/// start does: none that holds no work is materialized.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lift_of_a_stopped_mob_materializes_no_member_without_work() {
    let mob = CrashedMob::new("lazy-lift", 4, false, true).await;
    let handle = mob.resume(mob.builder()).await;
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
    tokio::time::timeout(LAZY_STEP, handle.resume())
        .await
        .expect("the lift answers")
        .expect("the lift succeeds");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    assert!(mob.materialized().await.is_empty());
    for (identity, _) in &mob.sessions {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity}"
        );
    }
}

/// The lift binds the operation owners of the members it holds live, and
/// only those: a member left Dormant gets its binding at its wake, through
/// the shared cold recipe, so the lift's post-commit work neither reads its
/// persisted session nor registers a runtime for it. Here the host requires
/// one member, which the lift's plan materializes; every other member's
/// durable reads are refused, so a read of one fails the lift.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lift_binds_only_the_members_it_holds_live() {
    let mob = CrashedMob::new("lazy-lift-bindings", 6, false, true).await;
    let live = worker(0);
    let handle = mob
        .resume(mob.builder().with_host_required_members([live.clone()]))
        .await;
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
    let dormant = mob
        .sessions
        .iter()
        .filter(|(identity, _)| identity != &live)
        .cloned()
        .collect::<Vec<_>>();
    for (_, session) in &dormant {
        mob.service
            .refuse_persisted_session_reads(session, PersistedSessionReadRefusal::StoreFailure);
    }

    tokio::time::timeout(LAZY_STEP, handle.resume())
        .await
        .expect("the lift answers")
        .expect("the lift reads no dormant member's session");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    assert_eq!(
        phase_of(&handle, &live),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    let runtime = mob.service.enable_runtime_adapter();
    assert!(
        runtime.contains_session(&mob.session(0)).await,
        "the live member's runtime is registered"
    );
    for (identity, session) in &dormant {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity}"
        );
        assert!(
            !runtime.contains_session(session).await,
            "the lift registers no runtime for the dormant member {identity}"
        );
    }
    assert_eq!(
        mob.materialized().await,
        vec![mob.session(0)],
        "the lift built the live member only"
    );
}

/// The lift of a stopped mob with no member live in this process settles
/// its (empty) readiness before the durable Resume, and only then does its
/// member plan open the wakes of the members it materializes. Each of those
/// wakes runs its member's readiness itself, so the lift joins them after
/// its own readiness settled: it answers Ok once every member it planned is
/// Live, and every other member stays Dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lift_joins_the_wakes_its_plan_opens_after_its_readiness() {
    let mob = CrashedMob::new("lazy-lift-join", 4, false, true).await;
    let required = [worker(0), worker(2)];
    let handle = mob
        .resume(mob.builder().with_host_required_members(required.clone()))
        .await;
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);

    tokio::time::timeout(LAZY_STEP, handle.resume())
        .await
        .expect("the lift answers")
        .expect("the lift joins the wakes its plan opened");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    for (identity, _) in &mob.sessions {
        let expected = if required.contains(identity) {
            dsl::MemberMaterializationPhase::Live
        } else {
            dsl::MemberMaterializationPhase::Dormant
        };
        assert_eq!(phase_of(&handle, identity), Some(expected), "{identity}");
    }
    let mut materialized = mob.materialized().await;
    materialized.sort();
    let mut planned = vec![mob.session(0), mob.session(2)];
    planned.sort();
    assert_eq!(
        materialized, planned,
        "the lift built each member it planned, once"
    );
}

/// A lift whose planned member takes longer to build than the lift's
/// inactivity budget (two seconds in this build) waits for the build and
/// answers Ok with the member Live. The lift reports its wait for a joined
/// wake as the member's host materialization, which no platform budget
/// ends. Before, it reported nothing between the durable Resume and the
/// member's settle, so the caller was answered
/// `LifecycleOperationProgressStalled` at `durable_resume_transition` while
/// the member still went Live.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lift_waits_for_a_slow_planned_build_without_a_stall() {
    let mob = CrashedMob::new("lazy-lift-slow-build", 2, false, true).await;
    let required = worker(0);
    let handle = mob
        .resume(mob.builder().with_host_required_members([required.clone()]))
        .await;
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
    // Each session build now outlasts the lift's inactivity budget.
    mob.service.set_create_session_delay_ms(3_000);

    let lifted = tokio::time::timeout(LAZY_STEP, handle.resume())
        .await
        .expect("the lift answers");
    assert!(
        lifted.is_ok(),
        "the lift waits for the slow build and succeeds: {lifted:?}"
    );
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    assert_eq!(
        phase_of(&handle, &required),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    assert_eq!(
        phase_of(&handle, &worker(1)),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
    assert_eq!(mob.materialized().await, vec![mob.session(0)]);
}

/// A lift whose plan reads a slow store answers Ok without a stall. Under
/// the verified policy the lift of a stopped mob reads the store before it
/// answers: its obligation scan, then the verified pass's read of each
/// member it leaves Dormant (one read at a time here). Each read answers
/// within the lift's inactivity budget (two seconds in this build), and
/// all of them together outlast it. The lift reports each read as a unit of
/// work, so the caller's budget starts again after each one, and the lift
/// answers Ok with both members Dormant. Before, nothing reported progress
/// from the durable Resume to the end of the pass, and the caller was
/// answered `LifecycleOperationProgressStalled` at
/// `durable_resume_transition` while the lift went on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lift_whose_plan_reads_a_slow_store_answers_without_a_stall() {
    let mob = CrashedMob::new("lazy-lift-slow-store", 2, false, true).await;
    let handle = mob
        .resume(
            mob.builder()
                .with_member_restore_policy(MemberRestorePolicy::OnDemandVerified)
                .with_resume_restore_concurrency(1),
        )
        .await;
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
    // The obligation scan, and each member's read in the verified pass,
    // answer after 1.2 s: each within the budget, the three beyond it.
    crate::runtime::boot_plan::delay_next_boot_obligation_scan_for_test(
        handle.mob_id(),
        Duration::from_millis(1_200),
    );
    mob.service.set_resume_authority_observe_delay_ms(1_200);

    let lifted = tokio::time::timeout(LAZY_STEP, handle.resume())
        .await
        .expect("the lift answers");
    mob.service.set_resume_authority_observe_delay_ms(0);
    assert!(
        lifted.is_ok(),
        "the lift reads the slow store and succeeds: {lifted:?}"
    );
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    for (identity, _) in &mob.sessions {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity}"
        );
    }
    assert!(
        mob.materialized().await.is_empty(),
        "the verified lift built no member"
    );
}

/// A verified pass that runs in a later round of the lift's plan still
/// bounds its first read. At restore concurrency 1 the host requires
/// lazy-00, so the plan wakes it in its first round and reaches lazy-01,
/// which it verifies, only once that wake closed, while the lift reported
/// lazy-00's host build, a stage with no inactivity budget. The pass's reads
/// are units of work with the budget (two seconds in this build): lazy-01's
/// read, held for three seconds, is answered as a stall at
/// `member_session_metadata_probe`, the lift goes on, and joining it again
/// answers Ok once the read returned. Before, the pass's first read ran
/// under the host build's stage, so a read that never returned held the lift
/// for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_verified_pass_whose_first_read_hangs_is_answered_as_a_stall() {
    let mob = CrashedMob::new("lazy-lift-later-verify", 2, false, true).await;
    let required = worker(0);
    let verified = worker(1);
    let handle = mob
        .resume(
            mob.builder()
                .with_member_restore_policy(MemberRestorePolicy::OnDemandVerified)
                .with_resume_restore_concurrency(1)
                .with_host_required_members([required.clone()]),
        )
        .await;
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
    // Each read of a member's resume authority now takes three seconds:
    // longer than the budget.
    mob.service.set_resume_authority_observe_delay_ms(3_000);

    let first = tokio::time::timeout(LAZY_STEP, handle.resume())
        .await
        .expect("the lift answers");
    let joined = tokio::time::timeout(LAZY_STEP, handle.resume())
        .await
        .expect("the joined lift answers");
    mob.service.set_resume_authority_observe_delay_ms(0);

    assert!(
        matches!(
            first,
            Err(MobError::LifecycleOperationProgressStalled { stage, .. })
                if stage == "member_session_metadata_probe"
        ),
        "the held read of the later verified pass is answered as a stall: {first:?}"
    );
    assert!(
        joined.is_ok(),
        "the lift, joined again, succeeds once the read returned: {joined:?}"
    );
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    assert_eq!(
        phase_of(&handle, &required),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    assert_eq!(
        phase_of(&handle, &verified),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
}

/// The Eager policy restores every member at start, as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_eager_policy_materializes_every_member() {
    let mob = CrashedMob::new("eager-start", 4, false, false).await;
    let handle = mob
        .resume(
            mob.builder()
                .with_member_restore_policy(MemberRestorePolicy::Eager),
        )
        .await;
    // The boot set's readiness settles its Waking members Live before the
    // actor serves its first command: one round trip reads the phases after
    // it.
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    let mut materialized = mob.materialized().await;
    materialized.sort();
    let mut expected = mob
        .sessions
        .iter()
        .map(|(_, session)| session.clone())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(materialized, expected);
    for (identity, _) in &mob.sessions {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Live),
            "{identity}"
        );
    }
}

/// Reads of dormant members do no work. Member lists, member
/// status and roster reads leave every member Dormant and build nothing;
/// a dormant member's status still reports its peer connectivity and its
/// resolved capabilities, and lists it Active.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_leave_dormant_members_dormant_and_report_them_whole() {
    let mob = CrashedMob::new("reads", 3, true, false).await;
    let handle = mob.resume(mob.builder()).await;
    for _ in 0..3 {
        let listed = handle.list_members().await;
        assert_eq!(listed.len(), mob.sessions.len());
        for entry in &listed {
            assert_eq!(format!("{:?}", entry.status), "Active", "{entry:?}");
        }
        for (identity, session) in &mob.sessions {
            let member = handle
                .get_member(identity)
                .await
                .expect("read the roster")
                .expect("a member");
            assert_eq!(member.bridge_session_id(), Some(session));
            let status = handle
                .member_status(identity)
                .await
                .expect("a dormant member's status");
            assert_eq!(status.current_session_id.as_ref(), Some(session));
            assert!(
                status.peer_connectivity.is_some(),
                "peer connectivity comes from membership: {status:?}"
            );
            assert!(
                status.resolved_capabilities.is_some(),
                "capabilities come from the durable session and profile: {status:?}"
            );
        }
    }
    assert!(mob.materialized().await.is_empty());
    for (identity, _) in &mob.sessions {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity}"
        );
    }
}

/// Under the verified policy a dormant member whose durable session is
/// gone reads Broken at ready, with nothing materialized for it; every other
/// member stays Dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_verified_policy_records_an_unrecoverable_member_broken_without_waking_it() {
    let mob = CrashedMob::new("verified", 3, false, false).await;
    let lost = worker(1);
    mob.service.delete_persisted_session(&mob.session(1)).await;
    let handle = mob
        .resume(
            mob.builder()
                .with_member_restore_policy(MemberRestorePolicy::OnDemandVerified),
        )
        .await;
    assert!(mob.materialized().await.is_empty());
    assert_eq!(phase_of(&handle, &lost), None, "Broken holds no phase");
    assert!(
        handle.restore_diagnostics.read().await.contains_key(&lost),
        "the verdict is recorded as the member's restore failure"
    );
    for identity in [worker(0), worker(2)] {
        assert_eq!(
            phase_of(&handle, &identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity}"
        );
    }
}

/// Under the default policy the same member reads Dormant at ready,
/// and the start read nothing of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_default_policy_leaves_an_unrecoverable_member_dormant_until_touched() {
    let mob = CrashedMob::new("on-demand", 3, false, false).await;
    let lost = worker(1);
    mob.service.delete_persisted_session(&mob.session(1)).await;
    let handle = mob.resume(mob.builder()).await;
    assert!(mob.materialized().await.is_empty());
    assert_eq!(
        phase_of(&handle, &lost),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
    assert!(!handle.restore_diagnostics.read().await.contains_key(&lost));
}

/// Wiring and unwiring two dormant members commits the
/// edge and its event without waking either, and a wired pair restarted
/// lazily keeps its edge with both members Dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wiring_dormant_members_wakes_neither() {
    let mob = CrashedMob::new("dormant-wiring", 4, true, false).await;
    let handle = mob.resume(mob.builder()).await;
    let edges = |handle: &MobHandle| handle.machine_state_watch_rx.borrow().wiring_edges.len();
    assert_eq!(edges(&handle), 1, "the wired pair keeps its edge");
    handle
        .wire(worker(2), worker(3))
        .await
        .expect("wire two dormant members");
    assert_eq!(edges(&handle), 2);
    handle
        .unwire(worker(0), worker(1))
        .await
        .expect("unwire two dormant members");
    assert_eq!(edges(&handle), 1);
    assert!(mob.materialized().await.is_empty());
    for (identity, _) in &mob.sessions {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity}"
        );
    }
}

/// For external peers: wiring and unwiring a dormant member's external
/// peers commit the machine edges and their events without waking it (the
/// verdict owes a member that is not materialized no trust row), and its
/// wake installs the row of the edge the machine still holds, not the row
/// of the edge it no longer holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_peer_wiring_of_a_dormant_member_wakes_nothing() {
    let mob = CrashedMob::new("dormant-external", 2, false, false).await;
    let handle = mob.resume(mob.builder()).await;
    let member = worker(0);
    let kept = test_trusted_peer_descriptor(
        "remote-mob/worker/r28e-kept",
        "inproc://remote-mob/worker/r28e-kept",
    );
    let dropped = test_trusted_peer_descriptor(
        "remote-mob/worker/r28e-dropped",
        "inproc://remote-mob/worker/r28e-dropped",
    );
    for external in [&kept, &dropped] {
        tokio::time::timeout(
            LAZY_STEP,
            handle.wire(member.clone(), PeerTarget::External(external.clone())),
        )
        .await
        .expect("the wire answers")
        .expect("wire a dormant member to an external peer");
    }
    tokio::time::timeout(
        LAZY_STEP,
        handle.unwire(member.clone(), AgentIdentity::from(dropped.name.as_str())),
    )
    .await
    .expect("the unwire answers")
    .expect("unwire a dormant member from an external peer");
    assert!(
        mob.materialized().await.is_empty(),
        "no wiring verb woke the member: {:?}",
        mob.materialized().await
    );
    assert_eq!(
        phase_of(&handle, &member),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
    let edge = |peer: &TrustedPeerDescriptor| {
        dsl::ExternalPeerEdge::new(
            dsl::AgentIdentity::from_domain(&member),
            dsl::ExternalPeerEndpoint::from(peer),
        )
    };
    let edges = handle
        .machine_state_watch_rx
        .borrow()
        .external_peer_edges
        .clone();
    assert!(edges.contains(&edge(&kept)), "{edges:?}");
    assert!(!edges.contains(&edge(&dropped)), "{edges:?}");
    let events = mob.events.replay_all().await.expect("replay the journal");
    let wired = events
        .iter()
        .filter(|event| matches!(event.kind, MobEventKind::ExternalPeerWired { .. }))
        .count();
    let unwired = events
        .iter()
        .filter(|event| matches!(event.kind, MobEventKind::ExternalPeerUnwired { .. }))
        .count();
    assert_eq!((wired, unwired), (2, 1), "each verb is journaled once");

    // Wake the member (its first touch is the wake alone: the worker is not
    // externally addressable, so a member-handle delivery would be refused).
    tokio::time::timeout(LAZY_STEP, handle.ensure_member_live(&member))
        .await
        .expect("the wake answers")
        .expect("the wake makes the member live");
    assert_eq!(
        phase_of(&handle, &member),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    let trusted = mob.service.trusted_peer_names(&mob.session(0)).await;
    assert!(
        trusted.iter().any(|name| name == kept.name.as_str()),
        "the wake installs the held edge's row: {trusted:?}"
    );
    assert!(
        !trusted.iter().any(|name| name == dropped.name.as_str()),
        "the wake installs no row for the edge the machine no longer holds: {trusted:?}"
    );
}

/// The MemberRetired rows the journal holds for `identity`.
async fn retired_rows(events: &Arc<dyn MobEventStore>, identity: &AgentIdentity) -> usize {
    events
        .replay_all()
        .await
        .expect("replay the journal")
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::MemberRetired { agent_identity, .. } if agent_identity == identity
            )
        })
        .count()
}

/// After a restart, a dormant member is retired without waking it.
/// MobMachine retires it archive-only: no runtime retire is routed to the
/// session no runtime holds, its durable session is archived, nothing is
/// materialized, and its wired peer stays dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dormant_member_is_retired_after_a_restart_without_waking() {
    let mob = CrashedMob::new("dormant-retire", 3, true, false).await;
    let handle = mob.resume(mob.builder()).await;
    let retiring = worker(1);
    assert_eq!(
        phase_of(&handle, &retiring),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
    tokio::time::timeout(LAZY_STEP, handle.retire(retiring.clone()))
        .await
        .expect("the retirement answers")
        .expect("the dormant member is retired");
    assert!(
        handle
            .get_member(&retiring)
            .await
            .expect("read the roster")
            .is_none(),
        "the retired member left the roster"
    );
    assert!(
        mob.service.test_is_session_archived(&mob.session(1)).await,
        "its durable session is archived"
    );
    assert!(
        mob.materialized().await.is_empty(),
        "nothing was materialized: {:?}",
        mob.materialized().await
    );
    for identity in [worker(0), worker(2)] {
        assert_eq!(
            phase_of(&handle, &identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity} stays dormant"
        );
    }
    let state = handle.machine_state_watch_rx.borrow().clone();
    assert!(state.wiring_edges.is_empty(), "its edge left with it");
    assert!(
        state.runtime_retire_archive_only.is_empty(),
        "the retirement's terminal closed it"
    );
    assert_eq!(retired_rows(&mob.events, &retiring).await, 1);
}

/// After a restart, a mob whose members are all dormant is destroyed
/// without waking any of them: every member's durable session is archived
/// and nothing is materialized.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mob_of_dormant_members_is_destroyed_after_a_restart_without_waking() {
    let mob = CrashedMob::new("dormant-destroy", 3, true, false).await;
    let handle = mob.resume(mob.builder()).await;
    tokio::time::timeout(LAZY_STEP, handle.destroy())
        .await
        .expect("the destroy answers")
        .expect("the mob of dormant members is destroyed");
    for (identity, session) in &mob.sessions {
        assert!(
            mob.service.test_is_session_archived(session).await,
            "{identity}'s durable session is archived"
        );
    }
    assert!(
        mob.materialized().await.is_empty(),
        "nothing was materialized: {:?}",
        mob.materialized().await
    );
}

/// A retirement that started before a crash and could not archive is
/// recovered archive-only by the restart: its member is not materialized,
/// no runtime retire is routed to its session, and the start's re-drive
/// archives the session and journals the retirement once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retirement_recovered_at_a_restart_completes_archive_only() {
    let definition = with_unique_mob_id(sample_definition(), "recovered-retirement");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create the mob");
    let retiring = worker(0);
    handle
        .spawn_spec(turn_driven(&retiring))
        .await
        .expect("spawn a worker");
    let session = handle
        .resolve_bridge_session_id(&retiring)
        .await
        .expect("a session-backed worker");
    service.set_archive_failure(&session).await;
    handle
        .retire(retiring.clone())
        .await
        .expect_err("the failing archive leaves the retirement started");
    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    let _ = restarted.enable_runtime_adapter();
    crash_stop_and_release_routes(handle).await;
    drop(service);

    let resumed = tokio::time::timeout(
        LAZY_STEP,
        Box::pin(
            MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
                events.clone(),
                runtime_metadata,
            ))
            .with_session_service(restarted.clone())
            .notify_orchestrator_on_resume(false)
            .resume(),
        ),
    )
    .await
    .expect("the restart answers")
    .expect("the restart succeeds");
    tokio::time::timeout(LAZY_STEP, async {
        while resumed
            .get_member(&retiring)
            .await
            .expect("read the roster")
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the start's re-drive completes the recovered retirement");
    assert!(
        restarted.test_is_session_archived(&session).await,
        "the recovered retirement archived the session"
    );
    assert!(
        restarted.recorded_create_requests().await.is_empty(),
        "the restart materialized nothing for the retiring member"
    );
    assert_eq!(retired_rows(&events, &retiring).await, 1);
}

/// A running mob over the mock service, for seats and rotations.
async fn seat_mob(
    label: &str,
    builder: impl FnOnce(MobBuilder) -> MobBuilder,
) -> (MobHandle, Arc<MockSessionService>, Arc<dyn MobEventStore>) {
    let definition = with_unique_mob_id(sample_definition(), label);
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle =
        builder(MobBuilder::new(definition, storage).with_session_service(service.clone()))
            .create()
            .await
            .expect("create the mob");
    (handle, service, events)
}

/// The kinds of the journal's events, in order.
async fn journal_kinds(events: &Arc<dyn MobEventStore>) -> Vec<String> {
    events
        .replay_all()
        .await
        .expect("read the journal")
        .iter()
        .map(|event| match &event.kind {
            MobEventKind::MemberSpawned(spawned) => {
                format!("MemberSpawned({})", spawned.agent_identity)
            }
            MobEventKind::MemberRetired { agent_identity, .. } => {
                format!("MemberRetired({agent_identity})")
            }
            MobEventKind::MemberKickoffUpdated { member, kickoff } => {
                format!("MemberKickoffUpdated({member}, {:?})", kickoff.phase)
            }
            other => format!("{:?}", std::mem::discriminant(other)),
        })
        .collect()
}

/// A seat batch in which one identity's peer id is already
/// held seats the others in one commit and refuses that identity alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_batch_refuses_only_the_identity_whose_peer_id_is_held() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let (handle, service, events) = seat_mob("seat-batch", |builder| {
        builder.with_member_comms_identity_root(keys.path())
    })
    .await;
    let shared = SessionId::new();
    service
        .replace_live_session(Session::with_id(shared.clone()))
        .await;
    let own = SessionId::new();
    service
        .replace_live_session(Session::with_id(own.clone()))
        .await;
    let resume = |identity: &AgentIdentity, session: &SessionId| {
        let mut spec = turn_driven(identity);
        spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
            bridge_session_id: session.clone(),
            resume_from_role: None,
        };
        spec
    };
    let before = journal_kinds(&events).await.len();
    let outcomes = handle
        .seat_dormant_members(vec![
            resume(&worker(0), &shared),
            resume(&worker(1), &shared),
            resume(&worker(2), &own),
        ])
        .await
        .expect("the batch commits");
    assert!(
        matches!(&outcomes[0], DormantSeatOutcome::Seated { session_id, .. } if session_id == &shared),
        "{outcomes:?}"
    );
    assert!(
        matches!(&outcomes[1], DormantSeatOutcome::Refused { .. }),
        "the second identity on the same key is refused: {outcomes:?}"
    );
    assert!(
        matches!(&outcomes[2], DormantSeatOutcome::Seated { session_id, .. } if session_id == &own),
        "{outcomes:?}"
    );
    assert_eq!(
        journal_kinds(&events).await.split_off(before),
        vec![
            format!("MemberSpawned({})", worker(0)),
            format!("MemberSpawned({})", worker(2)),
        ]
    );
    for identity in [worker(0), worker(2)] {
        assert_eq!(
            phase_of(&handle, &identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity}"
        );
    }
    assert_eq!(phase_of(&handle, &worker(1)), None);
    assert!(
        service.recorded_create_requests().await.is_empty(),
        "a resume seat builds nothing"
    );

    // A re-sent batch answers the seated identities AlreadySeated and writes
    // nothing.
    let journal = journal_kinds(&events).await.len();
    let again = handle
        .seat_dormant_members(vec![resume(&worker(0), &shared)])
        .await
        .expect("the re-sent batch");
    assert!(
        matches!(&again[0], DormantSeatOutcome::AlreadySeated { session_id, .. } if session_id == &shared),
        "{again:?}"
    );
    assert_eq!(journal_kinds(&events).await.len(), journal);
}

/// A mob for the seat comparison, with `sessions` live and their comms key
/// pairs stored under `keys`, as a host's import leaves them.
async fn seat_comparison_mob(
    label: &str,
    keys: &std::path::Path,
    sessions: &[&SessionId],
) -> (MobHandle, Arc<dyn MobEventStore>) {
    let (handle, service, events) = seat_mob(label, |builder| {
        builder.with_member_comms_identity_root(keys)
    })
    .await;
    for &session in sessions {
        service
            .replace_live_session(Session::with_id(session.clone()))
            .await;
        meerkat_comms::Keypair::generate()
            .save(&keys.join(session.to_string()))
            .await
            .expect("the session's comms key pair");
    }
    (handle, events)
}

/// The comparison's two chunks: one the machine accepts whole, and a mixed
/// one: a fresh seat, two resume seats of one session (the second one's peer
/// id is the first one's, so the machine refuses it at the commit), an
/// identity seated already, and one more resume seat.
fn seat_comparison_chunks(
    own: &SessionId,
    shared: &SessionId,
    other: &SessionId,
) -> [Vec<SpawnMemberSpec>; 2] {
    let resume = |index: usize, session: &SessionId| {
        let mut spec = turn_driven(&worker(index));
        spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
            bridge_session_id: session.clone(),
            resume_from_role: None,
        };
        spec
    };
    [
        vec![
            turn_driven(&worker(0)),
            resume(1, own),
            turn_driven(&worker(2)),
        ],
        vec![
            turn_driven(&worker(3)),
            resume(4, shared),
            resume(5, shared),
            resume(1, own),
            resume(6, other),
        ],
    ]
}

/// A name for each session the seat comparison meets: the resume sessions
/// by role, and each fresh seat's name-based session by its identity (its id
/// differs from mob to mob).
fn seat_session_names(
    handle: &MobHandle,
    resumed: &[(&SessionId, &str)],
    fresh: &[AgentIdentity],
) -> BTreeMap<String, String> {
    resumed
        .iter()
        .map(|&(session, name)| (session.to_string(), name.to_owned()))
        .chain(fresh.iter().map(|identity| {
            let session = fresh_seat_session_id(
                handle.mob_id(),
                handle.incarnation(),
                identity,
                crate::ids::Generation::INITIAL,
            );
            (session.to_string(), format!("fresh {identity}"))
        }))
        .collect()
}

/// `session` by its name in `names`.
fn session_name(names: &BTreeMap<String, String>, session: &str) -> String {
    names
        .get(session)
        .cloned()
        .unwrap_or_else(|| format!("unnamed {session}"))
}

/// Each seat outcome, its session named.
fn seat_outcome_summaries(
    outcomes: &[DormantSeatOutcome],
    names: &BTreeMap<String, String>,
) -> Vec<String> {
    let named = |session_id: &SessionId| session_name(names, &session_id.to_string());
    outcomes
        .iter()
        .map(|outcome| match outcome {
            DormantSeatOutcome::Seated {
                agent_identity,
                session_id,
            } => format!("{agent_identity} seated on {}", named(session_id)),
            DormantSeatOutcome::AlreadySeated {
                agent_identity,
                session_id,
            } => format!("{agent_identity} already seated on {}", named(session_id)),
            DormantSeatOutcome::SessionArchived {
                agent_identity,
                session_id,
            } => format!("{agent_identity} on archived {}", named(session_id)),
            DormantSeatOutcome::Embodied {
                agent_identity,
                session_id,
            } => format!("{agent_identity} embodied on {}", named(session_id)),
            DormantSeatOutcome::Refused {
                agent_identity,
                error,
            } => format!("{agent_identity} refused: {error:?}"),
        })
        .collect()
}

/// What MobMachine holds for each of `identities`, sessions named.
fn seat_machine_facts(
    handle: &MobHandle,
    identities: &[AgentIdentity],
    names: &BTreeMap<String, String>,
) -> Vec<String> {
    let state = handle.machine_state_watch_rx.borrow();
    identities
        .iter()
        .map(|identity| {
            let id = dsl::AgentIdentity::from_domain(identity);
            format!(
                "{identity}: runtime {:?}, generation {:?}, fence {:?}, session {:?}, \
                 residency {:?}, unborn {}, endpoint {}, peer id {}, host preparation {}, \
                 boot obligation {:?}, wake attempts {:?}, profile {:?}, mode {:?}",
                state.identity_to_runtime.get(&id),
                state.identity_runtime_generations.get(&id),
                state.identity_runtime_fence_tokens.get(&id),
                state
                    .member_session_bindings
                    .get(&id)
                    .map(|session| session_name(names, &session.0)),
                state.member_materialization.get(&id),
                state.member_seat_session_unborn.contains(&id),
                state.member_peer_endpoints.contains_key(&id),
                state.member_peer_ids.contains_key(&id),
                state.member_host_preparation_pending.contains(&id),
                state.member_boot_obligation_owed.get(&id),
                state.member_wake_attempts.get(&id),
                state.member_profile_names.get(&id),
                state.member_runtime_modes.get(&id),
            )
        })
        .collect()
}

/// The journal's rows after its first `from`, sessions named: each
/// `MemberSpawned` row with what a seat writes into it.
async fn seat_journal_rows(
    events: &Arc<dyn MobEventStore>,
    from: usize,
    names: &BTreeMap<String, String>,
) -> Vec<String> {
    events
        .replay_all()
        .await
        .expect("read the journal")
        .iter()
        .skip(from)
        .map(|event| match &event.kind {
            MobEventKind::MemberSpawned(spawned) => format!(
                "MemberSpawned({}): generation {:?}, fence {:?}, runtime {:?}, role {:?}, \
                 mode {:?}, session {:?}, unborn {}, endpoint {}, provenance {:?}",
                spawned.agent_identity,
                spawned.generation,
                spawned.fence_token,
                spawned.agent_runtime_id,
                spawned.role,
                spawned.runtime_mode,
                spawned
                    .bridge_member_ref
                    .as_ref()
                    .and_then(crate::event::MemberRef::bridge_session_id)
                    .map(|session_id| session_name(names, &session_id.to_string())),
                spawned.seat_session_unborn,
                spawned.member_peer_endpoint.is_some(),
                spawned.creation.provenance,
            ),
            other => format!("{:?}", std::mem::discriminant(other)),
        })
        .collect()
}

/// The commit applies a seat chunk's inputs as one batch and its boot plans
/// as another, and applies a batch the machine refuses again one identity
/// at a time. Two mobs seat the same specs, one a chunk per call and the
/// other one spec per call: first a chunk the machine accepts whole, then a
/// mixed one (a fresh seat, resume seats, an identity seated already, and
/// an identity whose peer id another seat of the chunk holds, which the
/// machine refuses at the commit). For these chunks (distinct identities,
/// no spec that owes a kickoff, and every resume session's key pair stored
/// before the seats, so both seats of the shared session carry its one key)
/// the outcomes, the refusal, the journal rows (fence tokens included) and
/// each identity's machine facts are the same. Other chunks can still differ
/// from one spec per call, as they did before the batch: a chunk refuses a
/// name given twice, plans every spec before any of its seats commits, and
/// an embodied spec's spawn takes its fence token between the plan and the
/// commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_chunk_answers_each_entry_as_seating_its_spec_alone_does() {
    let own = SessionId::new();
    let shared = SessionId::new();
    let other = SessionId::new();
    let sessions = [&own, &shared, &other];
    let resumed = [(&own, "own"), (&shared, "shared"), (&other, "other")];
    let fresh = [worker(0), worker(2), worker(3)];
    let identities = (0..7).map(worker).collect::<Vec<_>>();
    let batch_keys = tempfile::tempdir().expect("a comms identity root");
    let alone_keys = tempfile::tempdir().expect("a comms identity root");
    let (batch, batch_events) =
        seat_comparison_mob("seat-chunk-batch", batch_keys.path(), &sessions).await;
    let (alone, alone_events) =
        seat_comparison_mob("seat-chunk-alone", alone_keys.path(), &sessions).await;
    let batch_names = seat_session_names(&batch, &resumed, &fresh);
    let alone_names = seat_session_names(&alone, &resumed, &fresh);
    let batch_from = journal_kinds(&batch_events).await.len();
    let alone_from = journal_kinds(&alone_events).await.len();

    let mut batched = Vec::new();
    for chunk in seat_comparison_chunks(&own, &shared, &other) {
        batched.push(
            batch
                .seat_dormant_members(chunk)
                .await
                .expect("the chunk answers"),
        );
    }
    let mut one_by_one = Vec::new();
    for chunk in seat_comparison_chunks(&own, &shared, &other) {
        let mut answers = Vec::new();
        for spec in chunk {
            answers.extend(
                alone
                    .seat_dormant_members(vec![spec])
                    .await
                    .expect("the spec answers"),
            );
        }
        one_by_one.push(answers);
    }

    assert!(
        matches!(
            &batched[1][2],
            DormantSeatOutcome::Refused {
                error: MobError::MobMachineRejected {
                    context: "dormant_seat",
                    ..
                },
                ..
            }
        ),
        "the machine refuses the seat whose peer id is held: {:?}",
        batched[1]
    );
    let batched = batched
        .iter()
        .map(|outcomes| seat_outcome_summaries(outcomes, &batch_names))
        .collect::<Vec<_>>();
    let one_by_one = one_by_one
        .iter()
        .map(|outcomes| seat_outcome_summaries(outcomes, &alone_names))
        .collect::<Vec<_>>();
    assert_eq!(batched, one_by_one);
    assert_eq!(
        batched[0],
        vec![
            "lazy-00 seated on fresh lazy-00",
            "lazy-01 seated on own",
            "lazy-02 seated on fresh lazy-02",
        ]
    );
    assert_eq!(
        [&batched[1][..2], &batched[1][3..]].concat(),
        vec![
            "lazy-03 seated on fresh lazy-03",
            "lazy-04 seated on shared",
            "lazy-01 already seated on own",
            "lazy-06 seated on other",
        ]
    );

    assert_eq!(
        seat_machine_facts(&batch, &identities, &batch_names),
        seat_machine_facts(&alone, &identities, &alone_names)
    );
    for index in [0, 1, 2, 3, 4, 6] {
        assert_eq!(
            phase_of(&batch, &worker(index)),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{}",
            worker(index)
        );
    }
    assert_eq!(phase_of(&batch, &worker(5)), None);

    assert_eq!(
        seat_journal_rows(&batch_events, batch_from, &batch_names).await,
        seat_journal_rows(&alone_events, alone_from, &alone_names).await
    );
    assert_eq!(
        journal_kinds(&batch_events).await.split_off(batch_from),
        [0, 1, 2, 3, 4, 6]
            .into_iter()
            .map(|index| format!("MemberSpawned({})", worker(index)))
            .collect::<Vec<_>>()
    );
}

/// A seat needs the members' comms transport: without it every spec is
/// refused typed and nothing is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_without_a_comms_identity_source_is_refused_and_writes_nothing() {
    let (handle, service, events) = seat_mob("seat-no-identity-source", |builder| builder).await;
    let session = SessionId::new();
    service
        .replace_live_session(Session::with_id(session.clone()))
        .await;
    let mut spec = turn_driven(&worker(0));
    spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
        bridge_session_id: session,
        resume_from_role: None,
    };
    let before = journal_kinds(&events).await;
    let outcomes = handle
        .seat_dormant_members(vec![spec])
        .await
        .expect("the batch answers");
    assert!(
        matches!(
            &outcomes[0],
            DormantSeatOutcome::Refused {
                error: MobError::WiringError(_),
                ..
            }
        ),
        "{outcomes:?}"
    );
    assert_eq!(journal_kinds(&events).await, before);
}

/// An archived session is never bound: its seat is answered typed and
/// writes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_on_an_archived_session_is_answered_typed() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let (handle, service, events) = seat_mob("seat-archived", |builder| {
        builder.with_member_comms_identity_root(keys.path())
    })
    .await;
    let session = SessionId::new();
    service
        .replace_live_session(Session::with_id(session.clone()))
        .await;
    SessionService::archive(service.as_ref(), &session)
        .await
        .expect("archive the session");
    let mut spec = turn_driven(&worker(0));
    spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
        bridge_session_id: session.clone(),
        resume_from_role: None,
    };
    let before = journal_kinds(&events).await;
    let outcomes = handle
        .seat_dormant_members(vec![spec])
        .await
        .expect("the batch answers");
    assert!(
        matches!(&outcomes[0], DormantSeatOutcome::SessionArchived { session_id, .. } if session_id == &session),
        "{outcomes:?}"
    );
    assert_eq!(journal_kinds(&events).await, before);
}

/// A fresh spec that owes a kickoff is not seated: it
/// is spawned, live, on the seat's name-based session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_autonomous_spec_is_embodied_on_its_name_based_session() {
    let (handle, _service, _events) =
        seat_mob("embodied", MobBuilder::with_network_member_comms).await;
    let identity = worker(0);
    let mut spec = SpawnMemberSpec::new("worker", identity.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
    let outcomes = handle
        .seat_dormant_members(vec![spec])
        .await
        .expect("the batch answers");
    let expected = fresh_seat_session_id(
        handle.mob_id(),
        handle.incarnation(),
        &identity,
        crate::ids::Generation::INITIAL,
    );
    assert!(
        matches!(&outcomes[0], DormantSeatOutcome::Embodied { session_id, .. } if session_id == &expected),
        "{outcomes:?}"
    );
    assert_eq!(
        phase_of(&handle, &identity),
        Some(dsl::MemberMaterializationPhase::Live)
    );
}

/// Whether MobMachine holds `identity`'s seat session unborn.
fn seat_session_unborn(handle: &MobHandle, identity: &AgentIdentity) -> bool {
    crate::runtime::boot_plan::domain_member_seat_session_unborn(
        &handle.machine_state_watch_rx.borrow(),
        identity,
    )
}

/// A fresh turn-driven spec with no initial message is seated under its
/// name-based id and creates nothing: no session, no build, no runtime
/// registration. MobMachine records the session unborn, and the member's
/// first wake creates it, once, through the spawn provision path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_seat_creates_nothing_and_its_first_wake_creates_its_session_once() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let (handle, service, events) = seat_mob("fresh-seat", |builder| {
        builder.with_member_comms_identity_root(keys.path())
    })
    .await;
    let identity = worker(0);
    let before = journal_kinds(&events).await.len();
    let outcomes = handle
        .seat_dormant_members(vec![turn_driven(&identity)])
        .await
        .expect("the batch answers");
    let expected = fresh_seat_session_id(
        handle.mob_id(),
        handle.incarnation(),
        &identity,
        crate::ids::Generation::INITIAL,
    );
    assert!(
        matches!(&outcomes[0], DormantSeatOutcome::Seated { session_id, .. } if session_id == &expected),
        "{outcomes:?}"
    );
    assert!(
        service.recorded_create_requests().await.is_empty(),
        "the seat creates no session"
    );
    assert!(seat_session_unborn(&handle, &identity));
    assert_eq!(
        journal_kinds(&events).await.split_off(before),
        vec![format!("MemberSpawned({identity})")]
    );

    handle
        .ensure_member_live(&identity)
        .await
        .expect("the first wake materializes the member");
    let creates = service
        .recorded_create_requests()
        .await
        .into_iter()
        .filter(|record| record.session_id == expected)
        .collect::<Vec<_>>();
    assert_eq!(creates.len(), 1, "the first wake creates the session once");
    assert!(
        creates[0].runtime_build_mode_session_owned,
        "the create is a registered member build"
    );
    assert!(!seat_session_unborn(&handle, &identity));
}

/// A seat whose journal append failed wrote and created nothing; its retry
/// seats the identity on the same name-based session, still creating nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_retried_after_a_failed_journal_seats_its_fresh_session_once() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let storage = MobStorage::with_events_and_runtime_metadata(
        events.clone(),
        Arc::new(InMemoryMobRuntimeMetadataStore::new()),
    );
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let handle = MobBuilder::new(
        with_unique_mob_id(sample_definition(), "seat-retry"),
        storage,
    )
    .with_session_service(service.clone())
    .with_member_comms_identity_root(keys.path())
    .create()
    .await
    .expect("create the mob");
    let identity = worker(0);
    events.fail_appends_for("MemberSpawned").await;
    let failed = handle
        .seat_dormant_members(vec![turn_driven(&identity)])
        .await;
    assert!(
        failed.is_err(),
        "a store failure fails the batch: {failed:?}"
    );
    assert_eq!(phase_of(&handle, &identity), None, "nothing was seated");
    events.allow_appends_for("MemberSpawned").await;
    let outcomes = handle
        .seat_dormant_members(vec![turn_driven(&identity)])
        .await
        .expect("the retried batch commits");
    let expected = fresh_seat_session_id(
        handle.mob_id(),
        handle.incarnation(),
        &identity,
        crate::ids::Generation::INITIAL,
    );
    assert!(
        matches!(&outcomes[0], DormantSeatOutcome::Seated { session_id, .. } if session_id == &expected),
        "{outcomes:?}"
    );
    assert!(
        service.recorded_create_requests().await.is_empty(),
        "neither attempt created a session"
    );
    assert!(seat_session_unborn(&handle, &identity));
}

/// A member whose session existed (it is not an unborn seat) and is then lost
/// stays lost: its wake finds no successor and records it Broken, and nothing
/// recreates an empty session in its place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_session_of_a_born_member_ends_broken_and_is_not_recreated() {
    let mob = CrashedMob::new("lost-born-session", 2, false, false).await;
    let handle = mob.resume(mob.builder()).await;
    let lost = worker(1);
    assert!(!seat_session_unborn(&handle, &lost));
    mob.service.delete_persisted_session(&mob.session(1)).await;
    let woken = handle.ensure_member_live(&lost).await;
    assert!(woken.is_err(), "a lost session is not recreated: {woken:?}");
    let status = handle
        .member_status(&lost)
        .await
        .expect("the member's status");
    assert_eq!(
        status.status,
        crate::runtime::handle::MobMemberStatus::Broken,
        "{status:?}"
    );
    assert!(
        mob.materialized().await.is_empty(),
        "no session was created in the lost one's place: {:?}",
        mob.materialized().await
    );
}

/// On a service that keeps no durable member sessions the seat creates
/// nothing either, and the member's first wake creates its session fresh,
/// once, under the seat's session id: that service's path is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_seat_on_a_service_without_durable_sessions_is_created_at_its_first_wake() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let definition = with_unique_mob_id(sample_definition(), "seat-non-durable");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    service.keep_no_durable_sessions();
    // A mob over a service without the persistent-session contract is
    // created only on the ephemeral-sessions opt-in.
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service.clone())
        .allow_ephemeral_sessions(true)
        .with_member_comms_identity_root(keys.path())
        .create()
        .await
        .expect("create the mob");
    let identity = worker(0);
    let outcomes = handle
        .seat_dormant_members(vec![turn_driven(&identity)])
        .await
        .expect("the batch answers");
    let DormantSeatOutcome::Seated { session_id, .. } = &outcomes[0] else {
        panic!("the fresh spec is seated: {outcomes:?}");
    };
    let session_id = session_id.clone();
    assert!(service.recorded_create_requests().await.is_empty());
    handle
        .ensure_member_live(&identity)
        .await
        .expect("the first wake materializes the member");
    let creates = service
        .recorded_create_requests()
        .await
        .into_iter()
        .filter(|record| record.session_id == session_id)
        .count();
    assert_eq!(creates, 1, "the first wake creates the session once");
    assert_eq!(
        phase_of(&handle, &identity),
        Some(dsl::MemberMaterializationPhase::Live)
    );
}

/// A woken member rotated into another role takes that role at once: its
/// wake's profile check left no grant behind, so the rotation records its own
/// under its intent. The new role here also flips external addressability,
/// which the rotation's commit holds to its own grant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_after_a_wake_takes_its_new_role_at_once() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let (handle, _service, _events) = seat_mob("rotation-role", |builder| {
        builder.with_member_comms_identity_root(keys.path())
    })
    .await;
    let identity = worker(0);
    handle
        .seat_dormant_members(vec![turn_driven(&identity)])
        .await
        .expect("the batch answers");
    handle
        .ensure_member_live(&identity)
        .await
        .expect("the wake materializes the member");
    assert!(
        !handle
            .machine_state_watch_rx
            .borrow()
            .spawn_profile_authority_profile_names
            .contains_key(&dsl::AgentIdentity::from_domain(&identity)),
        "the wake's profile check records no grant"
    );
    // The lead profile is externally addressable; the worker profile is not.
    let mut lead = SpawnMemberSpec::new("lead", identity.clone());
    lead.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    tokio::time::timeout(LAZY_STEP, handle.rotate_member(lead))
        .await
        .expect("the rotation answers")
        .expect("the rotation commits under its own grant");
    let listed = handle
        .list_members()
        .await
        .into_iter()
        .find(|entry| entry.agent_identity == identity)
        .expect("the member is listed");
    assert_eq!(listed.role, ProfileName::from("lead"));
}

/// A rotation re-driven over its stuck retirement with another role seats
/// that role: the re-drive's profile authority replaces the grant the first
/// attempt recorded under the rotation's intent. The new role also flips
/// external addressability, which the rotation's commit holds to its grant,
/// so a re-drive that kept the first attempt's grant could never commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_redriven_with_another_role_seats_that_role() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let (handle, service, _events) = seat_mob("rotation-redrive-role", |builder| {
        builder.with_member_comms_identity_root(keys.path())
    })
    .await;
    let identity = worker(0);
    handle
        .seat_dormant_members(vec![turn_driven(&identity)])
        .await
        .expect("the batch answers");
    handle
        .ensure_member_live(&identity)
        .await
        .expect("the wake materializes the member");
    let old = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("a session-backed member");
    // The first attempt's retirement starts and sticks at its archive.
    service.set_archive_failure(&old).await;
    tokio::time::timeout(LAZY_STEP, handle.rotate_member(turn_driven(&identity)))
        .await
        .expect("the first rotation answers")
        .expect_err("the first rotation's retirement sticks at its archive");
    service.clear_archive_failure(&old).await;
    // The lead profile is externally addressable; the worker profile is not.
    let mut lead = SpawnMemberSpec::new("lead", identity.clone());
    lead.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    tokio::time::timeout(LAZY_STEP, handle.rotate_member(lead))
        .await
        .expect("the re-driven rotation answers")
        .expect("the re-driven rotation commits under its own grant");
    let listed = handle
        .list_members()
        .await
        .into_iter()
        .find(|entry| entry.agent_identity == identity)
        .expect("the member is listed");
    assert_eq!(listed.role, ProfileName::from("lead"));
    assert_eq!(
        handle
            .machine_state_watch_rx
            .borrow()
            .member_profile_names
            .get(&dsl::AgentIdentity::from_domain(&identity))
            .map(String::as_str),
        Some("lead"),
        "the machine seats the role the re-drive was given"
    );
}

/// A rotation retires the member's incarnation and seats the next
/// one on a new name-based session in one journal batch: MemberRetired is
/// followed at once by the next MemberSpawned, and the old session is
/// archived.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_reseats_the_member_on_a_new_session_in_one_batch() {
    let (handle, service, events) =
        seat_mob("rotation", MobBuilder::with_network_member_comms).await;
    let identity = worker(0);
    handle
        .spawn_spec(turn_driven(&identity))
        .await
        .expect("spawn the member");
    let old = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("a session-backed member");
    let new = tokio::time::timeout(LAZY_STEP, handle.rotate_member(turn_driven(&identity)))
        .await
        .expect("the rotation answers")
        .expect("the rotation succeeds");
    assert_eq!(
        new,
        fresh_seat_session_id(
            handle.mob_id(),
            handle.incarnation(),
            &identity,
            crate::ids::Generation::new(1),
        )
    );
    assert_ne!(new, old);
    assert_eq!(handle.resolve_bridge_session_id(&identity).await, Some(new));
    assert!(service.test_is_session_archived(&old).await);
    let kinds = journal_kinds(&events).await;
    let retired = kinds
        .iter()
        .position(|kind| kind == &format!("MemberRetired({identity})"))
        .expect("the retirement is journaled");
    assert_eq!(
        kinds.get(retired + 1),
        Some(&format!("MemberSpawned({identity})")),
        "the re-seat is journaled with the retirement: {kinds:?}"
    );
    let member = handle
        .get_member(&identity)
        .await
        .expect("read the roster")
        .expect("still a member");
    assert_eq!(member.generation, crate::ids::Generation::new(1));
}

/// 10,000 dormant members restart with zero materializations and
/// every member Dormant, within the coarse bound. Deterministic counters are
/// the assertion; the wall clock is a smoke bound. Run explicitly, in the
/// release profile.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "scale row: run explicitly in the release profile"]
async fn ten_thousand_dormant_members_restart_without_materializing_any() {
    const MEMBERS: usize = 10_000;
    let keys = tempfile::tempdir().expect("a comms identity root");
    let definition = with_unique_mob_id(sample_definition(), "scale");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .with_member_comms_identity_root(keys.path())
        .create()
        .await
        .expect("create the mob");
    let mut specs = Vec::with_capacity(MEMBERS);
    for index in 0..MEMBERS {
        let session = SessionId::new();
        service
            .replace_live_session(Session::with_id(session.clone()))
            .await;
        let mut spec = turn_driven(&AgentIdentity::from(format!("scale-{index:05}")));
        spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
            bridge_session_id: session,
            resume_from_role: None,
        };
        specs.push(spec);
    }
    let outcomes = handle
        .seat_dormant_members(specs)
        .await
        .expect("seat every member");
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome, DormantSeatOutcome::Seated { .. })),
        "every member is seated"
    );
    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    let _ = restarted.enable_runtime_adapter();
    crash_stop_and_release_routes(handle).await;
    drop(service);
    let started = std::time::Instant::now();
    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(restarted.clone())
    .with_member_comms_identity_root(keys.path())
    .notify_orchestrator_on_resume(false)
    .resume()
    .await
    .expect("restart the mob");
    let elapsed = started.elapsed();
    assert!(restarted.recorded_create_requests().await.is_empty());
    assert_eq!(restarted.active_session_count().await, 0);
    let state = resumed.machine_state_watch_rx.borrow().clone();
    assert_eq!(
        state
            .member_materialization
            .values()
            .filter(|phase| matches!(phase, dsl::MemberMaterializationPhase::Dormant))
            .count(),
        MEMBERS
    );
    assert!(
        elapsed < Duration::from_secs(60),
        "the restart took {elapsed:?}"
    );
}

/// What the identities of a seat-cost run are wired to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeatCostWiring {
    /// The sample definition, which declares no edge.
    Undeclared,
    /// `auto_wire_orchestrator` with the orchestrator live before the seats:
    /// one declared edge per seat, whose live end trusts the seated member's
    /// recorded endpoint.
    LiveOrchestrator,
}

/// What one seat-cost run measured.
struct SeatCostRun {
    members: usize,
    /// Writing the import's state: one session and its comms key pair per
    /// identity. Not part of the boot.
    setup: Duration,
    create: Duration,
    seat: Duration,
    chunks: Vec<crate::runtime::handle::seat_chunk_timing::SeatChunkTiming>,
    restart: Duration,
    /// Under [`SeatCostWiring::LiveOrchestrator`]: the orchestrator's first
    /// wake after the restart, whose topology step realizes its edge to every
    /// seated member.
    orchestrator_wake: Option<Duration>,
}

/// Microseconds per seat of `elapsed` over `seats` seats.
fn per_seat_micros(elapsed: Duration, seats: usize) -> f64 {
    elapsed.as_secs_f64() * 1_000_000.0 / seats.max(1) as f64
}

impl SeatCostRun {
    /// The per-seat cost of the last full chunk over the first chunk's, in
    /// total and for the commit alone. A trailing partial chunk is left out:
    /// its fixed per-chunk work over a few seats is not a per-seat cost.
    fn per_seat_growth(&self) -> Option<(f64, f64)> {
        let first = self.chunks.first()?;
        let last = self
            .chunks
            .iter()
            .rev()
            .find(|chunk| chunk.specs == crate::runtime::dormant_seat::DORMANT_SEAT_CHUNK)?;
        let total = |chunk: &crate::runtime::handle::seat_chunk_timing::SeatChunkTiming| {
            per_seat_micros(chunk.plan + chunk.prepare + chunk.commit, chunk.specs)
        };
        Some((
            total(last) / total(first).max(f64::EPSILON),
            per_seat_micros(last.commit, last.specs)
                / per_seat_micros(first.commit, first.specs).max(f64::EPSILON),
        ))
    }

    fn report(&self) {
        let per_seat = per_seat_micros;
        let millis = |elapsed: Duration| elapsed.as_secs_f64() * 1_000.0;
        eprintln!("seat cost: {} identities", self.members);
        eprintln!(
            "  setup (the import's state, outside the boot): {:?}",
            self.setup
        );
        eprintln!("  create the mob: {:?}", self.create);
        eprintln!(
            "  seat: {:?} ({:.0} us per seat)",
            self.seat,
            per_seat(self.seat, self.members)
        );
        let mut seated = 0_usize;
        for (index, chunk) in self.chunks.iter().enumerate() {
            let total = chunk.plan + chunk.prepare + chunk.commit;
            eprintln!(
                "    chunk {:>3}: seats {:>5}..{:<5} plan {:>9.1} ms, prepare {:>9.1} ms, \
                 commit {:>9.1} ms ({:.0} us per seat, commit {:.0} us per seat)",
                index + 1,
                seated + 1,
                seated + chunk.specs,
                millis(chunk.plan),
                millis(chunk.prepare),
                millis(chunk.commit),
                per_seat(total, chunk.specs),
                per_seat(chunk.commit, chunk.specs),
            );
            seated += chunk.specs;
        }
        if let Some((total, commit)) = self.per_seat_growth() {
            eprintln!(
                "  per-seat cost, last full chunk over first: {total:.1}x (commit alone {commit:.1}x)"
            );
        }
        eprintln!("  restart: {:?}", self.restart);
        if let Some(wake) = self.orchestrator_wake {
            eprintln!(
                "  the orchestrator's wake over {} edges: {wake:?}",
                self.members
            );
        }
    }
}

/// Seat `members` identities on a fresh mob as a host's first boot after an
/// import does (Resume seats of sessions that exist, with their comms key
/// pairs), then crash-stop and restart it, timing each phase and each seat
/// chunk's steps. Under [`SeatCostWiring::LiveOrchestrator`] each seat is
/// also wired to the orchestrator, spawned live before the seats, and the
/// run checks every edge, its row and the orchestrator's trust.
async fn seat_cost_run(members: usize, wiring: SeatCostWiring) -> SeatCostRun {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let definition = match wiring {
        SeatCostWiring::Undeclared => sample_definition(),
        SeatCostWiring::LiveOrchestrator => sample_definition_with_auto_wire(),
    };
    let definition = with_unique_mob_id(definition, "seat-cost");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();

    let started = std::time::Instant::now();
    let mut specs = Vec::with_capacity(members);
    for index in 0..members {
        let session = SessionId::new();
        service
            .replace_live_session(Session::with_id(session.clone()))
            .await;
        meerkat_comms::Keypair::generate()
            .save(&keys.path().join(session.to_string()))
            .await
            .expect("the session's comms key pair");
        let mut spec = turn_driven(&AgentIdentity::from(format!("seat-cost-{index:05}")));
        spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
            bridge_session_id: session,
            resume_from_role: None,
        };
        specs.push(spec);
    }
    let setup = started.elapsed();

    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let started = std::time::Instant::now();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .with_member_comms_identity_root(keys.path())
        .create()
        .await
        .expect("create the mob");
    let create = started.elapsed();
    let orchestrator = AgentIdentity::from("seat-cost-orchestrator");
    if wiring == SeatCostWiring::LiveOrchestrator {
        let mut spec = SpawnMemberSpec::new("lead", orchestrator.clone());
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        handle
            .spawn_spec(spec)
            .await
            .expect("spawn the orchestrator live");
    }

    let timings = crate::runtime::handle::seat_chunk_timing::watch(handle.mob_id());
    let started = std::time::Instant::now();
    let outcomes = handle
        .seat_dormant_members(specs)
        .await
        .expect("seat every identity");
    let seat = started.elapsed();
    assert_eq!(outcomes.len(), members);
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome, DormantSeatOutcome::Seated { .. })),
        "every identity is seated"
    );
    let dormant = handle
        .machine_state_watch_rx
        .borrow()
        .member_materialization
        .values()
        .filter(|phase| matches!(phase, dsl::MemberMaterializationPhase::Dormant))
        .count();
    assert_eq!(dormant, members, "every seat is Dormant");
    if wiring == SeatCostWiring::LiveOrchestrator {
        let dsl_orchestrator = dsl::AgentIdentity::from_domain(&orchestrator);
        let (edges, peer_ids) = {
            let state = handle.machine_state_watch_rx.borrow();
            let edges = state.wiring_edges.clone();
            let peer_ids = state
                .member_peer_ids
                .iter()
                .filter(|(identity, _)| **identity != dsl_orchestrator)
                .map(|(_, peer_id)| peer_id.0.clone())
                .collect::<BTreeSet<_>>();
            (edges, peer_ids)
        };
        assert_eq!(edges.len(), members, "one declared edge per seat");
        assert!(
            edges
                .iter()
                .all(|edge| edge.a == dsl_orchestrator || edge.b == dsl_orchestrator),
            "every edge joins the orchestrator"
        );
        let rows = events
            .replay_all()
            .await
            .expect("read the journal")
            .iter()
            .filter(|event| matches!(event.kind, MobEventKind::MembersWired { .. }))
            .count();
        assert_eq!(rows, members, "one MembersWired row per seat");
        let session = handle
            .resolve_bridge_session_id(&orchestrator)
            .await
            .expect("the orchestrator's session");
        let trusted = service
            .comms_runtime(&session)
            .await
            .expect("the orchestrator's comms runtime")
            .peers()
            .await
            .into_iter()
            .map(|peer| peer.peer_id.to_string())
            .collect::<BTreeSet<_>>();
        assert!(
            peer_ids.is_subset(&trusted),
            "the orchestrator trusts every seated member's recorded endpoint"
        );
    }
    // The orchestrator's spawn is the one create: a seat, and the wiring of
    // one, builds and creates nothing.
    assert_eq!(
        service.recorded_create_requests().await.len(),
        usize::from(wiring == SeatCostWiring::LiveOrchestrator),
        "a seat builds and creates nothing"
    );
    let chunks = timings
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        chunks.iter().map(|chunk| chunk.specs).sum::<usize>(),
        members,
        "every seat chunk was timed"
    );

    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    let _ = restarted.enable_runtime_adapter();
    crash_stop_and_release_routes(handle).await;
    drop(service);
    let started = std::time::Instant::now();
    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(restarted.clone())
    .with_member_comms_identity_root(keys.path())
    .notify_orchestrator_on_resume(false)
    .resume()
    .await
    .expect("restart the mob");
    let restart = started.elapsed();
    assert!(restarted.recorded_create_requests().await.is_empty());
    assert_eq!(
        restarted.active_session_count().await,
        0,
        "the restart materializes no seated member"
    );
    // The orchestrator's first wake after the restart realizes its edge to
    // every seated member in its topology step: it then trusts every seated
    // member's recorded endpoint.
    let orchestrator_wake = if wiring == SeatCostWiring::LiveOrchestrator {
        let started = std::time::Instant::now();
        let woken = resumed.ensure_member_live(&orchestrator).await;
        let wake = started.elapsed();
        if let Err(error) = woken {
            panic!("the orchestrator's wake over {members} edges failed after {wake:?}: {error}");
        }
        let dsl_orchestrator = dsl::AgentIdentity::from_domain(&orchestrator);
        let peer_ids = resumed
            .machine_state_watch_rx
            .borrow()
            .member_peer_ids
            .iter()
            .filter(|(identity, _)| **identity != dsl_orchestrator)
            .map(|(_, peer_id)| peer_id.0.clone())
            .collect::<BTreeSet<_>>();
        let session = resumed
            .resolve_bridge_session_id(&orchestrator)
            .await
            .expect("the orchestrator's session");
        let trusted = restarted
            .comms_runtime(&session)
            .await
            .expect("the woken orchestrator's comms runtime")
            .peers()
            .await
            .into_iter()
            .map(|peer| peer.peer_id.to_string())
            .collect::<BTreeSet<_>>();
        assert!(
            peer_ids.is_subset(&trusted),
            "the woken orchestrator trusts every seated member's recorded endpoint"
        );
        Some(wake)
    } else {
        None
    };
    crash_stop_and_release_routes(resumed).await;
    SeatCostRun {
        members,
        setup,
        create,
        seat,
        chunks,
        restart,
        orchestrator_wake,
    }
}

/// First-boot seat cost at scale. A fresh mob seats 1,000 identities, then a
/// second fresh mob 10,000, as a host's first boot after an import does
/// (Resume seats of sessions that exist, with their comms key pairs, through
/// `seat_dormant_members`), and each mob then restarts. The row prints the
/// wall time of each phase and of each seat chunk's plan (on the actor),
/// preparation (off it) and commit (on the actor), so the per-seat cost can
/// be read against how many members are seated already. Deterministic
/// counters are the assertions, with two bounds on the 10,000 run:
/// - seating takes under 10 s. Before the plan probed each chunk on one
///   fork and the commit applied each chunk as one batch, every seat copied
///   the whole machine state about five times, and the 10,000 seats took
///   about four minutes: quadratic in the roster. The bound sits well above
///   the few seconds that remain and far below anything quadratic: a single
///   copy of the state per seat again costs tens of seconds at 10,000;
/// - the restart takes under 60 s, the restart bound.
///
/// The per-seat cost of the last full chunk over the first chunk's is
/// printed with the chunk timings, as a diagnostic, not asserted: it divides
/// one chunk's time by another's, so one slow file read in either moves it
/// by several times, and the regression it would show, a copy of the state
/// per seat, already fails the 10 s bound.
///
/// The session service is a mock: a store's own read per seat comes on top.
/// Run explicitly, in the release profile, with the output shown (nextest
/// `--no-capture`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "scale row: run explicitly in the release profile"]
async fn seating_a_thousand_then_ten_thousand_identities_reports_each_phase() {
    let mut runs = Vec::new();
    for members in [1_000, 10_000] {
        let run = seat_cost_run(members, SeatCostWiring::Undeclared).await;
        run.report();
        runs.push(run);
    }
    let largest = runs.last().expect("the 10,000-identity run");
    assert!(
        largest.seat < Duration::from_secs(10),
        "seating {} identities took {:?}, over the 10 s bound",
        largest.members,
        largest.seat
    );
    assert!(
        largest.restart < Duration::from_secs(60),
        "the restart over {} seated identities took {:?}",
        largest.members,
        largest.restart
    );
}

/// First-boot seat cost at scale with a declared edge per seat: a fresh mob
/// under `auto_wire_orchestrator` spawns its orchestrator live, then seats
/// 10,000 identities, each wired to the orchestrator as a spawn of it would
/// be (the machine edge, its MembersWired row, and the live orchestrator's
/// trust in the seated member's recorded endpoint), restarts, and wakes the
/// orchestrator, whose topology step realizes its 10,000 edges. Each chunk's
/// edges are one prepared machine step and one journal batch, so the wiring
/// adds a fixed number of machine state copies per chunk and a few map
/// reads, one row and one trust install per seat: the 10,000 seats stay
/// under the same 10 s bound as seats with no declared edge, the restart
/// under 60 s, and the orchestrator's wake under 60 s. A wire per edge would
/// copy the whole machine state several times per seat, quadratic in the
/// roster. Run explicitly, in the release profile, with the output shown
/// (nextest `--no-capture`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "scale row: run explicitly in the release profile"]
async fn seating_ten_thousand_identities_wired_to_a_live_orchestrator_stays_under_the_bound() {
    let run = seat_cost_run(10_000, SeatCostWiring::LiveOrchestrator).await;
    run.report();
    assert!(
        run.seat < Duration::from_secs(10),
        "seating {} identities wired to the orchestrator took {:?}, over the 10 s bound",
        run.members,
        run.seat
    );
    assert!(
        run.restart < Duration::from_secs(60),
        "the restart over {} seated, wired identities took {:?}",
        run.members,
        run.restart
    );
    let wake = run
        .orchestrator_wake
        .expect("the run woke the orchestrator after the restart");
    assert!(
        wake < Duration::from_secs(60),
        "the orchestrator's wake over {} edges took {wake:?}, over the 60 s bound",
        run.members
    );
}

/// A crash after a rotation's retirement durably started and
/// before its terminal batch. The restart finds the incarnation Retiring
/// with no rotation intent; the host passes the identity as host-pending (so
/// the plain retirement cannot complete it) and calls `rotate_member` again,
/// which records the rotation on that retirement and seats the next
/// incarnation on the same name-based session the interrupted rotation
/// targeted. The journal never holds the retirement without the re-seat.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_interrupted_after_its_retirement_started_completes_on_redrive() {
    let definition = with_unique_mob_id(sample_definition(), "rotation-crash");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .with_network_member_comms()
        .create()
        .await
        .expect("create the mob");
    let identity = worker(0);
    handle
        .spawn_spec(turn_driven(&identity))
        .await
        .expect("spawn the member");
    let target = fresh_seat_session_id(
        handle.mob_id(),
        handle.incarnation(),
        &identity,
        crate::ids::Generation::new(1),
    );
    let hold =
        crate::runtime::actor::retirement_stage_hold::install(&identity, "retire-runtime-quiesce");
    let rotating = {
        let handle = handle.clone();
        let spec = turn_driven(&identity);
        tokio::spawn(async move { handle.rotate_member(spec).await })
    };
    tokio::time::timeout(LAZY_STEP, hold.entered())
        .await
        .expect("the rotation's retirement starts");
    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    let _ = restarted.enable_runtime_adapter();
    crash_stop_and_release_routes(handle).await;
    hold.release();
    let _ = tokio::time::timeout(LAZY_STEP, rotating).await;
    drop(service);

    let storage = MobStorage::with_events_and_runtime_metadata(events.clone(), runtime_metadata);
    let resumed = tokio::time::timeout(
        LAZY_STEP,
        Box::pin(
            MobBuilder::for_resume(storage)
                .with_session_service(restarted.clone())
                .with_network_member_comms()
                .with_host_pending_members([identity.clone()])
                .notify_orchestrator_on_resume(false)
                .resume(),
        ),
    )
    .await
    .expect("the restart answers")
    .expect("the restart succeeds");
    // The restart's own re-drive of the retirement cannot complete it while
    // the identity is host-pending; the host's re-issued rotation does.
    let deadline = tokio::time::Instant::now() + LAZY_STEP;
    let session = loop {
        match resumed.rotate_member(turn_driven(&identity)).await {
            Ok(session) => break session,
            Err(MobError::LifecycleOperationPending { .. })
                if tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("the re-issued rotation fails: {error}"),
        }
    };
    assert_eq!(
        session, target,
        "the re-drive targets the interrupted rotation's session"
    );
    assert_eq!(
        resumed.resolve_bridge_session_id(&identity).await,
        Some(target)
    );
    let kinds = journal_kinds(&events).await;
    let retirements = kinds
        .iter()
        .enumerate()
        .filter(|(_, kind)| *kind == &format!("MemberRetired({identity})"))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(retirements.len(), 1, "{kinds:?}");
    assert_eq!(
        kinds.get(retirements[0] + 1),
        Some(&format!("MemberSpawned({identity})")),
        "{kinds:?}"
    );
}

/// A member whose comms runtime is missing at start fails alone; the mob
/// starts Running and every other member is live. The failed member's
/// outcome is its own, typed: Broken with the readiness failure as its
/// error, not live. The failing member is wired (to worker 0), so its start
/// needs its comms runtime: an unwired turn-driven member needs no comms
/// drain, and its missing runtime would not matter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_whose_comms_runtime_is_missing_at_start_fails_alone() {
    let mob = CrashedMob::new("missing-comms", 3, true, false).await;
    mob.service.set_missing_comms_runtime(&mob.session(1)).await;
    let handle = mob
        .resume(
            mob.builder()
                .with_member_restore_policy(MemberRestorePolicy::Eager),
        )
        .await;
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    for identity in [worker(0), worker(2)] {
        assert_eq!(
            phase_of(&handle, &identity),
            Some(dsl::MemberMaterializationPhase::Live),
            "{identity}"
        );
    }
    let failed = worker(1);
    assert_ne!(
        phase_of(&handle, &failed),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    let outcome = handle
        .member_status(&failed)
        .await
        .expect("read the failed member's status");
    assert_eq!(
        outcome.status,
        crate::runtime::handle::MobMemberStatus::Broken,
        "{outcome:?}"
    );
    assert!(outcome.error.is_some(), "its failure is typed: {outcome:?}");
}

/// Wait until `predicate` holds, within [`LAZY_STEP`].
async fn wait_for<F, Fut>(what: &str, mut predicate: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + LAZY_STEP;
    loop {
        if predicate().await {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn autonomous(identity: &AgentIdentity, initial_message: Option<&str>) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", identity.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
    if let Some(message) = initial_message {
        spec = spec.with_initial_message(message);
    }
    spec
}

/// How many turns `service` ran on `session` with exactly `prompt`.
async fn turns_with(service: &MockSessionService, session: &SessionId, prompt: &str) -> usize {
    service
        .recorded_start_turn_prompts()
        .await
        .into_iter()
        .filter(|(ran_on, ran)| ran_on == session && ran == prompt)
        .count()
}

/// A mob over stored member keys whose one member (by default an autonomous
/// one, which ran its first life's kickoff) crashed and restarted under the
/// default policy: the member is seated Dormant, nothing materialized.
struct DormantMemberMob {
    keys: tempfile::TempDir,
    handle: MobHandle,
    events: Arc<dyn MobEventStore>,
    runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    /// The restarted service.
    service: Arc<MockSessionService>,
    identity: AgentIdentity,
    /// The first life's session.
    session: SessionId,
}

impl DormantMemberMob {
    async fn new(label: &str) -> Self {
        Self::with_spec(label, autonomous(&worker(0), Some("first life"))).await
    }

    /// [`Self::new`] with the member spawned from `spec`; an autonomous
    /// member runs its first life's kickoff before the crash.
    async fn with_spec(label: &str, spec: SpawnMemberSpec) -> Self {
        let keys = tempfile::tempdir().expect("a comms identity root");
        let definition = with_unique_mob_id(sample_definition(), label);
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        service.set_keep_alive_turns_complete_immediately(true);
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let runtime_metadata = storage.runtime_metadata.clone();
        let handle = MobBuilder::new(definition, storage)
            .with_session_service(service.clone())
            .with_member_comms_identity_root(keys.path())
            .create()
            .await
            .expect("create the mob");
        let identity = spec.identity.clone();
        let autonomous = spec.runtime_mode == Some(crate::MobRuntimeMode::AutonomousHost);
        handle.spawn_spec(spec).await.expect("spawn the member");
        if autonomous {
            handle
                .wait_for_members_kickoff_complete(std::slice::from_ref(&identity), Some(LAZY_STEP))
                .await
                .expect("the first life's kickoff resolves");
        }
        let session = handle
            .resolve_bridge_session_id(&identity)
            .await
            .expect("a session-backed member");
        let mut mob = Self {
            keys,
            handle,
            events,
            runtime_metadata,
            service,
            identity,
            session,
        };
        mob.restart().await;
        assert_eq!(
            phase_of(&mob.handle, &mob.identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "the restart left the member Dormant"
        );
        mob
    }

    /// Crash the mob, restart its session service and resume it under the
    /// default policy.
    async fn restart(&mut self) {
        let handle = self.handle.clone();
        crash_stop_and_release_routes(handle).await;
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        restarted.set_keep_alive_turns_complete_immediately(true);
        self.service = restarted;
        self.handle = tokio::time::timeout(
            LAZY_STEP,
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

    /// The name-based session of the incarnation at `generation`.
    fn session_at(&self, generation: u64) -> SessionId {
        fresh_seat_session_id(
            self.handle.mob_id(),
            self.handle.incarnation(),
            &self.identity,
            crate::ids::Generation::new(generation),
        )
    }
}

/// With a model override: a dormant member's capabilities are those of
/// the model its wake binds, its incarnation's model override over its
/// profile, resolved through the model catalog as a live member's are; the
/// read wakes nothing. Once the member is live, a resolved live projection
/// equals the dormant one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dormant_members_capabilities_follow_its_model_override() {
    const OVERRIDE: &str = "claude-opus-4-8";
    let mut spec = turn_driven(&worker(0));
    spec.model_override = Some(OVERRIDE.to_string());
    let mob = DormantMemberMob::with_spec("override-capabilities", spec).await;
    let provider =
        meerkat_models::infer_provider(OVERRIDE).expect("the override is a catalog model");
    let expected = meerkat_runtime::SessionLlmCapabilitySurface::from_model_profile(
        &meerkat_models::canonical()
            .profile_for(provider, OVERRIDE)
            .expect("the override's catalog profile"),
    )
    .to_wire_resolved();
    let dormant = mob
        .handle
        .member_status(&mob.identity)
        .await
        .expect("the dormant member's status")
        .resolved_capabilities;
    assert_eq!(dormant.as_ref(), Some(&expected));
    assert!(mob.service.recorded_create_requests().await.is_empty());
    assert_eq!(
        phase_of(&mob.handle, &mob.identity),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );

    mob.handle
        .ensure_member_live(&mob.identity)
        .await
        .expect("wake the member");
    let live = mob
        .handle
        .member_status(&mob.identity)
        .await
        .expect("the live member's status")
        .resolved_capabilities;
    if let Some(live) = live {
        assert_eq!(live, expected, "the live projection equals the dormant one");
    }
}

/// A ready wait over dormant autonomous members after a restart returns
/// at once: a seated Dormant member is ready as a member, and the wait
/// materializes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ready_wait_over_dormant_autonomous_members_returns_at_once() {
    let mob = DormantMemberMob::new("dormant-ready-wait").await;
    let ready = tokio::time::timeout(
        Duration::from_secs(5),
        mob.handle.wait_for_members_ready(
            std::slice::from_ref(&mob.identity),
            Some(Duration::from_secs(30)),
        ),
    )
    .await
    .expect("the wait returns at once")
    .expect("the dormant member is ready");
    assert_eq!(ready.len(), 1);
    tokio::time::timeout(
        Duration::from_secs(5),
        mob.handle.wait_for_ready(Some(Duration::from_secs(30))),
    )
    .await
    .expect("the mob-wide wait returns at once")
    .expect("every member is ready");
    assert!(
        mob.service.recorded_create_requests().await.is_empty(),
        "the waits materialized nothing"
    );
    assert_eq!(mob.service.active_session_count().await, 0);
    assert_eq!(
        phase_of(&mob.handle, &mob.identity),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
}

/// The rotation of a dormant autonomous member re-seats it Dormant and
/// records the kickoff its spec owes in the same journal batch; the rotation
/// itself runs no turn. The owed kickoff wakes the next incarnation once and
/// runs exactly once, on its new session, with the spec's initial message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotated_dormant_member_runs_its_owed_kickoff_once_on_its_new_session() {
    let mob = DormantMemberMob::new("rotation-kickoff").await;
    let identity = mob.identity.clone();
    let new = tokio::time::timeout(
        LAZY_STEP,
        mob.handle
            .rotate_member(autonomous(&identity, Some("second life"))),
    )
    .await
    .expect("the rotation answers")
    .expect("the rotation succeeds");
    assert_eq!(new, mob.session_at(1));
    let kinds = journal_kinds(&mob.events).await;
    let retired = kinds
        .iter()
        .position(|kind| kind == &format!("MemberRetired({identity})"))
        .expect("the retirement is journaled");
    assert_eq!(
        kinds.get(retired + 1..retired + 3),
        Some(
            &[
                format!("MemberSpawned({identity})"),
                format!("MemberKickoffUpdated({identity}, Pending)"),
            ][..]
        ),
        "the re-seat and its owed kickoff are journaled with the retirement: {kinds:?}"
    );
    wait_for("the owed kickoff to run on the new session", || async {
        turns_with(&mob.service, &new, "second life").await > 0
    })
    .await;
    let resolved = mob
        .handle
        .wait_for_members_kickoff_complete(std::slice::from_ref(&identity), Some(LAZY_STEP))
        .await
        .expect("the owed kickoff resolves");
    assert_eq!(
        resolved[0].1.kickoff.as_ref().map(|kickoff| kickoff.phase),
        Some(crate::roster::MobMemberKickoffPhase::Started)
    );
    assert_eq!(turns_with(&mob.service, &new, "second life").await, 1);
    assert!(
        !mob.service
            .recorded_start_turn_prompts()
            .await
            .iter()
            .any(|(session, _)| session == &mob.session),
        "the rotation ran no turn on the retired life's session"
    );
    assert_eq!(
        phase_of(&mob.handle, &identity),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    let member = mob
        .handle
        .get_member(&identity)
        .await
        .expect("read the roster")
        .expect("still a member");
    assert_eq!(member.generation, crate::ids::Generation::new(1));
}

/// Across a restart: a kickoff a rotation recorded and that did not run
/// before the process ended (here the next incarnation's wake fails) is owed
/// at the next start, whose boot verdict wakes the member; it runs exactly
/// once, with the initial message the rotation's spec named.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owed_kickoff_runs_once_with_its_prompt_after_a_restart() {
    let mut mob = DormantMemberMob::new("owed-kickoff-restart").await;
    let identity = mob.identity.clone();
    let next = mob.session_at(1);
    // The next incarnation's wake cannot create its seat session (the store
    // cannot serve the write now): the wake ends retryable, the seat stays
    // unborn and the kickoff owed, and nothing is created to roll back. The
    // restarted service creates it.
    mob.service.set_create_session_failure(&next).await;
    let new = tokio::time::timeout(
        LAZY_STEP,
        mob.handle
            .rotate_member(autonomous(&identity, Some("owed across the restart"))),
    )
    .await
    .expect("the rotation answers")
    .expect("the rotation succeeds");
    assert_eq!(new, next);
    wait_for("the failed wake to settle", || async {
        !matches!(
            phase_of(&mob.handle, &identity),
            Some(dsl::MemberMaterializationPhase::Waking)
        )
    })
    .await;
    let dsl_identity = dsl::AgentIdentity::from_domain(&identity);
    assert!(
        mob.handle
            .machine_state_watch_rx
            .borrow()
            .member_kickoff_pending
            .contains(&dsl_identity),
        "the kickoff stays owed while the member cannot wake"
    );
    let before = mob.service.clone();
    assert_eq!(
        turns_with(&before, &new, "owed across the restart").await,
        0
    );

    mob.restart().await;
    wait_for("the owed kickoff to run after the restart", || async {
        turns_with(&mob.service, &new, "owed across the restart").await > 0
    })
    .await;
    mob.handle
        .wait_for_members_kickoff_complete(std::slice::from_ref(&identity), Some(LAZY_STEP))
        .await
        .expect("the owed kickoff resolves");
    assert_eq!(
        turns_with(&mob.service, &new, "owed across the restart").await,
        1
    );
    assert_eq!(
        turns_with(&before, &new, "owed across the restart").await,
        0
    );
}

/// A crash during an owed kickoff's turn (a turn-driven rotation with an
/// initial message): the kickoff was starting when the process ended, so
/// the next start wakes its member for it; its input is admitted again under
/// the incarnation's kickoff key (the restarted runtime holds no copy of it)
/// and is not marked starting a second time. It runs to its end once,
/// resolves Started, and leaves nothing owed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crash_during_a_rotated_kickoff_turn_runs_the_kickoff_once_after_the_restart() {
    let mut mob =
        DormantMemberMob::with_spec("kickoff-crash-mid-turn", turn_driven(&worker(0))).await;
    let identity = mob.identity.clone();
    let prompt = "a kickoff the crash interrupts";
    mob.service.hold_start_turns();
    let mut held = mob.service.held_start_turn_entries();
    let new = tokio::time::timeout(
        LAZY_STEP,
        mob.handle
            .rotate_member(turn_driven(&identity).with_initial_message(prompt)),
    )
    .await
    .expect("the rotation answers")
    .expect("the rotation succeeds");
    tokio::time::timeout(LAZY_STEP, held.wait_for(|entries| *entries >= 1))
        .await
        .expect("the kickoff's turn starts")
        .expect("held turn entries");
    let dsl_identity = dsl::AgentIdentity::from_domain(&identity);
    assert!(
        mob.handle
            .machine_state_watch_rx
            .borrow()
            .member_kickoff_starting
            .contains(&dsl_identity),
        "the kickoff is starting when the process ends"
    );
    assert_eq!(turns_with(&mob.service, &new, prompt).await, 1);

    mob.restart().await;
    wait_for("the kickoff to run after the restart", || async {
        turns_with(&mob.service, &new, prompt).await > 0
    })
    .await;
    mob.handle
        .wait_for_members_kickoff_complete(std::slice::from_ref(&identity), Some(LAZY_STEP))
        .await
        .expect("the kickoff resolves");
    // That wait covers autonomous members only, and the mock counts the
    // kickoff's turn when it starts: this turn-driven member's kickoff can
    // still be Starting here. Read its phase once it settled.
    wait_for("the kickoff to settle after the restart", || async {
        !matches!(
            kickoff_phase_of(&mob.handle, &identity).await,
            Some(
                crate::roster::MobMemberKickoffPhase::Pending
                    | crate::roster::MobMemberKickoffPhase::Starting
                    | crate::roster::MobMemberKickoffPhase::CallbackPending
            )
        )
    })
    .await;
    assert_eq!(
        kickoff_phase_of(&mob.handle, &identity).await,
        Some(crate::roster::MobMemberKickoffPhase::Started)
    );
    assert_eq!(
        turns_with(&mob.service, &new, prompt).await,
        1,
        "the kickoff runs once after the restart"
    );
    let state = mob.handle.machine_state_watch_rx.borrow().clone();
    assert!(
        !state.member_kickoff_pending.contains(&dsl_identity)
            && !state.member_kickoff_starting.contains(&dsl_identity),
        "nothing is owed once the kickoff resolved"
    );
    assert_eq!(
        phase_of(&mob.handle, &identity),
        Some(dsl::MemberMaterializationPhase::Live)
    );
}

/// A crash between an owed kickoff's starting row and the admission of its
/// input (here the starting row is journaled after the next incarnation's
/// wake failed, as `KickoffMarkStarting` journals it before the admission):
/// the next start wakes the member for the starting kickoff, which is not
/// marked again; its input, never admitted before, is admitted under the
/// incarnation's kickoff key, runs once and resolves Started.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kickoff_journaled_starting_but_never_admitted_runs_once_after_a_restart() {
    let mut mob = DormantMemberMob::new("kickoff-starting-unadmitted").await;
    let identity = mob.identity.clone();
    let prompt = "a kickoff starting at the crash";
    let next = mob.session_at(1);
    // The next incarnation's wake cannot create its seat session (the store
    // cannot serve the write now): the wake ends retryable, the seat stays
    // unborn and the kickoff owed, and nothing is created to roll back. The
    // restarted service creates it.
    mob.service.set_create_session_failure(&next).await;
    let new = tokio::time::timeout(
        LAZY_STEP,
        mob.handle
            .rotate_member(autonomous(&identity, Some(prompt))),
    )
    .await
    .expect("the rotation answers")
    .expect("the rotation succeeds");
    assert_eq!(new, next);
    wait_for("the failed wake to settle", || async {
        !matches!(
            phase_of(&mob.handle, &identity),
            Some(dsl::MemberMaterializationPhase::Waking)
        )
    })
    .await;
    let pending = mob
        .events
        .replay_all()
        .await
        .expect("read the journal")
        .into_iter()
        .rev()
        .find_map(|event| match event.kind {
            MobEventKind::MemberKickoffUpdated { member, kickoff } if member == identity => {
                Some(kickoff)
            }
            _ => None,
        })
        .expect("the owed kickoff is journaled");
    assert_eq!(pending.phase, crate::roster::MobMemberKickoffPhase::Pending);
    mob.events
        .append(crate::event::NewMobEvent {
            mob_id: mob.handle.mob_id().clone(),
            timestamp: None,
            kind: MobEventKind::MemberKickoffUpdated {
                member: identity.clone(),
                kickoff: crate::roster::MobMemberKickoffSnapshot {
                    phase: crate::roster::MobMemberKickoffPhase::Starting,
                    updated_at: std::time::SystemTime::now(),
                    ..pending
                },
            },
        })
        .await
        .expect("journal the kickoff's starting row");
    let crashed = mob.service.clone();

    mob.restart().await;
    wait_for("the starting kickoff to run after the restart", || async {
        turns_with(&mob.service, &new, prompt).await > 0
    })
    .await;
    let resolved = mob
        .handle
        .wait_for_members_kickoff_complete(std::slice::from_ref(&identity), Some(LAZY_STEP))
        .await
        .expect("the kickoff resolves");
    assert_eq!(
        resolved[0].1.kickoff.as_ref().map(|kickoff| kickoff.phase),
        Some(crate::roster::MobMemberKickoffPhase::Started)
    );
    assert_eq!(turns_with(&mob.service, &new, prompt).await, 1);
    assert_eq!(turns_with(&crashed, &new, prompt).await, 0);
}

/// The rotation of an identity that is not seated: an identity whose
/// last retirement completed is seated by `rotate_member` as one fresh seat;
/// a spec that owes a kickoff is spawned live on its name-based session, and
/// its kickoff runs once there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_of_a_retired_identity_spawns_it_and_runs_its_kickoff_once() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let (handle, service, _events) = seat_mob("rotation-retired", |builder| {
        builder.with_member_comms_identity_root(keys.path())
    })
    .await;
    service.set_keep_alive_turns_complete_immediately(true);
    let identity = worker(0);
    handle
        .spawn_spec(turn_driven(&identity))
        .await
        .expect("spawn the member");
    handle
        .retire(identity.clone())
        .await
        .expect("retire the member");
    let session = tokio::time::timeout(
        LAZY_STEP,
        handle.rotate_member(autonomous(&identity, Some("a new life"))),
    )
    .await
    .expect("the rotation answers")
    .expect("the rotation seats the identity");
    assert_eq!(
        session,
        fresh_seat_session_id(
            handle.mob_id(),
            handle.incarnation(),
            &identity,
            crate::ids::Generation::new(1),
        )
    );
    handle
        .wait_for_members_kickoff_complete(std::slice::from_ref(&identity), Some(LAZY_STEP))
        .await
        .expect("the kickoff resolves");
    assert_eq!(turns_with(&service, &session, "a new life").await, 1);
}

/// The creation `handle` reads for the rotated member's `session`: the
/// first life's creation id, with Successor provenance naming the first
/// life's session, and the first life's birth.
async fn assert_creation_kept_by_rotation(
    handle: &MobHandle,
    session: &SessionId,
    first_life: &crate::MemberCreationSnapshot,
    when: &str,
) {
    let next = handle
        .member_creation_for_session(session)
        .await
        .unwrap_or_else(|error| panic!("read the next incarnation's creation {when}: {error}"))
        .unwrap_or_else(|| panic!("the next incarnation's creation is recorded {when}"));
    assert_eq!(
        next.creation.creation_id, first_life.creation.creation_id,
        "the rotation keeps the member's creation id {when}"
    );
    assert!(
        matches!(
            &next.creation.provenance,
            crate::MemberCreationProvenance::Successor {
                predecessor_session_id,
                predecessor_creation_id,
                ..
            } if predecessor_session_id == &first_life.session_id
                && Some(*predecessor_creation_id) == first_life.creation.creation_id
        ),
        "the next incarnation is the first life's successor {when}: {:?}",
        next.creation.provenance
    );
    assert_eq!(
        next.birth_cursor, first_life.birth_cursor,
        "the member keeps its birth {when}"
    );
}

/// A rotation (a host's reset of a seated identity) keeps the member's
/// creation, as a respawn does: the next incarnation records the creation id
/// of the retired incarnation's session, with Successor provenance naming
/// that session, although the host's spec names a host-root origin. The
/// record stands after the next incarnation's first wake creates its session
/// and after a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_keeps_the_members_creation_through_its_first_wake_and_a_restart() {
    let host_root = |identity: &AgentIdentity| {
        let mut spec = SpawnMemberSpec::host_root("worker", identity.clone());
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        spec
    };
    let identity = worker(0);
    let mut mob = DormantMemberMob::with_spec("rotation-creation", host_root(&identity)).await;
    let first_life = mob
        .handle
        .member_creation_for_session(&mob.session)
        .await
        .expect("read the first life's creation")
        .expect("the first life recorded its creation");
    assert_eq!(
        first_life.creation.provenance,
        crate::MemberCreationProvenance::Root,
        "a host-root spawn is a root creation"
    );
    assert!(first_life.creation.creation_id.is_some());

    let next = tokio::time::timeout(LAZY_STEP, mob.handle.rotate_member(host_root(&identity)))
        .await
        .expect("the rotation answers")
        .expect("the rotation succeeds");
    assert_eq!(next, mob.session_at(1));
    assert_creation_kept_by_rotation(&mob.handle, &next, &first_life, "after the rotation").await;

    tokio::time::timeout(LAZY_STEP, mob.handle.ensure_member_live(&identity))
        .await
        .expect("the first wake answers")
        .expect("the first wake materializes the next incarnation");
    assert!(
        mob.service
            .recorded_create_requests()
            .await
            .iter()
            .any(|record| record.session_id == next),
        "the first wake created the next incarnation's session"
    );
    assert_creation_kept_by_rotation(&mob.handle, &next, &first_life, "after its first wake").await;

    mob.restart().await;
    assert_creation_kept_by_rotation(&mob.handle, &next, &first_life, "after a restart").await;
}

/// A rotation in a Stopped mob is refused typed before anything of it
/// starts: the identity stays seated on its session at its generation, and
/// nothing is retired or archived.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_in_a_stopped_mob_is_refused_and_retires_nothing() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let (handle, service, events) = seat_mob("stopped-rotation", |builder| {
        builder.with_member_comms_identity_root(keys.path())
    })
    .await;
    let identity = worker(0);
    handle
        .spawn_spec(turn_driven(&identity))
        .await
        .expect("spawn the member");
    let session = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("a session-backed member");
    handle.stop().await.expect("stop the mob");
    let error = tokio::time::timeout(LAZY_STEP, handle.rotate_member(turn_driven(&identity)))
        .await
        .expect("the rotation answers")
        .expect_err("a stopped mob rotates nothing");
    assert!(
        matches!(error, MobError::InvalidTransition { .. }),
        "refused typed: {error:?}"
    );
    assert_eq!(
        handle.resolve_bridge_session_id(&identity).await,
        Some(session.clone())
    );
    let member = handle
        .get_member(&identity)
        .await
        .expect("read the roster")
        .expect("still a member");
    assert_eq!(member.generation, crate::ids::Generation::INITIAL);
    assert!(!service.test_is_session_archived(&session).await);
    let journal = events.replay_all().await.expect("read the journal");
    assert!(
        !journal.iter().any(|event| matches!(
            event.kind,
            MobEventKind::MemberRetirementStarted { .. } | MobEventKind::MemberRetired { .. }
        )),
        "no retirement started"
    );
}

/// Retiring a member while its wake is still provisioning is
/// archive-only, and the archive races that provision. The retirement
/// completes, the wake's attempt closes with it, and the provision the wake
/// was making is rolled back: no session of the member is left live, the
/// session is archived, and no retirement of the member is left stuck.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retiring_a_waking_member_leaves_no_session_live_and_nothing_stuck() {
    let mob = CrashedMob::new("retire-waking", 1, false, false).await;
    let handle = mob.resume(mob.builder()).await;
    let member = worker(0);
    let session = mob.session(0);
    // The wake's provision of the member's session is slow.
    mob.service.set_create_session_delay_ms(1_500);
    let waking = {
        let handle = handle.clone();
        let member = member.clone();
        tokio::spawn(async move { handle.ensure_member_live(&member).await })
    };
    wait_for("the member to be waking", || async {
        phase_of(&handle, &member) == Some(dsl::MemberMaterializationPhase::Waking)
    })
    .await;
    tokio::time::timeout(LAZY_STEP, handle.retire(member.clone()))
        .await
        .expect("the retirement answers")
        .expect("retiring a waking member succeeds");
    // The wake's caller gets an answer; whether it is a refusal or a
    // success that the retirement then ended is not this row's subject.
    let _ = tokio::time::timeout(LAZY_STEP, waking)
        .await
        .expect("the wake's caller is answered");
    assert!(
        handle
            .get_member(&member)
            .await
            .expect("read the roster")
            .is_none(),
        "the member left"
    );
    wait_for("the member's session to be archived", || async {
        mob.service.test_is_session_archived(&session).await
    })
    .await;
    wait_for("no session of the member to be left live", || async {
        mob.service.active_session_count().await == 0
    })
    .await;
    assert_eq!(phase_of(&handle, &member), None);
    let state = handle.machine_state_watch_rx.borrow().clone();
    assert!(
        state.runtime_retire_pending_sessions.is_empty(),
        "no retirement is left pending: {:?}",
        state.runtime_retire_pending_sessions
    );
}

/// One comms drain rule for every readiness path: an explicit resume that
/// rebuilds an unwired turn-driven member whose runtime has no comms
/// requires no drain for it, so the resume succeeds and the member is
/// settled Live, as its wake and its restore readiness settle it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_resume_rebuilds_an_unwired_comms_less_member_without_a_drain() {
    let (handle, service, _events) = seat_mob("resume-comms-less", |builder| builder).await;
    let member = worker(0);
    handle
        .spawn_spec(turn_driven(&member))
        .await
        .expect("spawn the member");
    let session = handle
        .resolve_bridge_session_id(&member)
        .await
        .expect("a session-backed member");
    handle.stop().await.expect("stop the mob");
    // The resume must rebuild the member's session, and the rebuilt runtime
    // has no comms.
    MobSessionService::discard_live_session(service.as_ref(), &session)
        .await
        .expect("discard the live session");
    service.set_missing_comms_runtime(&session).await;
    let builds = service.recorded_create_requests().await.len();
    tokio::time::timeout(LAZY_STEP, handle.resume())
        .await
        .expect("the resume answers")
        .expect("the explicit resume succeeds");
    assert_eq!(handle.status().await.expect("phase"), MobState::Running);
    let status = handle
        .member_status(&member)
        .await
        .expect("the member's status");
    assert_ne!(
        status.status,
        crate::runtime::handle::MobMemberStatus::Broken,
        "{status:?}"
    );
    assert_eq!(
        phase_of(&handle, &member),
        Some(dsl::MemberMaterializationPhase::Live),
        "the resume settled the member Live"
    );
    assert!(
        service.recorded_create_requests().await.len() > builds,
        "the resume rebuilt the member's session"
    );
    assert!(
        service
            .has_live_session(&session)
            .await
            .expect("live session read"),
        "the member serves from its rebuilt session"
    );
}

/// A host build preparer that reads who manages each member's identity
/// through the handle Core hands it before it lets the member's build go
/// ahead: prepared when the read answers, retryable when it is refused. It
/// records every read.
#[derive(Default)]
struct ManagementReadingPreparer {
    reads: std::sync::Mutex<Vec<Result<crate::MemberIdentityManagement, String>>>,
}

impl ManagementReadingPreparer {
    fn reads(&self) -> Vec<Result<crate::MemberIdentityManagement, String>> {
        self.reads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl crate::runtime::MemberBuildPreparer for ManagementReadingPreparer {
    async fn prepare_member_build(
        &self,
        request: crate::runtime::MemberBuildPreparation,
    ) -> Result<crate::runtime::MemberBuildPrepared, crate::runtime::MemberBuildPreparationRefusal>
    {
        let read = request
            .handle
            .identity_management(&request.agent_identity)
            .await
            .map_err(|error| error.to_string());
        self.reads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(read.clone());
        match read {
            Ok(_) => Ok(crate::runtime::MemberBuildPrepared::Prepared),
            Err(reason) => Err(crate::runtime::MemberBuildPreparationRefusal::Retryable { reason }),
        }
    }
}

/// A boot restore hands the host's build preparer the mob's own read-only
/// view, as a wake does (member_wake_rows'
/// `a_wake_hands_the_build_preparer_the_mobs_own_read_view`): the authority
/// the mob's launch minted, the owner's. An Eager start restores the member,
/// and its preparer's one read of who manages the member's identity is
/// answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_boot_restore_hands_the_build_preparer_the_mobs_own_read_view() {
    let definition = with_unique_mob_id(sample_definition(), "boot-preparer-read-view");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage.clone())
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create the mob");
    let member = worker(0);
    handle
        .spawn_spec(turn_driven(&member))
        .await
        .expect("spawn the member");
    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    let _ = restarted.enable_runtime_adapter();
    crash_stop_and_release_routes(handle).await;
    drop(service);

    let preparer = Arc::new(ManagementReadingPreparer::default());
    let resumed = tokio::time::timeout(
        LAZY_STEP,
        Box::pin(
            MobBuilder::for_resume(storage)
                .with_session_service(restarted.clone())
                .notify_orchestrator_on_resume(false)
                .with_member_restore_policy(MemberRestorePolicy::Eager)
                .with_member_build_preparer(preparer.clone())
                .resume(),
        ),
    )
    .await
    .expect("the resume answers")
    .expect("the resume succeeds");
    assert_eq!(
        preparer.reads(),
        vec![Ok(crate::MemberIdentityManagement::CommandDriven)],
        "the boot restore's one preparation read who manages the member's identity, answered"
    );
    // The boot restore leaves the member Waking; its readiness settles it
    // Live before the actor serves its first command: one round trip reads
    // the phase after it.
    assert_eq!(resumed.status().await.unwrap(), MobState::Running);
    assert_eq!(
        phase_of(&resumed, &member),
        Some(dsl::MemberMaterializationPhase::Live)
    );
}

/// The lift of a stopped mob asks the host's build preparer about each member
/// its Eager plan rebuilds, with the mob's own read-only view, before the
/// member's provision, as a wake and a boot restore do: the lift rebuilds
/// the member through the explicit-resume work path, which prepares no build
/// of its own. Its one preparation is answered and the member ends Live.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lift_hands_the_build_preparer_each_member_it_rebuilds() {
    let definition = with_unique_mob_id(sample_definition(), "lift-preparer-read-view");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage.clone())
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create the mob");
    let member = worker(0);
    handle
        .spawn_spec(turn_driven(&member))
        .await
        .expect("spawn the member");
    handle.stop().await.expect("stop the mob");
    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    let _ = restarted.enable_runtime_adapter();
    crash_stop_and_release_routes(handle).await;
    drop(service);

    let preparer = Arc::new(ManagementReadingPreparer::default());
    let resumed = tokio::time::timeout(
        LAZY_STEP,
        Box::pin(
            MobBuilder::for_resume(storage)
                .with_session_service(restarted.clone())
                .notify_orchestrator_on_resume(false)
                .with_member_restore_policy(MemberRestorePolicy::Eager)
                .with_member_build_preparer(preparer.clone())
                .resume(),
        ),
    )
    .await
    .expect("the reconstruction answers")
    .expect("the reconstruction succeeds");
    assert_eq!(resumed.status().await.unwrap(), MobState::Stopped);
    assert!(
        preparer.reads().is_empty(),
        "a stopped start builds nothing: {:?}",
        preparer.reads()
    );

    tokio::time::timeout(LAZY_STEP, resumed.resume())
        .await
        .expect("the lift answers")
        .expect("the lift succeeds");
    assert_eq!(
        preparer.reads(),
        vec![Ok(crate::MemberIdentityManagement::CommandDriven)],
        "the lift's one preparation of the member read who manages its identity, answered"
    );
    resumed.status().await.expect("the mob answers");
    assert_eq!(
        phase_of(&resumed, &member),
        Some(dsl::MemberMaterializationPhase::Live)
    );
}

/// The durable-head status-view reads the session service made of each of
/// `mob`'s workers so far, in spawn order.
fn status_head_reads(mob: &CrashedMob) -> Vec<u64> {
    mob.sessions
        .iter()
        .map(|(_, session)| {
            mob.service
                .durable_reads_of(session)
                .member_status_durable_head
        })
        .collect()
}

/// A Dormant member's status reads like an idle live member's: run state
/// idle, healthy and no in-flight work, from MobMachine's Dormant fact (no
/// run of it is open in a process that has not materialized it, so no
/// execution snapshot is asked for), with its preview and token count from
/// its committed durable head, one read of that head per status read. The
/// read wakes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dormant_members_status_reads_idle_and_healthy_with_its_durable_preview() {
    use crate::runtime::handle::{MemberHealthClass, MemberRunState};
    use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, Usage};

    let mob = CrashedMob::new("dormant-status", 2, false, false).await;
    let target = worker(0);
    let session = mob.session(0);
    let mut answered = mob
        .service
        .persisted_session_clone(&session)
        .await
        .expect("the worker's committed body");
    answered.push(Message::BlockAssistant(BlockAssistantMessage::new(
        vec![AssistantBlock::Text {
            text: "the last durable answer".to_string(),
            meta: None,
        }],
        StopReason::EndTurn,
    )));
    answered.record_cumulative_usage(Usage {
        input_tokens: 30,
        output_tokens: 12,
        ..Usage::default()
    });
    mob.service.replace_persisted_session(answered).await;
    mob.service.serve_durable_head_status_views();
    let handle = mob.resume(mob.builder()).await;
    assert_eq!(
        phase_of(&handle, &target),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );

    let snapshots_before = mob.service.execution_snapshot_calls();
    for read in 1..=2_u64 {
        let status = handle
            .member_status(&target)
            .await
            .expect("a dormant member's status");
        let progress = status
            .progress
            .as_ref()
            .expect("a local member's status carries its progress");
        assert_eq!(progress.run_state, MemberRunState::Idle, "{status:?}");
        assert_eq!(progress.health, MemberHealthClass::Healthy, "{status:?}");
        assert_eq!(progress.in_flight_work, 0, "{status:?}");
        assert_eq!(
            status.output_preview.as_deref(),
            Some("the last durable answer"),
            "{status:?}"
        );
        assert_eq!(status.tokens_used, 42, "{status:?}");
        assert_eq!(status.preview_unavailable, None, "{status:?}");
        assert_eq!(status.current_session_id.as_ref(), Some(&session));
        assert_eq!(
            mob.service
                .durable_reads_of(&session)
                .member_status_durable_head,
            read,
            "one durable-head read per status read"
        );
    }
    assert_eq!(
        mob.service.execution_snapshot_calls(),
        snapshots_before,
        "a Dormant member's progress asks no session for an execution snapshot"
    );

    assert!(
        mob.materialized().await.is_empty(),
        "the status reads built no member: {:?}",
        mob.materialized().await
    );
    assert_eq!(mob.service.active_session_count().await, 0);
    for (identity, _) in &mob.sessions {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity}"
        );
    }
}

/// The all-member waits read a Dormant member's session at most once per
/// call. `collect_completed` reads nothing of it (a Dormant member holds no
/// run to collect, and MobMachine answers that it is not terminal). The
/// kickoff waits poll its status from MobMachine and read only the snapshot
/// they return. The readiness waits take its readiness from MobMachine and
/// read its durable head once for the snapshot they return, which reads
/// like an idle live member's. None of them wakes a member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_all_member_waits_read_a_dormant_member_at_most_once_per_call() {
    use crate::runtime::handle::{MemberHealthClass, MemberRunState};

    let mob = CrashedMob::new("dormant-waits", 3, false, false).await;
    mob.service.serve_durable_head_status_views();
    let handle = mob.resume(mob.builder()).await;
    let members = mob
        .sessions
        .iter()
        .map(|(identity, _)| identity.clone())
        .collect::<Vec<_>>();
    for identity in &members {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity}"
        );
    }

    assert!(handle.collect_completed().await.is_empty());
    assert_eq!(
        status_head_reads(&mob),
        vec![0, 0, 0],
        "collect_completed reads no Dormant member"
    );

    let kicked = tokio::time::timeout(LAZY_STEP, handle.wait_for_kickoff_complete(Some(LAZY_STEP)))
        .await
        .expect("the kickoff wait answers")
        .expect("no Dormant member holds the kickoff wait");
    assert_eq!(kicked.len(), members.len());
    assert_eq!(
        status_head_reads(&mob),
        vec![1, 1, 1],
        "the kickoff poll reads no Dormant member; its snapshots read one head each"
    );
    tokio::time::timeout(
        LAZY_STEP,
        handle.wait_for_members_kickoff_complete(&members, Some(LAZY_STEP)),
    )
    .await
    .expect("the members' kickoff wait answers")
    .expect("no Dormant member holds the members' kickoff wait");
    assert_eq!(status_head_reads(&mob), vec![2, 2, 2]);

    let ready = tokio::time::timeout(LAZY_STEP, handle.wait_for_ready(Some(LAZY_STEP)))
        .await
        .expect("the readiness wait answers")
        .expect("a Dormant member is ready");
    assert_eq!(ready.len(), members.len());
    for (identity, snapshot) in &ready {
        let progress = snapshot
            .progress
            .as_ref()
            .expect("a local member's snapshot carries its progress");
        assert_eq!(progress.run_state, MemberRunState::Idle, "{identity}");
        assert_eq!(progress.health, MemberHealthClass::Healthy, "{identity}");
        assert_eq!(snapshot.preview_unavailable, None, "{identity}");
    }
    assert_eq!(
        status_head_reads(&mob),
        vec![3, 3, 3],
        "readiness reads no Dormant member; its snapshots read one head each"
    );
    tokio::time::timeout(
        LAZY_STEP,
        handle.wait_for_members_ready(&members, Some(LAZY_STEP)),
    )
    .await
    .expect("the members' readiness wait answers")
    .expect("Dormant members are ready");
    assert_eq!(status_head_reads(&mob), vec![4, 4, 4]);

    assert!(
        mob.materialized().await.is_empty(),
        "the waits built no member: {:?}",
        mob.materialized().await
    );
    for identity in &members {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity}"
        );
    }
}

/// A seated member never woken, whose seat session is unborn, reads like a
/// member eagerly created with an empty session before its first turn: run
/// state idle, healthy, no in-flight work, no preview and zero tokens with
/// no `preview_unavailable` marker (a `session_absent` would claim the
/// preview never comes back, where its first wake creates the session).
/// The status read reads nothing of the session, which does not exist yet,
/// and wakes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_never_woken_seats_status_reads_like_an_idle_member_with_an_empty_session() {
    use crate::runtime::handle::{MemberHealthClass, MemberRunState};

    let keys = tempfile::tempdir().expect("a comms identity root");
    let (handle, service, _events) = seat_mob("unborn-status", |builder| {
        builder.with_member_comms_identity_root(keys.path())
    })
    .await;
    service.serve_durable_head_status_views();
    let identity = worker(0);
    let outcomes = handle
        .seat_dormant_members(vec![turn_driven(&identity)])
        .await
        .expect("the batch answers");
    let session = fresh_seat_session_id(
        handle.mob_id(),
        handle.incarnation(),
        &identity,
        crate::ids::Generation::INITIAL,
    );
    assert!(
        matches!(&outcomes[0], DormantSeatOutcome::Seated { session_id, .. } if session_id == &session),
        "{outcomes:?}"
    );
    assert!(seat_session_unborn(&handle, &identity));
    // The seat itself observed the session's resume authority once (the
    // observation that recorded it unborn); the status read adds nothing.
    let durable_before = service.durable_reads_of(&session);
    let reads_before = service.session_reads_of(&session).calls;
    let snapshots_before = service.execution_snapshot_calls();

    let status = handle
        .member_status(&identity)
        .await
        .expect("a seated member's status");
    let progress = status
        .progress
        .as_ref()
        .expect("a local member's status carries its progress");
    assert_eq!(progress.run_state, MemberRunState::Idle, "{status:?}");
    assert_eq!(progress.health, MemberHealthClass::Healthy, "{status:?}");
    assert_eq!(progress.in_flight_work, 0, "{status:?}");
    assert_eq!(status.output_preview, None, "{status:?}");
    assert_eq!(status.tokens_used, 0, "{status:?}");
    assert_eq!(status.preview_unavailable, None, "{status:?}");
    assert_eq!(status.current_session_id.as_ref(), Some(&session));

    assert_eq!(
        service.durable_reads_of(&session),
        durable_before,
        "the status read reads nothing of the unborn session, its resume authority included"
    );
    assert_eq!(service.session_reads_of(&session).calls, reads_before);
    assert_eq!(service.execution_snapshot_calls(), snapshots_before);
    assert!(
        service.recorded_create_requests().await.is_empty(),
        "the status read created no session"
    );
    assert_eq!(
        phase_of(&handle, &identity),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the status read woke nothing"
    );
    assert!(seat_session_unborn(&handle, &identity));
}

/// A mob whose admission closed owns none of its members' sessions for the
/// session-owner wake registry: it wakes no member again, so a session-keyed
/// live operation on a Dormant member's session is answered `NotOwned` and
/// proceeds as for an unowned session, as in a process that never held the
/// mob. The answer wakes nothing and builds nothing.
async fn assert_a_closed_mob_answers_not_owned(
    mob: &CrashedMob,
    handle: &MobHandle,
    reason: crate::MobAdmissionCloseReason,
) {
    use meerkat_core::session_owner_wake::{SessionOwnerWakeOutcome, SessionOwnerWakeRegistry};

    assert_eq!(*handle.admission_closed().borrow(), Some(reason));
    let registry = SessionOwnerWakeRegistry::global();
    for (member, session) in &mob.sessions {
        let answer = registry.wake(session).await;
        assert!(
            matches!(answer, Some(Ok(SessionOwnerWakeOutcome::NotOwned))),
            "{member}: {answer:?}"
        );
        assert_eq!(
            phase_of(handle, member),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{member}: the answer woke nothing"
        );
    }
    assert!(
        mob.materialized().await.is_empty(),
        "nothing was built: {:?}",
        mob.materialized().await
    );
}

/// A stood-down mob (its admission closed unconditionally, as a host's
/// stand-down closes it) answers `NotOwned` for its Dormant members'
/// sessions and wakes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stood_down_mob_answers_not_owned_for_its_dormant_members_sessions() {
    let mob = CrashedMob::new("closed-owner-stand-down", 2, false, false).await;
    let handle = mob.resume(mob.builder()).await;
    let registry = meerkat_core::session_owner_wake::SessionOwnerWakeRegistry::global();
    for (member, session) in &mob.sessions {
        assert_eq!(
            phase_of(&handle, member),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{member}"
        );
        assert!(
            registry.owned(session),
            "{member}: the running mob routes its Dormant member's session"
        );
    }
    handle
        .close_admission(crate::MobAdmissionCloseReason::StandDown)
        .await
        .expect("stand the mob down");
    assert_a_closed_mob_answers_not_owned(&mob, &handle, crate::MobAdmissionCloseReason::StandDown)
        .await;
}

/// A mob whose last public owner left closes admission the same way, and
/// answers `NotOwned` for its Dormant members' sessions too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mob_whose_last_owner_left_answers_not_owned_for_its_dormant_members_sessions() {
    let mob = CrashedMob::new("closed-owner-released", 2, false, false).await;
    let handle = mob.resume(mob.builder()).await;
    assert!(handle.is_public_owner());
    let observer = handle.observer();
    let mut closed = observer.admission_closed();
    drop(handle);
    tokio::time::timeout(LAZY_STEP, closed.wait_for(Option::is_some))
        .await
        .expect("the last owner's release closes admission")
        .expect("the admission watch stays open");
    assert_a_closed_mob_answers_not_owned(
        &mob,
        &observer,
        crate::MobAdmissionCloseReason::LastOwnerReleased,
    )
    .await;
}
