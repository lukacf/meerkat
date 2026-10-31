#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! Operator instructions on a mob: the rows of the mob runtime.
//!
//! A host writes a slot's body at a revision for a target (every member, a
//! profile's members, or one member). MobMachine commits the record, journals
//! it, and owes every member whose snapshot changes; the mob records each
//! owed member's whole snapshot in the member's session: through the session
//! while it is live, store-only while it is dormant, never waking or
//! materializing the member, and in the first build of a session it mints
//! (a member added later, an unborn seat's first wake). These rows drive the
//! mob through its handle and read the member sessions' committed targets
//! from the mock session service, which records what a persistent owner
//! records.

use super::*;
use crate::operator_instruction::{
    MemberInstructionBlockReason, MemberInstructionDeliveryState, MemberInstructionResidency,
    MemberInstructionStatus, OperatorInstructionCommittedEvent, OperatorInstructionDisposition,
    OperatorInstructionError, OperatorInstructionStatusQuery, OperatorInstructionTarget,
    OperatorInstructionWrite,
};
use crate::runtime::DormantSeatOutcome;
use meerkat_core::{
    InstructionEffect, InstructionKey, InstructionNamespace, InstructionRevision,
    InstructionSetErrorCode, InstructionSlot, InstructionTargetOwner, InstructionTargetRecord,
    SessionInstructionChange, SessionInstructionWrite,
};

const STEP: Duration = Duration::from_mins(1);
const FORMAL: &str = "Answer formally.";
const BRIEF: &str = "Keep every answer brief.";

fn slot(key: &str) -> InstructionSlot {
    InstructionSlot {
        namespace: InstructionNamespace::new("app.example").expect("namespace"),
        key: InstructionKey::new(key).expect("key"),
    }
}

fn revision(value: u64) -> InstructionRevision {
    InstructionRevision::new(value).expect("revision")
}

fn write(
    key: &str,
    at: u64,
    body: &str,
    target: OperatorInstructionTarget,
) -> OperatorInstructionWrite {
    OperatorInstructionWrite {
        slot: slot(key),
        revision: revision(at),
        body: body.to_string(),
        content_sha256: None,
        target,
    }
}

fn profile(name: &str) -> OperatorInstructionTarget {
    OperatorInstructionTarget::Profile {
        profile: ProfileName::from(name),
    }
}

fn turn_driven(role: &str, identity: &AgentIdentity) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new(role, identity.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

/// Wait until `check` holds, within [`STEP`].
async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + STEP;
    loop {
        if check().await {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} never happened"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The slot keys a session's committed target holds Active, with their
/// revision.
async fn active_slots(service: &MockSessionService, session: &SessionId) -> Vec<(String, u64)> {
    service
        .committed_instruction_target(session)
        .await
        .map(|state| {
            state
                .slots()
                .iter()
                .filter(|slot| slot.effect == InstructionEffect::Active)
                .map(|slot| (slot.key.as_str().to_owned(), slot.revision.get()))
                .collect()
        })
        .unwrap_or_default()
}

/// The generation a session's committed target holds.
async fn target_generation(service: &MockSessionService, session: &SessionId) -> Option<u64> {
    service
        .committed_instruction_target(session)
        .await
        .map(|state| state.target_generation())
}

/// The writes a service made to `session`'s document for mob deliveries.
fn writes_to(service: &MockSessionService, session: &SessionId) -> usize {
    service
        .instruction_records()
        .iter()
        .filter(|(recorded, _, wrote)| recorded == session && *wrote)
        .count()
}

async fn status(handle: &MobHandle) -> crate::operator_instruction::OperatorInstructionStatusPage {
    handle
        .operator_instruction_status(OperatorInstructionStatusQuery::default())
        .await
        .expect("the status read answers")
}

async fn member_status(
    handle: &MobHandle,
    identity: &AgentIdentity,
) -> Option<MemberInstructionStatus> {
    status(handle)
        .await
        .members
        .into_iter()
        .find(|member| &member.agent_identity == identity)
}

/// Wait until the member's session recorded the mob's current snapshot.
async fn current(handle: &MobHandle, identity: &AgentIdentity) {
    eventually(&format!("{identity} records its snapshot"), || async move {
        member_status(handle, identity).await.is_some_and(|member| {
            member.delivery.state == MemberInstructionDeliveryState::Current
                && member.delivery.recorded_generation.is_some()
        })
    })
    .await;
}

/// A mob over the mock session service with turn-driven members spawned
/// live, one per (name, role).
struct InstructionMob {
    handle: MobHandle,
    service: Arc<MockSessionService>,
    events: Arc<dyn MobEventStore>,
    runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    members: Vec<(AgentIdentity, SessionId)>,
    concurrency: Option<usize>,
}

impl InstructionMob {
    async fn new(label: &str, members: &[(&str, &str)]) -> Self {
        Self::with_concurrency(label, members, None).await
    }

    async fn with_concurrency(
        label: &str,
        members: &[(&str, &str)],
        concurrency: Option<usize>,
    ) -> Self {
        Self::with_storage(label, members, concurrency, MobStorage::in_memory()).await
    }

    /// [`Self::new`] over `events`, the mob's journal.
    async fn with_events(
        label: &str,
        members: &[(&str, &str)],
        events: Arc<dyn MobEventStore>,
    ) -> Self {
        Self::with_storage(label, members, None, MobStorage::with_events(events)).await
    }

    async fn with_storage(
        label: &str,
        members: &[(&str, &str)],
        concurrency: Option<usize>,
        storage: MobStorage,
    ) -> Self {
        let definition = with_unique_mob_id(sample_definition(), label);
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let events = storage.events.clone();
        let runtime_metadata = storage.runtime_metadata.clone();
        let mut builder =
            MobBuilder::new(definition, storage).with_session_service(service.clone());
        if let Some(concurrency) = concurrency {
            builder = builder.with_member_instruction_delivery_concurrency(concurrency);
        }
        let handle = builder.create().await.expect("create the mob");
        let mut mob = Self {
            handle,
            service,
            events,
            runtime_metadata,
            members: Vec::new(),
            concurrency,
        };
        for (name, role) in members {
            mob.spawn(name, role).await;
        }
        mob
    }

    async fn spawn(&mut self, name: &str, role: &str) -> (AgentIdentity, SessionId) {
        let identity = AgentIdentity::from(*name);
        self.handle
            .spawn_spec(turn_driven(role, &identity))
            .await
            .expect("spawn the member");
        let session = self
            .handle
            .resolve_bridge_session_id(&identity)
            .await
            .expect("a session-backed member");
        self.members.push((identity.clone(), session.clone()));
        (identity, session)
    }

    fn member(&self, index: usize) -> (AgentIdentity, SessionId) {
        self.members[index].clone()
    }

    /// Spawn an autonomous member whose session its host keeps: the mob's
    /// archive authority holds no record of the session, so the member's
    /// retirement releases only its runtime, and the host keeps the session.
    async fn spawn_host_kept(&mut self, name: &str) -> (AgentIdentity, SessionId) {
        let identity = AgentIdentity::from(name);
        let mut spec = SpawnMemberSpec::new("worker", identity.clone());
        spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
        self.handle
            .spawn_spec(spec)
            .await
            .expect("spawn the member");
        let session = self
            .handle
            .resolve_bridge_session_id(&identity)
            .await
            .expect("a session-backed member");
        self.service.set_archive_authority_unknown(&session).await;
        self.service.set_archive_not_found(&session).await;
        self.members.push((identity.clone(), session.clone()));
        (identity, session)
    }

    /// A durable session outside the mob, named for `identity` in this mob,
    /// whose own host holds its operator-instruction target (one Active
    /// slot), with no live actor: a session an attach adopts.
    async fn host_instructed_session(&self, identity: &AgentIdentity) -> SessionId {
        let created = self
            .service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "a host's own session".to_string().into(),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    comms_name: Some(
                        crate::runtime::actor::render_member_comms_name(
                            self.handle.mob_id().as_str(),
                            "worker",
                            identity.as_str(),
                        )
                        .expect("comms name"),
                    ),
                    mob_member_binding: None,
                    ..Default::default()
                }),
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                labels: None,
            })
            .await
            .expect("the host's session");
        let session = created.session_id;
        MobSessionService::discard_live_session(self.service.as_ref(), &session)
            .await
            .expect("discard the creating actor");
        self.service
            .session_comms_names
            .write()
            .await
            .remove(&session);
        let mut persisted = self.service.persisted_sessions.write().await;
        let document = persisted
            .get_mut(&session)
            .expect("the host's durable session");
        document
            .set_session_instruction(SessionInstructionWrite {
                slot: slot("host"),
                revision: revision(1),
                change: SessionInstructionChange::Activate {
                    body: BRIEF.to_string(),
                    content_sha256: None,
                },
            })
            .expect("the host writes its own instruction");
        session
    }

    /// Crash the mob and restart its session service with its durable state
    /// only: every session the mob created is dormant in the new process.
    async fn crash(self) -> CrashedInstructionMob {
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        crash_stop_and_release_routes(self.handle).await;
        CrashedInstructionMob {
            service: restarted,
            events: self.events,
            runtime_metadata: self.runtime_metadata,
            members: self.members,
            concurrency: self.concurrency,
        }
    }
}

struct CrashedInstructionMob {
    service: Arc<MockSessionService>,
    events: Arc<dyn MobEventStore>,
    runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    members: Vec<(AgentIdentity, SessionId)>,
    concurrency: Option<usize>,
}

impl CrashedInstructionMob {
    /// Resume the mob over the restarted service: every member is dormant.
    async fn resume(&self) -> MobHandle {
        let mut builder = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
            self.events.clone(),
            self.runtime_metadata.clone(),
        ))
        .with_session_service(self.service.clone())
        .notify_orchestrator_on_resume(false);
        if let Some(concurrency) = self.concurrency {
            builder = builder.with_member_instruction_delivery_concurrency(concurrency);
        }
        tokio::time::timeout(STEP, Box::pin(builder.resume()))
            .await
            .expect("the resume answers")
            .expect("the resume succeeds")
    }

    /// The sessions the restarted service built an agent for.
    async fn materialized(&self) -> Vec<SessionId> {
        self.service
            .recorded_create_requests()
            .await
            .into_iter()
            .map(|record| record.session_id)
            .collect()
    }
}

/// A profile target reaches exactly the members of that profile: their
/// sessions record the slot, and no other member's session is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_target_reaches_only_that_profile() {
    let mob = InstructionMob::new(
        "oi-profile",
        &[
            ("oi-profile-w1", "worker"),
            ("oi-profile-w2", "worker"),
            ("oi-profile-l1", "lead"),
        ],
    )
    .await;
    let receipt = mob
        .handle
        .set_operator_instruction(write("tone", 1, FORMAL, profile("worker")))
        .await
        .expect("the write commits");
    assert_eq!(
        receipt.disposition,
        OperatorInstructionDisposition::Committed
    );
    assert_eq!(receipt.matched_members, 2, "two workers match the profile");
    for index in 0..2 {
        let (identity, session) = mob.member(index);
        current(&mob.handle, &identity).await;
        assert_eq!(
            active_slots(&mob.service, &session).await,
            vec![("tone".to_string(), 1)],
            "{identity} records the slot"
        );
    }
    let (lead, lead_session) = mob.member(2);
    assert!(
        mob.service
            .committed_instruction_target(&lead_session)
            .await
            .is_none(),
        "the lead's session holds no target"
    );
    assert_eq!(writes_to(&mob.service, &lead_session), 0);
    let lead_status = member_status(&mob.handle, &lead)
        .await
        .expect("the lead's row");
    assert!(
        lead_status.slots.iter().all(|slot| slot.state
            == crate::operator_instruction::MemberInstructionSlotState::NotTargeted),
        "the record does not target the lead: {lead_status:?}"
    );
}

/// A member added after a write is born with the current set: its new
/// session carries the snapshot from its first build, and the mob's
/// delivery to it only confirms it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_added_later_is_born_with_the_current_set() {
    let mut mob = InstructionMob::new("oi-born", &[("oi-born-w1", "worker")]).await;
    mob.handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the write commits");
    let (later, session) = mob.spawn("oi-born-w2", "worker").await;
    assert_eq!(
        target_generation(&mob.service, &session).await,
        Some(1),
        "the new session is born with the mob's generation"
    );
    assert_eq!(
        active_slots(&mob.service, &session).await,
        vec![("tone".to_string(), 1)]
    );
    current(&mob.handle, &later).await;
    assert_eq!(
        writes_to(&mob.service, &session),
        0,
        "the born session needs no delivery write"
    );
}

/// An unborn seat is owed but not delivered: nothing wakes it and no
/// session is created for it. Its first wake creates the session with the
/// snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unborn_seat_records_at_first_wake() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let definition = with_unique_mob_id(sample_definition(), "oi-unborn");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service.clone())
        .with_member_comms_identity_root(keys.path())
        .create()
        .await
        .expect("create the mob");
    let identity = AgentIdentity::from("oi-unborn-worker");
    let outcomes = handle
        .seat_dormant_members(vec![turn_driven("worker", &identity)])
        .await
        .expect("the seat batch answers");
    let session = match &outcomes[0] {
        DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
        other => panic!("the fresh spec is seated: {other:?}"),
    };
    handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the write commits");
    let (handle_ref, identity_ref) = (&handle, &identity);
    eventually(
        "the unborn seat is reported unborn and owed",
        || async move {
            member_status(handle_ref, identity_ref)
                .await
                .is_some_and(|member| {
                    member.residency == MemberInstructionResidency::Unborn
                        && matches!(
                            member.delivery.state,
                            MemberInstructionDeliveryState::Owed { .. }
                        )
                })
        },
    )
    .await;
    assert!(
        service.recorded_create_requests().await.is_empty(),
        "nothing created the unborn seat's session"
    );
    assert_eq!(writes_to(&service, &session), 0, "nothing delivered to it");

    handle
        .ensure_member_live(&identity)
        .await
        .expect("the first wake creates the session");
    assert_eq!(
        active_slots(&service, &session).await,
        vec![("tone".to_string(), 1)],
        "the session is born with the snapshot"
    );
    current(&handle, &identity).await;
    assert_eq!(writes_to(&service, &session), 0);
}

/// A dormant member's session records the snapshot store-only: no session
/// is built and the member stays dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dormant_member_is_recorded_without_wake_or_materialization() {
    let mob = InstructionMob::new(
        "oi-dormant",
        &[("oi-dormant-w1", "worker"), ("oi-dormant-w2", "worker")],
    )
    .await;
    let crashed = mob.crash().await;
    let handle = crashed.resume().await;
    for (identity, _) in &crashed.members {
        assert_eq!(
            handle.member_materialization_for_test(identity),
            Some(crate::machines::mob_machine::MemberMaterializationPhase::Dormant)
        );
    }
    handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the write commits");
    for (identity, session) in &crashed.members {
        current(&handle, identity).await;
        assert_eq!(
            active_slots(&crashed.service, session).await,
            vec![("tone".to_string(), 1)]
        );
        assert_eq!(
            handle.member_materialization_for_test(identity),
            Some(crate::machines::mob_machine::MemberMaterializationPhase::Dormant),
            "{identity} stays dormant"
        );
        assert_eq!(
            member_status(&handle, identity)
                .await
                .expect("its row")
                .residency,
            MemberInstructionResidency::Dormant
        );
    }
    assert!(
        crashed.materialized().await.is_empty(),
        "no member session was built: {:?}",
        crashed.materialized().await
    );
}

/// A retarget revokes the slot for members that left the selector and
/// activates it for members that joined it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retarget_revokes_members_that_left_the_selector() {
    let mob = InstructionMob::new(
        "oi-retarget",
        &[("oi-retarget-w1", "worker"), ("oi-retarget-l1", "lead")],
    )
    .await;
    let (worker, worker_session) = mob.member(0);
    let (lead, lead_session) = mob.member(1);
    mob.handle
        .set_operator_instruction(write("tone", 1, FORMAL, profile("worker")))
        .await
        .expect("the write commits");
    current(&mob.handle, &worker).await;
    assert_eq!(
        active_slots(&mob.service, &worker_session).await,
        vec![("tone".to_string(), 1)]
    );

    mob.handle
        .set_operator_instruction(write("tone", 2, FORMAL, profile("lead")))
        .await
        .expect("the retarget commits");
    current(&mob.handle, &worker).await;
    current(&mob.handle, &lead).await;
    assert!(
        active_slots(&mob.service, &worker_session).await.is_empty(),
        "the worker's session revokes the slot"
    );
    let worker_target = mob
        .service
        .committed_instruction_target(&worker_session)
        .await
        .expect("the worker's target");
    assert!(
        worker_target
            .slots()
            .iter()
            .any(|slot| slot.key.as_str() == "tone"
                && slot.revision.get() == 2
                && slot.effect == InstructionEffect::Absent),
        "the revoke is the record's new revision: {worker_target:?}"
    );
    assert_eq!(
        active_slots(&mob.service, &lead_session).await,
        vec![("tone".to_string(), 2)]
    );
}

/// A retired member keeps no delivery fact; seated again, it is a new
/// member for delivery.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retire_clears_member_delivery_facts() {
    let mut mob = InstructionMob::new("oi-retire", &[("oi-retire-w1", "worker")]).await;
    let (identity, _) = mob.member(0);
    mob.handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the write commits");
    current(&mob.handle, &identity).await;
    mob.handle
        .retire(identity.clone())
        .await
        .expect("retire the member");
    assert!(
        member_status(&mob.handle, &identity).await.is_none(),
        "a retired member has no delivery row"
    );
    mob.members.clear();
    let (respawned, session) = mob.spawn("oi-retire-w1", "worker").await;
    assert_eq!(respawned, identity);
    current(&mob.handle, &respawned).await;
    assert_eq!(
        active_slots(&mob.service, &session).await,
        vec![("tone".to_string(), 1)]
    );
}

/// A restart replays the desired records and the obligations: a member owed
/// before the crash is owed after it, and its session records the snapshot
/// once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_replay_restores_desired_and_owed() {
    let mob = InstructionMob::new(
        "oi-restart",
        &[("oi-restart-w1", "worker"), ("oi-restart-w2", "worker")],
    )
    .await;
    mob.handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the first write commits");
    for (identity, _) in mob.members.clone() {
        current(&mob.handle, &identity).await;
    }
    let _held = mob.service.hold_instruction_records();
    mob.handle
        .set_operator_instruction(write("style", 1, BRIEF, profile("worker")))
        .await
        .expect("the second write commits");
    let crashed = mob.crash().await;
    let gate = crashed.service.hold_instruction_records();
    let handle = crashed.resume().await;
    let page = status(&handle).await;
    assert_eq!(page.generation, 2, "the generation is replayed");
    assert_eq!(
        page.desired
            .iter()
            .map(|record| (record.slot.key.as_str().to_owned(), record.revision.get()))
            .collect::<Vec<_>>(),
        vec![("style".to_string(), 1), ("tone".to_string(), 1)],
        "the desired records are replayed"
    );
    for member in &page.members {
        assert_eq!(member.delivery.recorded_generation, Some(1), "{member:?}");
        assert!(
            matches!(
                member.delivery.state,
                MemberInstructionDeliveryState::Owed { generation: 2, .. }
            ),
            "the obligation is replayed: {member:?}"
        );
    }
    gate.add_permits(16);
    for (identity, session) in &crashed.members {
        current(&handle, identity).await;
        assert_eq!(
            active_slots(&crashed.service, session).await,
            vec![("style".to_string(), 1), ("tone".to_string(), 1)]
        );
        assert_eq!(
            writes_to(&crashed.service, session),
            1,
            "{identity} is written once"
        );
    }
}

/// A reset clears the records and moves the generation past the one the mob
/// had, across a restart too: a session never sees the generation go back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reset_keeps_the_generation_high_water() {
    let mut mob = InstructionMob::new("oi-reset", &[("oi-reset-w1", "worker")]).await;
    let (identity, _) = mob.member(0);
    mob.handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the write commits");
    current(&mob.handle, &identity).await;
    mob.handle.reset().await.expect("reset the mob");
    let page = status(&mob.handle).await;
    assert_eq!(page.generation, 2, "the reset moves the generation on");
    assert!(page.desired.is_empty(), "the reset clears the records");
    mob.members.clear();
    let (_, session) = mob.spawn("oi-reset-w2", "worker").await;
    assert_eq!(
        target_generation(&mob.service, &session).await,
        Some(2),
        "a session born after the reset holds the moved generation"
    );
    let receipt = mob
        .handle
        .set_operator_instruction(write("tone", 1, BRIEF, OperatorInstructionTarget::Mob))
        .await
        .expect("a write after the reset commits");
    assert_eq!(receipt.generation, 3);

    let crashed = mob.crash().await;
    let handle = crashed.resume().await;
    assert_eq!(
        status(&handle).await.generation,
        3,
        "a restart keeps the high-water"
    );
    let receipt = handle
        .set_operator_instruction(write("tone", 2, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("a write after the restart commits");
    assert_eq!(receipt.generation, 4);
}

/// Stale, duplicate, conflicting, unknown and terminal writes answer typed,
/// and change nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_duplicate_conflict_and_terminal_refusals_are_typed() {
    let mob = InstructionMob::new("oi-refusals", &[("oi-refusals-w1", "worker")]).await;
    let code = |result: Result<_, OperatorInstructionError>| result.expect_err("refused").code();
    let receipt = mob
        .handle
        .set_operator_instruction(write("tone", 2, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the write commits");
    assert_eq!(receipt.generation, 1);
    assert_eq!(
        code(
            mob.handle
                .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
                .await
        ),
        Some(InstructionSetErrorCode::StaleRevision)
    );
    let duplicate = mob
        .handle
        .set_operator_instruction(write("tone", 2, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("an exact repeat answers");
    assert_eq!(
        duplicate.disposition,
        OperatorInstructionDisposition::Duplicate
    );
    assert_eq!(duplicate.generation, 1, "a duplicate changes nothing");
    assert_eq!(
        code(
            mob.handle
                .set_operator_instruction(write("tone", 2, BRIEF, OperatorInstructionTarget::Mob))
                .await
        ),
        Some(InstructionSetErrorCode::RevisionConflict)
    );
    assert_eq!(
        code(
            mob.handle
                .revoke_operator_instruction(slot("missing"), revision(1))
                .await
        ),
        Some(InstructionSetErrorCode::UnknownSlot)
    );
    let mut mismatched = write("style", 1, FORMAL, OperatorInstructionTarget::Mob);
    mismatched.content_sha256 = Some(meerkat_core::InstructionContentDigest::for_body(BRIEF));
    assert_eq!(
        code(mob.handle.set_operator_instruction(mismatched).await),
        Some(InstructionSetErrorCode::DigestMismatch)
    );
    assert_eq!(
        code(
            mob.handle
                .set_operator_instruction(write(
                    "big",
                    1,
                    &"x".repeat(256 * 1024 + 1),
                    OperatorInstructionTarget::Mob
                ))
                .await
        ),
        Some(InstructionSetErrorCode::BodyTooLarge)
    );
    assert_eq!(
        status(&mob.handle).await.generation,
        1,
        "no refusal moved the generation"
    );

    mob.handle.complete().await.expect("complete the mob");
    assert_eq!(
        code(
            mob.handle
                .set_operator_instruction(write("tone", 3, FORMAL, OperatorInstructionTarget::Mob))
                .await
        ),
        Some(InstructionSetErrorCode::MobTerminal)
    );
}

/// Events of the kinds before operator instructions stay at mob event
/// schema 9; only the two operator-instruction kinds are stored at 10, and
/// a row whose version is not its kind's is refused.
#[test]
fn events_without_instructions_stay_schema_9() {
    use crate::event::{decode_stored_mob_event, encode_stored_mob_event};
    let event = |kind| MobEvent {
        cursor: 1,
        timestamp: chrono::Utc::now(),
        mob_id: MobId::from("oi-schema"),
        kind,
    };
    let stored_version = |kind| {
        let encoded = encode_stored_mob_event(&event(kind)).expect("encode");
        serde_json::from_slice::<serde_json::Value>(&encoded).expect("parse")["schema_version"]
            .clone()
    };
    for kind in [
        MobEventKind::MobCompleted,
        MobEventKind::MobReset,
        MobEventKind::MobStopped,
    ] {
        assert_eq!(stored_version(kind), 9);
    }
    let committed = MobEventKind::OperatorInstructionCommitted(OperatorInstructionCommittedEvent {
        slot: slot("tone"),
        revision: revision(1),
        effect: InstructionEffect::Active,
        content_sha256: Some(meerkat_core::InstructionContentDigest::for_body(FORMAL)),
        body_bytes: FORMAL.len() as u64,
        target: OperatorInstructionTarget::Mob,
        generation: 1,
        body: Some(FORMAL.to_string()),
    });
    assert_eq!(stored_version(committed.clone()), 10);
    let public = serde_json::to_value(event(committed.clone())).expect("public event");
    assert!(
        public["kind"].get("body").is_none(),
        "the public event never carries the body"
    );
    let encoded = encode_stored_mob_event(&event(committed.clone())).expect("encode");
    let decoded = decode_stored_mob_event(&encoded).expect("decode");
    assert_eq!(decoded.kind, committed, "the stored row keeps the body");

    let mut stored: serde_json::Value = serde_json::from_slice(&encoded).expect("parse");
    stored["schema_version"] = serde_json::json!(9);
    decode_stored_mob_event(&serde_json::to_vec(&stored).expect("bytes"))
        .expect_err("an operator-instruction row at 9 is refused");
    let completed = encode_stored_mob_event(&event(MobEventKind::MobCompleted)).expect("encode");
    let mut stored: serde_json::Value = serde_json::from_slice(&completed).expect("parse");
    stored["schema_version"] = serde_json::json!(10);
    decode_stored_mob_event(&serde_json::to_vec(&stored).expect("bytes"))
        .expect_err("an earlier kind at 10 is refused");
}

/// A mob-wide write records every dormant member store-only, at most the
/// configured number at once, and wakes none of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_wide_set_records_dormant_members_with_bounded_parallelism() {
    const MEMBERS: usize = 12;
    const BOUND: usize = 3;
    let names = (0..MEMBERS)
        .map(|index| format!("oi-fanout-w{index:02}"))
        .collect::<Vec<_>>();
    let members = names
        .iter()
        .map(|name| (name.as_str(), "worker"))
        .collect::<Vec<_>>();
    let mob = InstructionMob::with_concurrency("oi-fanout", &members, Some(BOUND)).await;
    let crashed = mob.crash().await;
    let gate = crashed.service.hold_instruction_records();
    let handle = crashed.resume().await;
    handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the write commits");
    let service = &crashed.service;
    eventually("the dormant lane fills", || async move {
        service.instruction_records_in_flight.load(Ordering::SeqCst) == BOUND as u64
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        crashed.service.instruction_records_max_in_flight(),
        BOUND as u64,
        "no more than the bound are in flight"
    );
    gate.add_permits(MEMBERS);
    for (identity, session) in &crashed.members {
        current(&handle, identity).await;
        assert_eq!(writes_to(&crashed.service, session), 1);
        assert_eq!(
            handle.member_materialization_for_test(identity),
            Some(crate::machines::mob_machine::MemberMaterializationPhase::Dormant)
        );
    }
    assert_eq!(
        crashed.service.instruction_records_max_in_flight(),
        BOUND as u64
    );
    assert!(
        crashed.materialized().await.is_empty(),
        "no member was woken"
    );
}

/// A crash in the middle of a fan-out resumes the remaining obligations: a
/// member whose session recorded before the crash is not written again, and
/// every other one is written once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crash_mid_fanout_resumes_the_remaining_obligations_once() {
    const MEMBERS: usize = 8;
    const BOUND: usize = 2;
    let names = (0..MEMBERS)
        .map(|index| format!("oi-midcrash-w{index:02}"))
        .collect::<Vec<_>>();
    let members = names
        .iter()
        .map(|name| (name.as_str(), "worker"))
        .collect::<Vec<_>>();
    let mob = InstructionMob::with_concurrency("oi-midcrash", &members, Some(BOUND)).await;
    let first = mob.crash().await;
    let gate = first.service.hold_instruction_records();
    let handle = first.resume().await;
    handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the write commits");
    let first_service = &first.service;
    eventually("the first wave is in flight", || async move {
        first_service
            .instruction_records_in_flight
            .load(Ordering::SeqCst)
            == BOUND as u64
    })
    .await;
    gate.add_permits(BOUND);
    let handle_ref = &handle;
    let current_members = || async move {
        status(handle_ref)
            .await
            .members
            .into_iter()
            .filter(|member| member.delivery.state == MemberInstructionDeliveryState::Current)
            .map(|member| member.agent_identity)
            .collect::<Vec<_>>()
    };
    eventually("the first wave is recorded", || async move {
        current_members().await.len() >= BOUND
    })
    .await;
    let recorded_before = current_members().await;
    // The second wave is held in flight when the mob crashes.
    let second = CrashedInstructionMob {
        service: Arc::new(first.service.cold_restart_preserving_durable_state().await),
        events: first.events.clone(),
        runtime_metadata: first.runtime_metadata.clone(),
        members: first.members.clone(),
        concurrency: first.concurrency,
    };
    let _ = second.service.enable_runtime_adapter();
    crash_stop_and_release_routes(handle).await;
    let resumed = second.resume().await;
    for (identity, session) in &second.members {
        current(&resumed, identity).await;
        let before = writes_to(&first.service, session);
        let after = writes_to(&second.service, session);
        assert_eq!(
            before + after,
            1,
            "{identity} is written exactly once ({before} + {after})"
        );
        if recorded_before.contains(identity) {
            assert_eq!(
                after, 0,
                "{identity} recorded before the crash is not written again"
            );
        }
    }
}

/// Another mob's target is never overwritten: a member whose session's
/// target another mob owns is blocked, typed. Once that mob releases the
/// target, the member's mob binds it with its next write.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_mobs_member_target_is_refused_until_it_releases_it() {
    let mob = InstructionMob::new("oi-owner", &[("oi-owner-w1", "worker")]).await;
    let (identity, session) = mob.member(0);
    // Mob A recorded its own snapshot in the session.
    let owned_by_a = InstructionTargetRecord {
        mob_id: "mob-a".to_string(),
        agent_identity: "member-of-a".to_string(),
        generation: 7,
        slots: Vec::new(),
    };
    for store in [
        &mob.service.persisted_sessions,
        &mob.service.live_session_data,
    ] {
        if let Some(document) = store.write().await.get_mut(&session) {
            document
                .record_instruction_target(owned_by_a.clone())
                .expect("mob A records its snapshot");
        }
    }
    mob.handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the write commits");
    let (handle_ref, identity_ref) = (&mob.handle, &identity);
    eventually("the member is blocked", || async move {
        member_status(handle_ref, identity_ref)
            .await
            .is_some_and(|member| {
                member.delivery.state
                    == MemberInstructionDeliveryState::Blocked {
                        reason: MemberInstructionBlockReason::OwnedByOtherMob,
                    }
            })
    })
    .await;
    assert_eq!(
        mob.service
            .committed_instruction_target(&session)
            .await
            .expect("mob A's target")
            .owner(),
        &meerkat_core::InstructionTargetOwner::Mob {
            mob_id: "mob-a".to_string(),
            agent_identity: "member-of-a".to_string(),
        },
        "mob A's target is untouched"
    );

    // Mob A releases the target; this mob's next write binds it.
    for store in [
        &mob.service.persisted_sessions,
        &mob.service.live_session_data,
    ] {
        if let Some(document) = store.write().await.get_mut(&session) {
            document
                .release_instruction_target("mob-a")
                .expect("mob A releases its target");
        }
    }
    mob.handle
        .set_operator_instruction(write("tone", 2, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the next write commits");
    current(&mob.handle, &identity).await;
    let target = mob
        .service
        .committed_instruction_target(&session)
        .await
        .expect("this mob's target");
    assert!(
        matches!(
            target.owner(),
            meerkat_core::InstructionTargetOwner::Mob { mob_id, .. }
                if mob_id.as_str() == mob.handle.mob_id().as_str()
        ),
        "this mob owns the target now: {target:?}"
    );
    assert_eq!(
        active_slots(&mob.service, &session).await,
        vec![("tone".to_string(), 2)]
    );
}

/// A member whose session its host keeps retires: its mob releases the
/// member session's target, empty and unowned, before the member's seat
/// clears, and another mob's snapshot then binds the session while this mob
/// lives on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retiring_member_whose_host_keeps_its_session_releases_the_target() {
    let mut mob = InstructionMob::new("oi-release", &[]).await;
    let (identity, session) = mob.spawn_host_kept("oi-release-w1").await;
    mob.handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the write commits");
    current(&mob.handle, &identity).await;
    mob.handle
        .retire(identity.clone())
        .await
        .expect("the member retires");
    let target = mob
        .service
        .committed_instruction_target(&session)
        .await
        .expect("the host keeps the session and its target");
    assert_eq!(
        target.owner(),
        &InstructionTargetOwner::Unowned,
        "the retirement released the member session's target: {target:?}"
    );
    assert!(
        target.slots().is_empty(),
        "a released target is empty: {target:?}"
    );

    // Another mob's snapshot binds the released session; this mob lives on.
    let mut persisted = mob.service.persisted_sessions.write().await;
    persisted
        .get_mut(&session)
        .expect("the host's session")
        .record_instruction_target(InstructionTargetRecord {
            mob_id: "another-mob".to_string(),
            agent_identity: "another-member".to_string(),
            generation: 1,
            slots: Vec::new(),
        })
        .expect("another mob binds the released target");
    drop(persisted);
    assert!(
        status(&mob.handle)
            .await
            .members
            .iter()
            .all(|member| member.agent_identity != identity),
        "the retired member left the mob"
    );
}

/// A destroy whose release of a member's target does not land answers
/// incomplete and keeps the member seated with its delivery facts; the
/// retried destroy releases the target and completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_destroy_whose_release_does_not_land_is_incomplete_and_its_retry_releases() {
    let mut mob = InstructionMob::new("oi-destroy", &[]).await;
    let (identity, session) = mob.spawn_host_kept("oi-destroy-w1").await;
    mob.handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the write commits");
    current(&mob.handle, &identity).await;
    mob.service.fail_next_instruction_release();
    let error = mob
        .handle
        .destroy()
        .await
        .expect_err("a destroy whose release does not land is incomplete");
    let report = match error {
        crate::runtime::handle::MobDestroyError::Incomplete { report } => report,
        other => panic!("an incomplete destroy, not {other:?}"),
    };
    assert!(
        report.errors.iter().any(|error| {
            error.contains(identity.as_str()) && error.contains("operator instruction target")
        }),
        "the report names the member whose release did not land: {report:?}"
    );
    assert!(
        mob.handle
            .list_members_including_retiring()
            .await
            .iter()
            .any(|entry| entry.agent_identity == identity),
        "the member stays seated for the retry"
    );

    mob.handle
        .destroy()
        .await
        .expect("the retried destroy releases the target and completes");
    let target = mob
        .service
        .committed_instruction_target(&session)
        .await
        .expect("the host keeps the session and its target");
    assert_eq!(
        target.owner(),
        &InstructionTargetOwner::Unowned,
        "the retried destroy released the target: {target:?}"
    );
}

/// A write whose journal row landed although its append answered an error
/// commits: the next write takes the next generation, and after a restart
/// the acknowledged write is the slot's record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_acknowledged_write_after_an_ambiguous_append_survives_a_restart() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let mob = InstructionMob::with_events(
        "oi-ambiguous",
        &[("oi-ambiguous-w1", "worker")],
        events.clone(),
    )
    .await;
    events
        .fail_after_append_for("OperatorInstructionCommitted")
        .await;
    let landed = mob
        .handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("a write whose row landed commits");
    assert_eq!(landed.generation, 1);
    events
        .acknowledge_appends_for("OperatorInstructionCommitted")
        .await;
    let acknowledged = mob
        .handle
        .set_operator_instruction(write("tone", 2, BRIEF, OperatorInstructionTarget::Mob))
        .await
        .expect("the next write commits");
    assert_eq!(
        acknowledged.generation, 2,
        "the next write never reuses the landed row's generation"
    );

    let crashed = mob.crash().await;
    let handle = crashed.resume().await;
    let page = status(&handle).await;
    assert_eq!(page.generation, 2);
    let record = page
        .desired
        .iter()
        .find(|record| record.slot == slot("tone"))
        .expect("the slot's record");
    assert_eq!(
        record.revision,
        revision(2),
        "the acknowledged write is the slot's record after the restart"
    );
}

/// A member seated over an existing session whose own host holds its
/// operator-instruction target (an attach): the mob's first delivery takes
/// the target over, so the member records the mob's snapshot, which
/// replaces the host's target.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attached_sessions_own_target_is_taken_over_by_the_mob() {
    let mob = InstructionMob::new("oi-attach", &[]).await;
    mob.handle
        .set_operator_instruction(write("tone", 1, FORMAL, OperatorInstructionTarget::Mob))
        .await
        .expect("the write commits");
    let identity = AgentIdentity::from("oi-attach-w1");
    let session = mob.host_instructed_session(&identity).await;
    let mut spec = SpawnMemberSpec::new("worker", identity.clone()).with_launch_mode(
        crate::launch::MemberLaunchMode::Resume {
            bridge_session_id: session.clone(),
            resume_from_role: None,
        },
    );
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    mob.handle
        .spawn_spec(spec)
        .await
        .expect("attach the host's session");
    let (handle_ref, identity_ref) = (&mob.handle, &identity);
    eventually("the member's delivery settles", || async move {
        member_status(handle_ref, identity_ref)
            .await
            .is_some_and(|member| {
                !matches!(
                    member.delivery.state,
                    MemberInstructionDeliveryState::Owed { .. }
                )
            })
    })
    .await;
    let member = member_status(&mob.handle, &identity)
        .await
        .expect("the member's row");
    assert_eq!(
        member.delivery.state,
        MemberInstructionDeliveryState::Current,
        "the mob's delivery took the host's target over: {member:?}"
    );
    let target = mob
        .service
        .committed_instruction_target(&session)
        .await
        .expect("the member session's target");
    assert_eq!(
        target.owner(),
        &InstructionTargetOwner::Mob {
            mob_id: mob.handle.mob_id().as_str().to_string(),
            agent_identity: identity.as_str().to_string(),
        }
    );
    assert_eq!(
        active_slots(&mob.service, &session).await,
        vec![("tone".to_string(), 1)],
        "the mob's snapshot replaced the host's target"
    );
}
