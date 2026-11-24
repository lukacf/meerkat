//! Whether a recorded seat still stands (`MobHandle::seat_now`), and where a
//! member's lineage goes next in its own mob (`MobHandle::member_lineage_step`).
//! They run on the combined candidate: the machine, the actor, the journal and
//! the handle's reads together.

use super::*;
use crate::runtime::{
    LineageClosed, MemberLineageStep, MemberRespawn, MemberSeatKind, MobLineageCause, RecordedSeat,
    SeatNow,
};

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

/// A running mob over the mock durable session service and in-memory
/// storage, with session-scoped member comms identities.
struct SeatMob {
    _keys: tempfile::TempDir,
    handle: MobHandle,
}

impl SeatMob {
    async fn new(label: &str) -> Self {
        let keys = tempfile::tempdir().expect("a comms identity root");
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let handle = MobBuilder::new(
            with_unique_mob_id(sample_definition(), label),
            MobStorage::in_memory(),
        )
        .with_session_service(service)
        .with_member_comms_identity_root(keys.path())
        .create()
        .await
        .expect("create the mob");
        Self {
            _keys: keys,
            handle,
        }
    }

    async fn seat(&self, spec: SpawnMemberSpec) {
        tokio::time::timeout(STEP, self.handle.spawn_spec(spec))
            .await
            .expect("the seat answers")
            .expect("the member is seated");
    }

    async fn session(&self, name: &str) -> SessionId {
        self.handle
            .resolve_bridge_session_id(&id(name))
            .await
            .unwrap_or_else(|| panic!("{name} is bound to a session"))
    }

    /// The seat of `name` as it stands now, with its identity's life marker:
    /// what a seat it made would record of it.
    async fn recorded(&self, name: &str) -> RecordedSeat {
        let session = self.session(name).await;
        let (identity, generation) = self
            .handle
            .member_of_session(&session)
            .await
            .unwrap_or_else(|| panic!("{name} serves its session"));
        RecordedSeat::new(
            self.handle.mob_id().clone(),
            self.handle.incarnation(),
            identity.clone(),
            generation,
            session,
        )
        .with_creator_life(self.handle.identity_life(&identity).expect("life marker"))
    }

    async fn now(&self, recorded: &RecordedSeat) -> SeatNow {
        tokio::time::timeout(STEP, self.handle.seat_now(recorded))
            .await
            .expect("the proof answers")
    }

    async fn retire(&self, name: &str) {
        tokio::time::timeout(STEP, self.handle.retire(id(name)))
            .await
            .expect("the retirement answers")
            .expect("the member retires");
        let deadline = tokio::time::Instant::now() + STEP;
        while self
            .handle
            .get_member(&id(name))
            .await
            .expect("roster read")
            .is_some()
        {
            assert!(tokio::time::Instant::now() < deadline, "{name} left");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// The recorded session serving the recorded identity at the recorded
/// generation is the current seat.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seat_now_answers_current_for_the_recorded_seat() {
    let mob = SeatMob::new("seat-current").await;
    mob.seat(worker_spec("creator")).await;
    let recorded = mob.recorded("creator").await;
    assert_eq!(mob.now(&recorded).await, SeatNow::Current);
}

/// A successor on the same session is the current seat still.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seat_now_answers_current_for_a_same_session_successor() {
    let mob = SeatMob::new("seat-same-session").await;
    mob.seat(worker_spec("creator")).await;
    let recorded = mob.recorded("creator").await;
    tokio::time::timeout(
        STEP,
        mob.handle
            .respawn_member(id("creator"), MemberRespawn::new()),
    )
    .await
    .expect("the respawn answers")
    .expect("a same-session successor is seated");
    assert_eq!(mob.session("creator").await, recorded.owner_session);
    assert_eq!(mob.now(&recorded).await, SeatNow::Current);
}

/// A respawn onto another session keeps the member's life: the seat is no
/// longer current, and it is the same life.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_respawn_onto_a_fresh_session_is_the_same_life_and_not_current() {
    let mob = SeatMob::new("seat-respawn").await;
    mob.seat(worker_spec("creator")).await;
    let recorded = mob.recorded("creator").await;
    tokio::time::timeout(STEP, mob.handle.respawn(id("creator"), None))
        .await
        .expect("the respawn answers")
        .expect("the member is respawned");
    assert_ne!(mob.session("creator").await, recorded.owner_session);
    assert_eq!(mob.now(&recorded).await, SeatNow::SameLife);
}

/// A rotation keeps the member's life.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_is_the_same_life() {
    let mob = SeatMob::new("seat-rotation").await;
    mob.seat(worker_spec("creator").with_entry_custody(true))
        .await;
    let recorded = mob.recorded("creator").await;
    tokio::time::timeout(STEP, mob.handle.rotate_member(worker_spec("creator")))
        .await
        .expect("the rotation answers")
        .expect("the member is rotated");
    assert_ne!(mob.session("creator").await, recorded.owner_session);
    assert_eq!(mob.now(&recorded).await, SeatNow::SameLife);
}

/// A plain member that retired is gone, and so is its seat when another
/// member is seated under its name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_retirement_of_a_plain_member_then_a_seat_under_the_name_is_gone() {
    let mob = SeatMob::new("seat-retired").await;
    mob.seat(worker_spec("creator")).await;
    let recorded = mob.recorded("creator").await;
    mob.retire("creator").await;
    assert_eq!(mob.now(&recorded).await, SeatNow::Gone, "no member");
    mob.seat(worker_spec("creator")).await;
    assert_eq!(
        mob.now(&recorded).await,
        SeatNow::Gone,
        "another member under the name"
    );
}

/// An identity entry outlives a plain retirement of its member: with no
/// member seated it is gone, and its next seat is the same life.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_identity_entry_is_gone_while_unseated_and_the_same_life_when_seated_again() {
    let mob = SeatMob::new("seat-entry").await;
    mob.seat(worker_spec("entry").with_entry_custody(true))
        .await;
    let recorded = mob.recorded("entry").await;
    mob.retire("entry").await;
    assert_eq!(
        mob.now(&recorded).await,
        SeatNow::Gone,
        "an identity entry with no member seated now"
    );
    mob.seat(worker_spec("entry").with_entry_custody(true))
        .await;
    assert_eq!(
        mob.now(&recorded).await,
        SeatNow::SameLife,
        "an identity entry seated again after a plain retirement"
    );
}

/// A mob reset ends every life.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mob_reset_proves_no_seat() {
    let mob = SeatMob::new("seat-reset").await;
    mob.seat(worker_spec("creator")).await;
    let recorded = mob.recorded("creator").await;
    tokio::time::timeout(STEP, mob.handle.reset())
        .await
        .expect("the reset answers")
        .expect("the mob resets");
    mob.seat(worker_spec("creator")).await;
    assert_eq!(mob.now(&recorded).await, SeatNow::Gone);
}

/// A seat recorded in another incarnation of the mob, or in another mob,
/// is not this mob's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_incarnation_of_the_parent_proves_no_seat() {
    let mob = SeatMob::new("seat-incarnation").await;
    mob.seat(worker_spec("creator")).await;
    let recorded = mob.recorded("creator").await;
    let mut earlier = recorded.clone();
    earlier.parent_incarnation = meerkat_core::service::MobIncarnationId::mint();
    assert_eq!(mob.now(&earlier).await, SeatNow::Gone);
    let mut elsewhere = recorded;
    elsewhere.parent_mob_id = MobId::from("another-mob");
    assert_eq!(mob.now(&elsewhere).await, SeatNow::Gone);
}

/// A handle that may not read the mob's state answers unreadable where the
/// answer needs it, never a guess.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_marker_that_cannot_be_read_answers_unreadable() {
    let mob = SeatMob::new("seat-unreadable").await;
    mob.seat(worker_spec("creator")).await;
    let recorded = mob.recorded("creator").await;
    tokio::time::timeout(STEP, mob.handle.respawn(id("creator"), None))
        .await
        .expect("the respawn answers")
        .expect("the member is respawned");
    let unscoped = mob.handle.clone().with_command_authority(
        crate::control_policy::CommandAuthority::principal(
            crate::control_policy::MobControlPrincipal::Unresolved,
        ),
    );
    assert_eq!(
        tokio::time::timeout(STEP, unscoped.seat_now(&recorded))
            .await
            .expect("the proof answers"),
        SeatNow::Unreadable
    );
}

/// A host's own seat is read from its row, and its lineage is its mob's; a
/// seat nobody attested closes; a name no member holds closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_members_seat_and_step_are_read_from_its_row() {
    let mob = SeatMob::new("seat-read").await;
    let mut host = SpawnMemberSpec::host_root("worker", id("host-seated"));
    host.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    mob.seat(host).await;
    mob.seat(worker_spec("unsaid")).await;

    let seat = mob
        .handle
        .member_seat(&id("host-seated"))
        .await
        .expect("a seated member has a seat");
    assert_eq!(seat.kind(), MemberSeatKind::Host);
    assert_eq!(
        mob.handle.member_lineage_step(&id("host-seated")).await,
        MemberLineageStep::Mob(MobLineageCause::HostSeat)
    );
    assert_eq!(
        mob.handle
            .member_seat(&id("unsaid"))
            .await
            .map(|seat| seat.kind()),
        Some(MemberSeatKind::Unproven)
    );
    assert_eq!(
        mob.handle.member_lineage_step(&id("unsaid")).await,
        MemberLineageStep::Closed(LineageClosed::Unproven)
    );
    assert_eq!(mob.handle.member_seat(&id("nobody")).await, None);
    assert_eq!(
        mob.handle.member_lineage_step(&id("nobody")).await,
        MemberLineageStep::Closed(LineageClosed::NoMember)
    );
}

// The seat record the runtime writes at a seat: each kind at its door.

use crate::runtime::{MemberSeat, SeatRecorder, SeatSealer};

async fn seat_of(handle: &MobHandle, name: &str) -> MemberSeat {
    handle
        .member_seat(&id(name))
        .await
        .unwrap_or_else(|| panic!("{name} is seated"))
}

/// The kind word `name`'s last seat row stores, `None` for a row that stores
/// no record.
async fn stored_seat_kind(handle: &MobHandle, name: &str) -> Option<String> {
    handle
        .events
        .replay_all()
        .await
        .expect("read the journal")
        .into_iter()
        .rev()
        .find_map(|event| match event.kind {
            MobEventKind::MemberSpawned(spawned) if spawned.agent_identity == id(name) => Some(
                spawned
                    .seat_lineage()
                    .and_then(|seat| seat.kind_word().map(str::to_string)),
            ),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{name} has no seat row"))
}

async fn creation_of(handle: &MobHandle, name: &str) -> crate::MemberCreationProvenance {
    let session = handle
        .resolve_bridge_session_id(&id(name))
        .await
        .unwrap_or_else(|| panic!("{name} is bound to a session"));
    handle
        .member_creation_for_session(&session)
        .await
        .expect("the creation read answers")
        .unwrap_or_else(|| panic!("{name} has a creation record"))
        .creation
        .provenance
}

/// The host's attested seat is recorded as the host's own, by the runtime's
/// seat step; a seat nobody attested records that who seated it could not be
/// shown. The attestation says nothing of the member's creation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hosts_attested_seat_is_recorded_as_the_hosts_own() {
    let mob = SeatMob::new("seat-attested").await;
    mob.seat(worker_spec("attested").attest_host_seat()).await;
    mob.seat(worker_spec("plain")).await;

    let attested = seat_of(&mob.handle, "attested").await;
    assert_eq!(attested.kind(), MemberSeatKind::Host);
    assert_eq!(attested.recorded_by(), Some(SeatRecorder::Shell));
    assert_eq!(attested.recorded(), None);
    assert_eq!(
        mob.handle.member_lineage_step(&id("attested")).await,
        MemberLineageStep::Mob(MobLineageCause::HostSeat)
    );
    assert_eq!(
        stored_seat_kind(&mob.handle, "attested").await.as_deref(),
        Some("host")
    );

    let plain = seat_of(&mob.handle, "plain").await;
    assert_eq!(plain.kind(), MemberSeatKind::Unproven);
    assert_eq!(plain.recorded_by(), Some(SeatRecorder::Shell));
    assert_eq!(
        mob.handle.member_lineage_step(&id("plain")).await,
        MemberLineageStep::Closed(LineageClosed::Unproven)
    );

    assert_eq!(
        creation_of(&mob.handle, "attested").await,
        crate::MemberCreationProvenance::Unproven,
        "the attestation leaves the creation record as it was"
    );
    assert_eq!(
        creation_of(&mob.handle, "plain").await,
        crate::MemberCreationProvenance::Unproven
    );
}

/// A seat a call made for an agent is never the host's own, whatever the
/// spec attests: at the plain spawn and at the dormant seat, for both forms
/// of acting for an agent. The host's own call at the same entries is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_made_for_an_agent_is_never_the_hosts_own() {
    use crate::control_policy::{CommandAuthority, MobControlPrincipal};
    let mob = SeatMob::new("seat-for-agent").await;
    let lanes = [
        ("lane", CommandAuthority::agent_lane()),
        (
            "principal",
            CommandAuthority::principal_for_agent(MobControlPrincipal::Owner),
        ),
    ];
    for (lane, authority) in lanes {
        let agent = mob.handle.clone().with_command_authority(authority);
        let attested = format!("{lane}-attested");
        tokio::time::timeout(
            STEP,
            agent.spawn_spec(worker_spec(&attested).attest_host_seat()),
        )
        .await
        .expect("the spawn answers")
        .expect("the member is seated");
        let rooted = format!("{lane}-root");
        let mut root = SpawnMemberSpec::host_root("worker", id(&rooted));
        root.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        tokio::time::timeout(STEP, agent.spawn_spec(root))
            .await
            .expect("the spawn answers")
            .expect("the member is seated");
        let dormant = format!("{lane}-dormant");
        let outcomes = tokio::time::timeout(
            STEP,
            agent.seat_dormant_members(vec![worker_spec(&dormant).attest_host_seat()]),
        )
        .await
        .expect("the seat batch answers")
        .expect("the seat batch is taken");
        assert!(
            matches!(&outcomes[0], DormantSeatOutcome::Seated { .. }),
            "{outcomes:?}"
        );
        for name in [&attested, &rooted, &dormant] {
            let seat = seat_of(&mob.handle, name).await;
            assert_eq!(seat.kind(), MemberSeatKind::Unproven, "{name}");
            assert_eq!(seat.recorded_by(), Some(SeatRecorder::Shell), "{name}");
        }
    }
    let outcomes = tokio::time::timeout(
        STEP,
        mob.handle
            .seat_dormant_members(vec![worker_spec("host-dormant").attest_host_seat()]),
    )
    .await
    .expect("the seat batch answers")
    .expect("the seat batch is taken");
    assert!(
        matches!(&outcomes[0], DormantSeatOutcome::Seated { .. }),
        "{outcomes:?}"
    );
    assert_eq!(
        seat_of(&mob.handle, "host-dormant").await.kind(),
        MemberSeatKind::Host
    );
}

/// A member its mob's own spawn tools seat records the calling member's seat,
/// sealed by the mob from MobMachine's own bindings: nothing of the call's
/// arguments. A call with no calling session records that who seated the
/// member could not be shown.
#[tokio::test]
async fn a_member_its_mobs_spawn_tools_seat_records_the_calling_members_seat() {
    let (handle, _service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let parent = id("calling-member");
    handle
        .spawn(ProfileName::from("worker"), parent.clone(), None)
        .await
        .expect("the host seats the calling member");
    let parent_session = handle
        .resolve_bridge_session_id(&parent)
        .await
        .expect("the calling member's session");
    let tools = || -> Arc<dyn AgentToolDispatcher> {
        Arc::new(super::super::tools::MobOperatorToolDispatcher::new(
            handle.clone(),
            true,
            generated_mob_operator_authority_with_scope(
                handle.mob_id().as_str(),
                handle.live_incarnation(),
            ),
        ))
    };
    let dispatcher = super::dispatched_from_session(tools(), parent_session.clone());
    let (_, generation) = handle
        .member_of_session(&parent_session)
        .await
        .expect("the calling member serves its session");
    let mut expected = RecordedSeat::new(
        handle.mob_id().clone(),
        handle.incarnation(),
        parent.clone(),
        generation,
        parent_session.clone(),
    );
    expected.creator_life = Some(handle.identity_life(&parent).expect("life marker"));
    expected.creator_creation = handle
        .member_creation_for_session(&parent_session)
        .await
        .expect("the creation read answers")
        .expect("the calling member has a creation record")
        .creation
        .creation_id;
    assert!(expected.creator_creation.is_some());
    for (tool, name, args) in [
        (
            "spawn_member",
            "single",
            serde_json::json!({"profile":"worker", "member_id":"single"}),
        ),
        (
            "spawn_many_members",
            "batch",
            serde_json::json!({"specs":[{"profile":"worker", "member_id":"batch"}]}),
        ),
    ] {
        let args = serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
        dispatcher
            .dispatch(ToolCallView {
                id: "seat-record",
                name: tool,
                args: &args,
            })
            .await
            .expect("the spawn tool seats the member");
        let seat = seat_of(&handle, name).await;
        assert_eq!(seat.kind(), MemberSeatKind::Member, "{tool}");
        assert_eq!(seat.recorded(), Some(&expected), "{tool}");
        assert_eq!(seat.sealed_by(), Some(SeatSealer::Mob), "{tool}");
        assert_eq!(seat.recorded_by(), Some(SeatRecorder::Shell), "{tool}");
        assert!(!seat.own_turn(), "{tool}");
        assert_eq!(handle.seat_now(&expected).await, SeatNow::Current);
        assert!(
            matches!(
                handle.member_lineage_step(&id(name)).await,
                MemberLineageStep::Member {
                    identity,
                    proof: crate::runtime::MemberLinkProof::Seat(recorded),
                    ..
                } if identity == parent && recorded == expected
            ),
            "{tool}"
        );
    }
    let args = serde_json::value::RawValue::from_string(
        serde_json::json!({"profile":"worker", "member_id":"ownerless"}).to_string(),
    )
    .expect("raw args");
    tools()
        .dispatch(ToolCallView {
            id: "seat-record-ownerless",
            name: "spawn_member",
            args: &args,
        })
        .await
        .expect("the spawn tool seats the member");
    let ownerless = seat_of(&handle, "ownerless").await;
    assert_eq!(ownerless.kind(), MemberSeatKind::Unproven);
    assert_eq!(ownerless.recorded(), None);
    handle.shutdown().await.expect("shutdown");
}

/// A member the mob's own spawn policy seats records the mob's definition.
#[tokio::test]
async fn a_member_the_mobs_spawn_policy_seats_records_the_definition() {
    let target = id("policy-seated");
    let (handle, _) = create_test_mob(sample_definition_with_static_spawn_policy(
        target.as_str(),
        "lead",
    ))
    .await;
    handle
        .submit_work(
            AgentRuntimeId::initial(target.clone()),
            FenceToken::new(0),
            WorkRef::new(),
            WorkSpec::new("seat by policy".to_string(), WorkOrigin::External),
        )
        .await
        .expect("the policy seats the member");
    let seat = seat_of(&handle, "policy-seated").await;
    assert_eq!(seat.kind(), MemberSeatKind::Definition);
    assert_eq!(seat.recorded_by(), Some(SeatRecorder::Shell));
    assert_eq!(
        handle.member_lineage_step(&target).await,
        MemberLineageStep::Mob(MobLineageCause::DefinitionSeat)
    );
    handle.shutdown().await.expect("shutdown");
}

/// A helper a schedule's fire seats records a schedule's seat in both forms,
/// the copy included. A helper the host's own call seats through the same
/// door records that who seated it could not be shown.
#[tokio::test]
async fn a_helper_a_schedule_seats_records_the_schedules_seat_in_both_forms() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_return_exact_run_result(true);
    let options = || HelperOptions {
        role_name: Some(ProfileName::from("worker")),
        runtime_mode: Some(crate::MobRuntimeMode::TurnDriven),
        ..HelperOptions::default()
    };
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            id("source"),
            Some("source context".into()),
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("the host seats the source");

    tokio::time::timeout(
        STEP,
        handle.spawn_scheduled_helper(id("fired"), "do it", options(), "fired-result", 256),
    )
    .await
    .expect("the scheduled helper answers")
    .expect("the scheduled helper runs");
    tokio::time::timeout(
        STEP,
        handle.fork_scheduled_helper(
            &id("source"),
            id("fired-copy"),
            "continue",
            crate::launch::ForkContext::FullHistory,
            options(),
            "fired-copy-result",
            256,
        ),
    )
    .await
    .expect("the scheduled copy answers")
    .expect("the scheduled copy runs");
    tokio::time::timeout(
        STEP,
        handle.spawn_helper(id("called"), "do it", options(), "called-result", 256),
    )
    .await
    .expect("the helper answers")
    .expect("the helper runs");
    tokio::time::timeout(
        STEP,
        handle.fork_helper(
            &id("source"),
            id("called-copy"),
            "continue",
            crate::launch::ForkContext::FullHistory,
            options(),
            "called-copy-result",
            256,
        ),
    )
    .await
    .expect("the copy answers")
    .expect("the copy runs");

    for (name, kind) in [
        ("fired", "scheduled"),
        ("fired-copy", "scheduled"),
        ("called", "unproven"),
        ("called-copy", "unproven"),
    ] {
        assert_eq!(
            stored_seat_kind(&handle, name).await.as_deref(),
            Some(kind),
            "{name}"
        );
    }
    handle.shutdown().await.expect("shutdown");
}

/// A respawn and a rotation keep the seat record of the member's life,
/// whatever their own step carries; the successor's row stores it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_respawn_and_a_rotation_keep_the_seat_record_of_the_life() {
    let mob = SeatMob::new("seat-kept").await;
    mob.seat(
        worker_spec("kept")
            .attest_host_seat()
            .with_entry_custody(true),
    )
    .await;
    let seated = seat_of(&mob.handle, "kept").await;
    assert_eq!(seated.kind(), MemberSeatKind::Host);

    // A plain member's respawn takes a fresh session.
    mob.seat(worker_spec("kept-plain").attest_host_seat()).await;
    let plain = seat_of(&mob.handle, "kept-plain").await;
    assert_eq!(plain.kind(), MemberSeatKind::Host);
    let plain_session = mob.session("kept-plain").await;
    tokio::time::timeout(STEP, mob.handle.respawn(id("kept-plain"), None))
        .await
        .expect("the respawn answers")
        .expect("the member is respawned");
    assert_ne!(mob.session("kept-plain").await, plain_session);
    assert_eq!(seat_of(&mob.handle, "kept-plain").await, plain);
    assert_eq!(
        stored_seat_kind(&mob.handle, "kept-plain").await.as_deref(),
        Some("host"),
        "the successor's row stores the record"
    );

    tokio::time::timeout(STEP, mob.handle.respawn(id("kept"), None))
        .await
        .expect("the respawn answers")
        .expect("the member is respawned");
    assert_eq!(seat_of(&mob.handle, "kept").await, seated);
    assert_eq!(
        stored_seat_kind(&mob.handle, "kept").await.as_deref(),
        Some("host"),
        "the successor's row stores the record"
    );

    let second = mob.session("kept").await;
    tokio::time::timeout(STEP, mob.handle.rotate_member(worker_spec("kept")))
        .await
        .expect("the rotation answers")
        .expect("the member is rotated");
    assert_ne!(mob.session("kept").await, second);
    assert_eq!(seat_of(&mob.handle, "kept").await, seated);
    assert_eq!(
        stored_seat_kind(&mob.handle, "kept").await.as_deref(),
        Some("host")
    );

    tokio::time::timeout(
        STEP,
        mob.handle.respawn_member(id("kept"), MemberRespawn::new()),
    )
    .await
    .expect("the respawn answers")
    .expect("a same-session successor is seated");
    assert_eq!(seat_of(&mob.handle, "kept").await, seated);
}

/// A seat under a name whose member's life ended records anew, from its own
/// seat's observations.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_under_a_name_whose_life_ended_records_anew() {
    let mob = SeatMob::new("seat-anew").await;
    mob.seat(worker_spec("again").attest_host_seat()).await;
    assert_eq!(
        seat_of(&mob.handle, "again").await.kind(),
        MemberSeatKind::Host
    );
    mob.retire("again").await;
    assert_eq!(mob.handle.member_seat(&id("again")).await, None);
    mob.seat(worker_spec("again")).await;
    assert_eq!(
        seat_of(&mob.handle, "again").await.kind(),
        MemberSeatKind::Unproven
    );
}

/// A resume launch does not change who seated the member: the host's
/// attested seat on a session it names to resume is the host's own, an
/// unattested one shows nothing, and a seat made for an agent is never the
/// host's. What the creation record says of a resumed session is the same
/// with and without the attestation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_launch_does_not_change_who_seated_the_member() {
    use crate::control_policy::{CommandAuthority, MobControlPrincipal};
    let definition = with_unique_mob_id(sample_definition(), "seat-resumed");
    let names = ["attested", "plain", "for-agent"];

    // Sessions built in another deployment.
    let exported_keys = tempfile::tempdir().expect("the exporting comms identity root");
    let exporting = Arc::new(MockSessionService::new());
    let _exporting_adapter = exporting.enable_runtime_adapter();
    let exporter = MobBuilder::new(definition.clone(), MobStorage::in_memory())
        .with_session_service(exporting.clone())
        .with_member_comms_identity_root(exported_keys.path())
        .create()
        .await
        .expect("create the exporting mob");
    let mut sessions = Vec::new();
    for name in names {
        exporter
            .spawn_spec(worker_spec(name))
            .await
            .expect("spawn the member");
        sessions.push(
            exporter
                .resolve_bridge_session_id(&id(name))
                .await
                .expect("a session-backed member"),
        );
    }
    crash_stop_and_release_routes(exporter).await;

    let service = Arc::new(exporting.cold_restart_preserving_durable_state().await);
    let _adapter = service.enable_runtime_adapter();
    let keys = tempfile::tempdir().expect("an empty comms identity root");
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service.clone())
        .with_member_comms_identity_root(keys.path())
        .create()
        .await
        .expect("create the importing mob");
    let resume = |name: &str, session: &SessionId| {
        let mut spec = worker_spec(name);
        spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
            bridge_session_id: session.clone(),
            resume_from_role: None,
        };
        spec
    };
    let seated = |outcomes: Vec<DormantSeatOutcome>| {
        assert!(
            outcomes
                .iter()
                .all(|outcome| matches!(outcome, DormantSeatOutcome::Seated { .. })),
            "{outcomes:?}"
        );
    };
    seated(
        handle
            .seat_dormant_members(vec![
                resume("attested", &sessions[0]).attest_host_seat(),
                resume("plain", &sessions[1]),
            ])
            .await
            .expect("the seat batch answers"),
    );
    let agent = handle
        .clone()
        .with_command_authority(CommandAuthority::principal_for_agent(
            MobControlPrincipal::Owner,
        ));
    seated(
        agent
            .seat_dormant_members(vec![resume("for-agent", &sessions[2]).attest_host_seat()])
            .await
            .expect("the seat batch answers"),
    );

    assert_eq!(
        seat_of(&handle, "attested").await.kind(),
        MemberSeatKind::Host
    );
    assert_eq!(
        seat_of(&handle, "plain").await.kind(),
        MemberSeatKind::Unproven
    );
    assert_eq!(
        seat_of(&handle, "for-agent").await.kind(),
        MemberSeatKind::Unproven
    );
    let creation = |session: &SessionId| {
        let session = session.clone();
        let handle = handle.clone();
        async move {
            handle
                .member_creation_for_session(&session)
                .await
                .expect("the creation read answers")
                .map(|snapshot| std::mem::discriminant(&snapshot.creation.provenance))
        }
    };
    assert_eq!(
        creation(&sessions[0]).await,
        creation(&sessions[1]).await,
        "the attestation changes nothing of what a resume records as the creation"
    );
    assert_ne!(
        creation(&sessions[0]).await,
        Some(std::mem::discriminant(
            &crate::MemberCreationProvenance::Root
        )),
        "a resumed session is no root creation"
    );
}

/// A spec built from wire arguments attests nothing by itself, and the
/// host's attestation on it stands only off a call made for an agent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_wire_spawns_seat_is_the_hosts_only_off_the_agent_lane() {
    use crate::control_policy::{CommandAuthority, MobControlPrincipal};
    let mob = SeatMob::new("seat-wire").await;
    let wire = |name: &str| {
        SpawnMemberSpec::from_wire(
            "worker".to_string(),
            name.to_string(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
    };
    mob.seat(wire("wire-plain")).await;
    mob.seat(wire("wire-attested").attest_host_seat()).await;
    let agent = mob
        .handle
        .clone()
        .with_command_authority(CommandAuthority::principal_for_agent(
            MobControlPrincipal::Owner,
        ));
    tokio::time::timeout(
        STEP,
        agent.spawn_spec(wire("wire-for-agent").attest_host_seat()),
    )
    .await
    .expect("the spawn answers")
    .expect("the member is seated");
    for (name, kind) in [
        ("wire-plain", MemberSeatKind::Unproven),
        ("wire-attested", MemberSeatKind::Host),
        ("wire-for-agent", MemberSeatKind::Unproven),
    ] {
        assert_eq!(seat_of(&mob.handle, name).await.kind(), kind, "{name}");
    }
}

/// A copy's lineage goes on at its source only while the source's mob holds,
/// under that name, a member of the creation the copy records: the source
/// itself, also after its respawn. A later holder of a retired source's name
/// gives its copy nothing.
#[tokio::test]
async fn a_later_holder_of_a_retired_sources_name_gives_its_copy_nothing() {
    use crate::runtime::{MemberLink, MemberLinkProof};
    let (handle, _) = create_test_mob(sample_definition()).await;
    let source = id("copied-source");
    let rooted = || {
        let mut spec = SpawnMemberSpec::host_root("worker", source.clone());
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        spec.tool_access_policy = Some(meerkat_core::ops::ToolAccessPolicy::ReadOnly);
        spec
    };
    handle
        .spawn_spec(rooted())
        .await
        .expect("the host seats the source");
    let source_session = handle
        .resolve_bridge_session_id(&source)
        .await
        .expect("the source's session");
    wait_for_fork_source_settled(&handle, &source_session).await;
    let source_creation = handle
        .member_creation_for_session(&source_session)
        .await
        .expect("the creation read answers")
        .expect("the source has a creation record")
        .creation
        .creation_id
        .expect("a creation id");
    handle
        .fork_member(&source, worker_spec("the-copy"), None)
        .await
        .expect("the host copies the source");

    assert_eq!(
        seat_of(&handle, "the-copy").await.kind(),
        MemberSeatKind::ForkOf
    );
    assert_eq!(
        handle.member_lineage_step(&id("the-copy")).await,
        MemberLineageStep::Member {
            mob_id: handle.mob_id().clone(),
            identity: source.clone(),
            link: MemberLink::ForkOf,
            proof: MemberLinkProof::Creation(source_creation),
        }
    );
    assert_eq!(
        handle.creation_now(&source, source_creation).await,
        SeatNow::SameLife
    );
    assert_eq!(
        handle.creation_now(&id("the-copy"), source_creation).await,
        SeatNow::Gone,
        "a member of another creation"
    );
    assert_eq!(
        handle.creation_now(&id("nobody"), source_creation).await,
        SeatNow::Gone
    );

    tokio::time::timeout(STEP, handle.respawn(source.clone(), None))
        .await
        .expect("the respawn answers")
        .expect("the source is respawned");
    assert_eq!(
        handle.creation_now(&source, source_creation).await,
        SeatNow::SameLife,
        "a respawn keeps the member's creation"
    );

    tokio::time::timeout(STEP, handle.retire(source.clone()))
        .await
        .expect("the retirement answers")
        .expect("the source retires");
    let deadline = tokio::time::Instant::now() + STEP;
    while handle
        .get_member(&source)
        .await
        .expect("roster read")
        .is_some()
    {
        assert!(tokio::time::Instant::now() < deadline, "the source left");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        handle.creation_now(&source, source_creation).await,
        SeatNow::Gone
    );
    handle
        .spawn_spec(rooted())
        .await
        .expect("the host seats another member under the name");
    assert_eq!(
        handle.creation_now(&source, source_creation).await,
        SeatNow::Gone,
        "a later holder of the name is no member of the source's creation"
    );
    handle.shutdown().await.expect("shutdown");
}
