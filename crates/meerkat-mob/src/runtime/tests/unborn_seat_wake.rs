//! Rows of a seat that built nothing, on the wake side: a session an earlier
//! wake created before a crash lost its born row, or one a wake that ended
//! retryable left (before or after its born row), is adopted or revived at
//! the next wake or at an eager start, never created again; one member's
//! create that fails for a passing cause leaves only that member Dormant at
//! an eager start; a reload of a member whose session was never created is
//! refused, typed. They run on the combined candidate: the machine, the wake
//! path and the boot path together.

use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::{DormantSeatOutcome, MemberRestorePolicy};

const STEP: Duration = Duration::from_secs(60);

/// A mob over stored member keys with one fresh turn-driven member seated
/// Dormant: its seat built nothing.
struct SeatedMob {
    keys: tempfile::TempDir,
    handle: MobHandle,
    events: Arc<dyn MobEventStore>,
    runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    service: Arc<MockSessionService>,
    identity: AgentIdentity,
    /// The session the seat named.
    session: SessionId,
}

impl SeatedMob {
    async fn new(label: &str) -> Self {
        Self::with_storage(label, MobStorage::in_memory()).await
    }

    async fn with_storage(label: &str, storage: MobStorage) -> Self {
        let keys = tempfile::tempdir().expect("a comms identity root");
        let definition = with_unique_mob_id(sample_definition(), label);
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let events = storage.events.clone();
        let runtime_metadata = storage.runtime_metadata.clone();
        let handle = MobBuilder::new(definition, storage)
            .with_session_service(service.clone())
            .with_member_comms_identity_root(keys.path())
            .create()
            .await
            .expect("create the mob");
        // Named by the row: the wake test seams are keyed by identity.
        let identity = AgentIdentity::from(format!("{label}-worker"));
        let mut spec = SpawnMemberSpec::new("worker", identity.clone());
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        let outcomes = handle
            .seat_dormant_members(vec![spec])
            .await
            .expect("the seat batch answers");
        let session = match &outcomes[0] {
            DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
            other => panic!("the fresh spec is seated: {other:?}"),
        };
        Self {
            keys,
            handle,
            events,
            runtime_metadata,
            service,
            identity,
            session,
        }
    }

    fn unborn(&self) -> bool {
        crate::runtime::boot_plan::domain_member_seat_session_unborn(
            &self.handle.machine_state_watch_rx.borrow(),
            &self.identity,
        )
    }

    fn phase(&self) -> Option<dsl::MemberMaterializationPhase> {
        self.handle.member_materialization_for_test(&self.identity)
    }

    /// Wait until the member's phase is `expected`, within [`STEP`].
    async fn wait_phase(&self, expected: dsl::MemberMaterializationPhase) {
        let deadline = tokio::time::Instant::now() + STEP;
        while self.phase() != Some(expected) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the member never reached {expected:?}: {:?}",
                self.phase()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// What an earlier wake's create leaves when the process crashes before
    /// its born row: the seat's session exists, minted under the seat's id
    /// with the durable metadata a member's create writes (through the
    /// service's own create), its live actor gone with the process, and no
    /// row names it born.
    async fn create_the_session_without_its_born_row(&self) {
        let created = self
            .service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "created before its born row".to_string().into(),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    resume_session: Some(Session::with_id(self.session.clone())),
                    resume_session_intent: meerkat_core::SessionBuildIntent::Mint,
                    comms_name: Some(
                        super::actor::render_member_comms_name(
                            self.handle.mob_id().as_str(),
                            "worker",
                            self.identity.as_str(),
                        )
                        .expect("comms name"),
                    ),
                    mob_member_binding: Some(meerkat_core::MobMemberBinding {
                        mob_id: self.handle.mob_id().to_string(),
                        role: "worker".to_string(),
                        member: self.identity.to_string(),
                    }),
                    ..Default::default()
                }),
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                labels: None,
            })
            .await
            .expect("create the seat's session");
        assert_eq!(
            created.session_id, self.session,
            "the session is minted under the seat's id"
        );
        MobSessionService::discard_live_session(self.service.as_ref(), &self.session)
            .await
            .expect("the crash ends the creating actor");
    }

    /// Crash the mob, restart its session service and resume it under
    /// `policy`.
    async fn restart(&mut self, policy: MemberRestorePolicy) {
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
                .with_member_restore_policy(policy)
                .notify_orchestrator_on_resume(false)
                .resume(),
            ),
        )
        .await
        .expect("the restart answers")
        .expect("the restart succeeds");
    }

    /// The born rows the journal holds for the member's seat session.
    async fn born_rows(&self) -> usize {
        self.events
            .replay_all()
            .await
            .expect("read the journal")
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    MobEventKind::MemberSessionBindingRecovered(recovered)
                        if recovered.agent_identity == self.identity
                            && recovered.records_seat_session_born()
                )
            })
            .count()
    }

    /// The creates of the seat's session the current service recorded: the
    /// builds that minted it. A build that resumes the session (an adoption
    /// of a session an earlier wake created, a revival) creates nothing.
    async fn creates_of_the_session(&self) -> usize {
        self.service
            .recorded_create_requests()
            .await
            .iter()
            .filter(|record| {
                record.session_id == self.session
                    && record.session_build_intent == meerkat_core::SessionBuildIntent::Mint
            })
            .count()
    }

    /// After the adoption: the member is live on the seat's session (an
    /// eager start's member becomes Live once the actor's startup readiness
    /// ran), resumed rather than created again, its session born, and one
    /// born row journaled.
    async fn assert_adopted(&self) {
        self.wait_phase(dsl::MemberMaterializationPhase::Live).await;
        assert_eq!(
            self.handle.resolve_bridge_session_id(&self.identity).await,
            Some(self.session.clone()),
            "the member serves the session its seat named"
        );
        assert_eq!(
            self.creates_of_the_session().await,
            0,
            "a session that exists is resumed, never created again"
        );
        assert!(!self.unborn(), "the adopted session is born");
        assert_eq!(self.born_rows().await, 1, "one born row is journaled");
    }
}

/// A first wake that created the seat's session and crashed before its born
/// row leaves the session unborn after the restart, though it exists. The
/// next wake adopts it: it resumes the session and resolves it born, through
/// the creation's own transition, never as a plain resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_session_created_before_its_born_row_is_adopted_at_the_next_wake() {
    let mut mob = SeatedMob::new("unborn-adopt-wake").await;
    assert!(mob.unborn(), "the seat built nothing");
    mob.create_the_session_without_its_born_row().await;
    mob.restart(MemberRestorePolicy::OnDemand).await;
    assert!(
        mob.unborn(),
        "without a born row the restart holds the session unborn"
    );
    assert_eq!(
        mob.phase(),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the restart materialized nothing"
    );

    mob.handle
        .ensure_member_live(&mob.identity)
        .await
        .expect("the wake adopts the created session");

    mob.assert_adopted().await;
}

/// An eager start adopts such a session the same way, in the boot restore.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eager_start_adopts_a_seat_session_created_before_its_born_row() {
    let mut mob = SeatedMob::new("unborn-adopt-eager").await;
    mob.create_the_session_without_its_born_row().await;
    mob.restart(MemberRestorePolicy::Eager).await;

    mob.assert_adopted().await;
}

/// An eager start of a seat whose session was never created creates it
/// there, under the id the seat named, and resolves it born. The member is
/// Live once the actor's startup readiness ran after the start returned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eager_start_creates_a_never_created_seat_session_under_its_seat_id() {
    let mut mob = SeatedMob::new("unborn-eager-create").await;
    mob.restart(MemberRestorePolicy::Eager).await;

    mob.wait_phase(dsl::MemberMaterializationPhase::Live).await;
    assert_eq!(
        mob.creates_of_the_session().await,
        1,
        "the start created the seat's session once, under the seat's id"
    );
    assert!(!mob.unborn(), "the created session is born");
    assert_eq!(mob.born_rows().await, 1, "one born row is journaled");
}

/// A first wake that created the seat's session and then ended retryable
/// (its born row could not be journaled, so its turn was refused) keeps the
/// session rather than archiving it: only its live materialization is
/// released, and the member is Dormant with its session unborn. The next
/// wake adopts that session, as it adopts one a crash left created, so the
/// session is created exactly once across both wakes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_wake_that_ends_retryable_after_the_create_leaves_the_session_to_the_next_wake() {
    let faults = Arc::new(FaultInjectedMobEventStore::new());
    let storage = MobStorage::with_events_and_runtime_metadata(
        faults.clone(),
        Arc::new(InMemoryMobRuntimeMetadataStore::new()),
    );
    let mob = SeatedMob::with_storage("unborn-retryable", storage).await;
    faults
        .fail_appends_for("MemberSessionBindingRecovered")
        .await;

    let refused = mob
        .handle
        .ensure_member_live(&mob.identity)
        .await
        .expect_err("the first wake ends retryable");
    assert!(
        matches!(
            &refused,
            MobError::MemberWakeRefused { refusal, .. } if refusal.is_retryable()
        ),
        "{refused:?}"
    );
    mob.wait_phase(dsl::MemberMaterializationPhase::Dormant)
        .await;
    assert_eq!(
        mob.creates_of_the_session().await,
        1,
        "the first wake created the seat's session"
    );
    assert!(mob.unborn(), "no born row was journaled: still unborn");
    assert_eq!(mob.born_rows().await, 0);
    assert!(
        !mob.service.has_live_session(&mob.session).await.unwrap(),
        "the session's live materialization was released"
    );
    assert!(
        !mob.service.test_is_session_archived(&mob.session).await,
        "the session was kept, not archived"
    );

    faults
        .allow_appends_for("MemberSessionBindingRecovered")
        .await;
    mob.handle
        .ensure_member_live(&mob.identity)
        .await
        .expect("the next wake adopts the session");
    assert_eq!(
        mob.phase(),
        Some(dsl::MemberMaterializationPhase::Live),
        "the member is live"
    );
    assert_eq!(
        mob.creates_of_the_session().await,
        1,
        "the session is created once across both wakes"
    );
    assert!(!mob.unborn(), "the adopted session is born");
    assert_eq!(mob.born_rows().await, 1, "one born row is journaled");
}

/// A reload of a member whose seat built nothing is refused, typed: it has
/// no session to reload until its first wake creates it. The refusal
/// changes nothing; the member stays Dormant with its session unborn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reload_of_a_member_whose_seat_built_nothing_is_refused_typed() {
    let mob = SeatedMob::new("unborn-reload").await;

    let refused = mob
        .handle
        .reload_member_registration(&mob.identity)
        .await
        .expect_err("there is nothing to reload");

    assert!(
        matches!(
            &refused,
            MobError::UnsupportedForMode { reason, .. }
                if reason.contains("created at its first wake")
        ),
        "{refused:?}"
    );
    assert!(mob.unborn(), "the session stays unborn");
    assert_eq!(mob.creates_of_the_session().await, 0, "nothing was created");
    assert_eq!(
        mob.phase(),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "nothing was materialized"
    );
}

/// A first wake that created the seat's session and journaled it born, then
/// ended retryable before the member was live (the mob was stopped under its
/// readiness), keeps the session: only its live materialization is
/// released. The next wake revives the same session as any born member's,
/// never reading it as lost and never creating it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_wake_that_ends_retryable_after_the_born_row_leaves_the_session_to_the_next_wake() {
    let mob = SeatedMob::new("born-retryable").await;
    let release =
        crate::runtime::actor::member_wake_turn::hold_wake_readiness_for_test(&mob.identity);
    let handle = mob.handle.clone();
    let woken = mob.identity.clone();
    let first = tokio::spawn(async move { handle.ensure_member_live(&woken).await });
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.born_rows().await == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the first wake never journaled its born row"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The born row is journaled first, inside the wake's plan step, and
    // MobMachine's state is published once that step returns: the
    // projection is waited for, not read in the same instant.
    while mob.unborn() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the created session is born"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let handle = mob.handle.clone();
    let stop = tokio::spawn(async move { handle.stop().await });
    let deadline = tokio::time::Instant::now() + STEP;
    while !matches!(mob.handle.status().await, Ok(MobState::Stopped)) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the stop never landed while the wake was held"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = release.send(true);
    let first = tokio::time::timeout(STEP, first)
        .await
        .expect("the first wake answers")
        .expect("the first wake task joins");
    assert!(first.is_err(), "the stop ended the first wake: {first:?}");
    tokio::time::timeout(STEP, stop)
        .await
        .expect("the stop answers")
        .expect("the stop task joins")
        .expect("the stop succeeds");
    mob.wait_phase(dsl::MemberMaterializationPhase::Dormant)
        .await;
    // The wake's rollback settles once the session's live materialization
    // is gone; only then is what it did with the session final.
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.service.has_live_session(&mob.session).await.unwrap() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session's live materialization was never released"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !mob.service.test_is_session_archived(&mob.session).await,
        "the born session was kept, not archived"
    );

    tokio::time::timeout(STEP, mob.handle.resume())
        .await
        .expect("the resume answers")
        .expect("the mob resumes");
    mob.handle
        .ensure_member_live(&mob.identity)
        .await
        .expect("the next wake revives the session");
    assert_eq!(
        mob.phase(),
        Some(dsl::MemberMaterializationPhase::Live),
        "the member is live"
    );
    assert_eq!(
        mob.handle.resolve_bridge_session_id(&mob.identity).await,
        Some(mob.session.clone()),
        "the member serves the session its seat named"
    );
    assert_eq!(
        mob.creates_of_the_session().await,
        1,
        "the session is created once across both wakes"
    );
    assert_eq!(mob.born_rows().await, 1, "one born row is journaled");
}

/// An eager start whose create of one never-created seat session fails for a
/// cause that clears on its own (the session store failed the write) leaves
/// only that member Dormant, its session still unborn: every other seat's
/// session is created and the start completes. Once the cause clears, the
/// member's first delivery creates its session once, under its seat id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eager_start_with_one_failing_seat_create_leaves_only_that_member_dormant() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let definition = with_unique_mob_id(sample_definition(), "unborn-eager-one-fails");
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
    let identities = (0..3)
        .map(|index| AgentIdentity::from(format!("eager-seat-{index}")))
        .collect::<Vec<_>>();
    let specs = identities
        .iter()
        .map(|identity| {
            let mut spec = SpawnMemberSpec::new("worker", identity.clone());
            spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
            spec
        })
        .collect();
    let outcomes = handle
        .seat_dormant_members(specs)
        .await
        .expect("the seat batch answers");
    let sessions = outcomes
        .iter()
        .map(|outcome| match outcome {
            DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
            other => panic!("every fresh spec is seated: {other:?}"),
        })
        .collect::<Vec<_>>();
    crash_stop_and_release_routes(handle).await;
    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    let _ = restarted.enable_runtime_adapter();
    restarted.set_create_session_failure(&sessions[1]).await;

    let handle = tokio::time::timeout(
        STEP,
        Box::pin(
            MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
                events.clone(),
                runtime_metadata.clone(),
            ))
            .with_session_service(restarted.clone())
            .with_member_comms_identity_root(keys.path())
            .with_member_restore_policy(MemberRestorePolicy::Eager)
            .notify_orchestrator_on_resume(false)
            .resume(),
        ),
    )
    .await
    .expect("the start answers")
    .expect("one failing create does not fail the start");

    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    let unborn = |identity: &AgentIdentity| {
        crate::runtime::boot_plan::domain_member_seat_session_unborn(
            &handle.machine_state_watch_rx.borrow(),
            identity,
        )
    };
    for index in [0, 2] {
        let identity = &identities[index];
        assert_eq!(
            handle.member_materialization_for_test(identity),
            Some(dsl::MemberMaterializationPhase::Live),
            "{identity} is live"
        );
        assert!(!unborn(identity), "{identity}'s session was created");
        assert_eq!(
            restarted
                .recorded_create_requests()
                .await
                .iter()
                .filter(|record| record.session_id == sessions[index])
                .count(),
            1,
            "{identity}'s session is created once, under its seat id"
        );
    }
    let failed = &identities[1];
    assert_eq!(
        handle.member_materialization_for_test(failed),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the member whose create failed is Dormant, not Broken"
    );
    assert!(unborn(failed), "its session was never created");
    let status = handle
        .member_status(failed)
        .await
        .expect("read the member's status");
    assert_ne!(
        status.status,
        crate::runtime::handle::MobMemberStatus::Broken,
        "{status:?}"
    );

    restarted.clear_create_session_failure(&sessions[1]).await;
    handle
        .ensure_member_live(failed)
        .await
        .expect("the member's first delivery creates its session");
    assert_eq!(
        handle.member_materialization_for_test(failed),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    assert!(!unborn(failed), "its session is created");
    assert_eq!(
        restarted
            .recorded_create_requests()
            .await
            .iter()
            .filter(|record| record.session_id == sessions[1])
            .count(),
        1,
        "its session is created once, under its seat id"
    );
}

/// The member binding history reads MobMachine's unborn-seat fact from the
/// journal rows the machine's own replay restores it from, folded the same
/// way. For every session a seat, a first wake, a rotation, a retirement
/// before a first wake and a mob reset leave, the history's `session_unborn`
/// equals the live machine's (`MobHandle::session_unborn`), and both equal
/// what each step leaves: before a restart, and after a restart rebuilt the
/// machine from the same journal. The history's sessions since the latest
/// mob reset keep the woken identity's created session after its rotation
/// seated the next life unborn, and a mob reset empties them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_binding_history_reads_each_seat_session_unborn_as_the_machine_holds_it() {
    /// Crash the mob and resume it over the same stores, as a restart does:
    /// the same storage composition, its built-in identity member store
    /// included, which the running mob's binding history reads through.
    async fn restart(
        handle: MobHandle,
        service: &MockSessionService,
        storage: &MobStorage,
        keys: &std::path::Path,
    ) -> (MobHandle, Arc<MockSessionService>) {
        crash_stop_and_release_routes(handle).await;
        let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        let handle = tokio::time::timeout(
            STEP,
            Box::pin(
                MobBuilder::for_resume(storage.clone())
                    .with_session_service(restarted.clone())
                    .with_member_comms_identity_root(keys)
                    .with_member_restore_policy(MemberRestorePolicy::OnDemand)
                    .notify_orchestrator_on_resume(false)
                    .resume(),
            ),
        )
        .await
        .expect("the restart answers")
        .expect("the restart succeeds");
        (handle, restarted)
    }

    let keys = tempfile::tempdir().expect("a comms identity root");
    let definition = with_unique_mob_id(sample_definition(), "unborn-history");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(definition, storage.clone())
        .with_session_service(service.clone())
        .with_member_comms_identity_root(keys.path())
        .create()
        .await
        .expect("create the mob");
    let spec = |identity: &AgentIdentity| {
        let mut spec = SpawnMemberSpec::new("worker", identity.clone());
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        spec
    };
    let seated_session = |outcome: &DormantSeatOutcome| match outcome {
        DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
        other => panic!("every fresh spec is seated: {other:?}"),
    };
    let woken = AgentIdentity::from("unborn-history-woken");
    let dormant = AgentIdentity::from("unborn-history-dormant");
    let retired = AgentIdentity::from("unborn-history-retired");
    let outcomes = handle
        .seat_dormant_members(vec![spec(&woken), spec(&dormant), spec(&retired)])
        .await
        .expect("the seat batch answers");
    let sessions = outcomes.iter().map(seated_session).collect::<Vec<_>>();
    handle
        .ensure_member_live(&woken)
        .await
        .expect("the first wake creates the session");
    let rotated = tokio::time::timeout(STEP, handle.rotate_member(spec(&woken)))
        .await
        .expect("the rotation answers")
        .expect("the rotation seats the next incarnation");
    tokio::time::timeout(STEP, handle.retire(retired.clone()))
        .await
        .expect("the retirement answers")
        .expect("a member retired before its first wake");

    let history_of = |journal: &[MobEvent], handle: &MobHandle| {
        crate::runtime::MemberBindingHistory::from_journal(
            journal,
            BTreeMap::new(),
            Some(handle.mob_id()),
        )
    };
    let since_reset = |history: &crate::runtime::MemberBindingHistory| {
        history
            .session_bindings_since_reset()
            .filter_map(|binding| binding.session_id().cloned())
            .collect::<BTreeSet<_>>()
    };
    let expected = [
        (sessions[0].clone(), false, "created by the first wake"),
        (rotated.clone(), true, "the rotation's next session"),
        (sessions[1].clone(), true, "never woken"),
        (sessions[2].clone(), true, "retired before its first wake"),
        (SessionId::new(), false, "never named"),
    ];
    let named = BTreeSet::from([
        sessions[0].clone(),
        rotated.clone(),
        sessions[1].clone(),
        sessions[2].clone(),
    ]);
    let journal = events.replay_all().await.expect("read the journal");
    let history = history_of(&journal, &handle);
    let read = handle
        .member_binding_history()
        .await
        .expect("the running mob reads its binding history");
    for (session, unborn, shape) in &expected {
        assert_eq!(history.session_unborn(session), *unborn, "history: {shape}");
        assert_eq!(read.session_unborn(session), *unborn, "read: {shape}");
        assert_eq!(handle.session_unborn(session), *unborn, "machine: {shape}");
    }
    assert_eq!(since_reset(&history), named);
    assert_eq!(since_reset(&read), named);
    assert_eq!(
        history
            .last_binding(&woken)
            .and_then(|binding| binding.session_id().cloned()),
        Some(rotated.clone()),
        "the woken identity's last binding is its next, unborn life"
    );
    assert_eq!(
        history
            .binding_for_session(&sessions[0])
            .map(|binding| binding.identity().clone()),
        Some(woken.clone()),
        "its created session stays among the sessions since the latest reset"
    );

    let (handle, service) = restart(handle, &service, &storage, keys.path()).await;
    let journal = events.replay_all().await.expect("read the journal");
    let history = history_of(&journal, &handle);
    for (session, unborn, shape) in &expected {
        assert_eq!(
            history.session_unborn(session),
            *unborn,
            "history after the restart: {shape}"
        );
        assert_eq!(
            handle.session_unborn(session),
            *unborn,
            "replayed machine: {shape}"
        );
    }
    assert_eq!(since_reset(&history), named);

    // A mob reset ends every membership and clears the fact; a seat after it
    // is unborn again, and only its session is named since the reset.
    tokio::time::timeout(STEP, handle.reset())
        .await
        .expect("the mob reset answers")
        .expect("reset the mob");
    let after_reset = AgentIdentity::from("unborn-history-after-reset");
    let outcomes = handle
        .seat_dormant_members(vec![spec(&after_reset)])
        .await
        .expect("the seat after the reset answers");
    let reseated = seated_session(&outcomes[0]);
    let mut after = expected
        .iter()
        .map(|(session, _, shape)| (session.clone(), false, *shape))
        .collect::<Vec<_>>();
    after.push((reseated.clone(), true, "seated after the reset"));
    let journal = events.replay_all().await.expect("read the journal");
    let history = history_of(&journal, &handle);
    let read = handle
        .member_binding_history()
        .await
        .expect("the running mob reads its binding history");
    for (session, unborn, shape) in &after {
        assert_eq!(
            history.session_unborn(session),
            *unborn,
            "history after the mob reset: {shape}"
        );
        assert_eq!(
            read.session_unborn(session),
            *unborn,
            "read after the mob reset: {shape}"
        );
        assert_eq!(
            handle.session_unborn(session),
            *unborn,
            "machine after the mob reset: {shape}"
        );
    }
    let only_reseated = BTreeSet::from([reseated.clone()]);
    assert_eq!(since_reset(&history), only_reseated);
    assert_eq!(since_reset(&read), only_reseated);

    let (handle, _service) = restart(handle, &service, &storage, keys.path()).await;
    let journal = events.replay_all().await.expect("read the journal");
    let history = history_of(&journal, &handle);
    for (session, unborn, shape) in &after {
        assert_eq!(
            history.session_unborn(session),
            *unborn,
            "history after the mob reset and a restart: {shape}"
        );
        assert_eq!(
            handle.session_unborn(session),
            *unborn,
            "replayed machine after the mob reset: {shape}"
        );
    }
    assert_eq!(since_reset(&history), only_reseated);
}

/// A member whose session is unborn has no history yet: its member history
/// reads an empty page, without waking it or creating the session. Its
/// first wake creates the session, and the read then reads that session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_whose_session_is_unborn_reads_an_empty_history() {
    let mob = SeatedMob::new("unborn-history-read").await;
    assert!(mob.unborn(), "the seat built nothing");
    let page = mob
        .handle
        .member_history(
            crate::control_policy::MobControlPrincipal::Owner,
            mob.identity.clone(),
            None,
            Some(16),
        )
        .await
        .expect("the history of an unborn session reads");
    assert!(page.page.messages.is_empty(), "{page:?}");
    assert_eq!(page.page.message_count, 0);
    assert!(mob.unborn(), "the read did not create the session");
    assert_eq!(
        mob.phase(),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the read did not wake the member"
    );
    assert_eq!(mob.creates_of_the_session().await, 0);

    mob.handle
        .ensure_member_live(&mob.identity)
        .await
        .expect("the first wake creates the session");
    assert!(!mob.unborn(), "the session is born");
    mob.handle
        .member_history(
            crate::control_policy::MobControlPrincipal::Owner,
            mob.identity.clone(),
            None,
            Some(16),
        )
        .await
        .expect("the history of the created session reads");
}
