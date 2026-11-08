//! A respawn's abandonment row is replayed at every resume, against the
//! machine state the whole journal leaves. When the name was spawned again
//! after the abandonment and that later life ended, the row names a
//! generation the name has moved past: MobMachine answers it as a no-op,
//! and the mob resumes, whoever wrote the row (the live abandonment, or a
//! resume that abandoned the respawn itself) and whenever (a journal 0.9.10
//! wrote included).

use super::*;

/// Bounds a hang only: a passing row never waits for it.
const STEP: Duration = Duration::from_secs(30);

/// A mob over a store-backed runtime, with every store kept, so a resume of
/// the mob reads what its earlier actors wrote.
struct ResumableMob {
    events: Arc<FaultInjectedMobEventStore>,
    service: Arc<MockSessionService>,
    runs: Arc<InMemoryMobRunStore>,
    specs: Arc<InMemoryMobSpecStore>,
    runtime_metadata: Arc<InMemoryMobRuntimeMetadataStore>,
    identity_store: Arc<InMemoryMobIdentityStore>,
    identity_status: Arc<InMemoryMobIdentityStatusStore>,
}

impl ResumableMob {
    fn new() -> Self {
        let runtime_store = Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let runtime = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent_without_blobs(
                runtime_store as Arc<dyn meerkat_runtime::RuntimeStore>,
            )
            .expect("construct runtime authority"),
        );
        let service = Arc::new(MockSessionService::new());
        service.set_runtime_adapter(runtime);
        Self {
            events: Arc::new(FaultInjectedMobEventStore::new()),
            service,
            runs: Arc::new(InMemoryMobRunStore::new()),
            specs: Arc::new(InMemoryMobSpecStore::new()),
            runtime_metadata: Arc::new(InMemoryMobRuntimeMetadataStore::new()),
            identity_store: Arc::new(InMemoryMobIdentityStore::new()),
            identity_status: Arc::new(InMemoryMobIdentityStatusStore::new()),
        }
    }

    fn storage(&self) -> MobStorage {
        MobStorage::custom_with_runtime_metadata(
            self.events.clone(),
            self.runs.clone(),
            self.specs.clone(),
            self.runtime_metadata.clone(),
            self.identity_store.clone(),
            self.identity_status.clone(),
        )
    }

    async fn create(&self) -> MobHandle {
        MobBuilder::new(sample_definition(), self.storage())
            .with_session_service(self.service.clone())
            .create()
            .await
            .expect("create mob")
    }

    /// The mob's resume from its stores, as it answers.
    async fn resume(&self) -> Result<MobHandle, MobError> {
        MobBuilder::for_resume(self.storage())
            .with_session_service(self.service.clone())
            .notify_orchestrator_on_resume(false)
            .resume()
            .await
    }

    /// Spawn a life of `identity` in `handle`'s mob.
    async fn spawn(&self, handle: &MobHandle, identity: &AgentIdentity) {
        handle
            .spawn_with_options(
                ProfileName::from("worker"),
                identity.clone(),
                None,
                Some(crate::MobRuntimeMode::TurnDriven),
                None,
            )
            .await
            .expect("spawn the member");
    }

    /// The generations of `identity` whose respawn the journal records
    /// abandoned.
    async fn abandoned_generations(&self, identity: &AgentIdentity) -> Vec<crate::ids::Generation> {
        self.events
            .replay_all()
            .await
            .expect("the journal")
            .iter()
            .filter_map(|event| match &event.kind {
                MobEventKind::RespawnTopologyAbandoned {
                    agent_identity,
                    generation,
                    ..
                } if agent_identity == identity => Some(*generation),
                _ => None,
            })
            .collect()
    }
}

/// A respawn abandoned after its old member's archive (the replacement's
/// membership never commits), the name spawned again, and that later life
/// retired. Every resume replays the first life's abandonment row, which
/// MobMachine answers as a no-op: the mob resumes, again after a second
/// crash, and the name is free for a third life.
#[tokio::test]
async fn a_mob_resumes_after_a_later_life_of_an_abandoned_respawns_name_retired() {
    let mob = ResumableMob::new();
    let handle = mob.create().await;
    let identity = AgentIdentity::from("respawn-later-retired");
    mob.spawn(&handle, &identity).await;

    mob.events.fail_appends_for("MemberSpawned").await;
    let respawned = tokio::time::timeout(STEP, handle.respawn(identity.clone(), None))
        .await
        .expect("the respawn answers within the bound");
    mob.events.allow_appends_for("MemberSpawned").await;
    assert!(respawned.is_err(), "the respawn fails: {respawned:?}");

    mob.spawn(&handle, &identity).await;
    retire_to_terminal(&handle, &identity)
        .await
        .expect("the later life retires");
    assert_eq!(
        mob.abandoned_generations(&identity).await,
        vec![crate::ids::Generation::INITIAL],
        "the first life's abandonment row precedes the later life"
    );

    crash_stop_and_release_routes(handle).await;
    let resumed = mob
        .resume()
        .await
        .unwrap_or_else(|error| panic!("the mob resumes: {error}"));
    assert!(
        resumed
            .get_member(&identity)
            .await
            .expect("the member lookup")
            .is_none(),
        "the name holds no member"
    );

    crash_stop_and_release_routes(resumed).await;
    let again = mob
        .resume()
        .await
        .unwrap_or_else(|error| panic!("the mob resumes again: {error}"));
    mob.spawn(&again, &identity).await;
    let third = again
        .get_member(&identity)
        .await
        .expect("the member lookup")
        .expect("the name's third life");
    assert_eq!(
        third.agent_runtime_id.generation,
        crate::ids::Generation::new(2),
        "the third life takes the next generation"
    );
    again.shutdown().await.expect("shutdown resumed mob");
}

/// The abandonment's row could not be written when the replacement failed
/// after the old member's archive, so the actor stopped for cold recovery;
/// the next resume abandoned the respawn and wrote the row. The name was
/// then spawned again and that later life retired: the following resume
/// replays the row the earlier resume wrote, as a no-op, and the mob
/// resumes.
#[tokio::test]
async fn a_mob_resumes_after_a_later_life_of_a_respawn_a_resume_abandoned_retired() {
    let mob = ResumableMob::new();
    let handle = mob.create().await;
    let identity = AgentIdentity::from("respawn-resume-abandoned");
    mob.spawn(&handle, &identity).await;

    mob.events.fail_appends_for("MemberSpawned").await;
    mob.events
        .fail_appends_for("RespawnTopologyAbandoned")
        .await;
    let respawned = tokio::time::timeout(STEP, handle.respawn(identity.clone(), None))
        .await
        .expect("the respawn answers within the bound");
    mob.events
        .allow_appends_for("RespawnTopologyAbandoned")
        .await;
    mob.events.allow_appends_for("MemberSpawned").await;
    assert!(respawned.is_err(), "the respawn fails: {respawned:?}");
    assert!(
        mob.abandoned_generations(&identity).await.is_empty(),
        "the live abandonment wrote no row"
    );

    crash_stop_and_release_routes(handle).await;
    let first = mob
        .resume()
        .await
        .unwrap_or_else(|error| panic!("the first resume: {error}"));
    assert_eq!(
        mob.abandoned_generations(&identity).await,
        vec![crate::ids::Generation::INITIAL],
        "the resume abandoned the respawn"
    );
    assert!(
        first
            .get_member(&identity)
            .await
            .expect("the member lookup")
            .is_none(),
        "the resume left no member under the name"
    );

    mob.spawn(&first, &identity).await;
    retire_to_terminal(&first, &identity)
        .await
        .expect("the later life retires");

    crash_stop_and_release_routes(first).await;
    let second = mob
        .resume()
        .await
        .unwrap_or_else(|error| panic!("the mob resumes: {error}"));
    assert!(
        second
            .get_member(&identity)
            .await
            .expect("the member lookup")
            .is_none(),
        "the name holds no member"
    );
    second.shutdown().await.expect("shutdown resumed mob");
}

/// The upgrade: a mob that 0.9.10 left in this state, every resume of it
/// failed. Its journal holds the rows below, written here by hand as 0.9.10
/// wrote them (no current writer is involved): the name's first life, its
/// respawn's retirement and archive, the respawn's abandonment, and the
/// name's later life, spawned and retired. The resume replays them and the
/// mob serves: the name is free for its next life.
#[tokio::test]
async fn a_mob_whose_journal_holds_a_stale_abandonment_row_resumes_after_the_upgrade() {
    let mob = ResumableMob::new();
    let handle = mob.create().await;
    let mob_id = handle.mob_id().clone();
    crash_stop_and_release_routes(handle).await;

    let identity = AgentIdentity::from("upgraded-later-retired");
    let first = crate::ids::Generation::INITIAL;
    let later = crate::ids::Generation::new(1);
    let runtime = |generation| crate::ids::AgentRuntimeId::new(identity.clone(), generation);
    let spawned = |generation, fence| {
        MobEventKind::MemberSpawned(crate::event::MemberSpawnedEvent::new(
            identity.clone(),
            generation,
            crate::ids::FenceToken::new(fence),
            runtime(generation),
            ProfileName::from("worker"),
        ))
    };
    let started = |generation, respawn| MobEventKind::MemberRetirementStarted {
        agent_identity: identity.clone(),
        agent_runtime_id: runtime(generation),
        generation,
        role: ProfileName::from("worker"),
        releasing: None,
        session_id: None,
        retiring_peer_endpoint: None,
        preserve_machine_topology: respawn,
        rotation: None,
        cause: None,
    };
    let retired = |generation| MobEventKind::MemberRetired {
        agent_identity: identity.clone(),
        generation,
        role: ProfileName::from("worker"),
        session_archived: true,
        facts: crate::event::MemberRetiredFacts::default(),
    };
    let rows = vec![
        spawned(first, 1),
        started(first, true),
        retired(first),
        MobEventKind::RespawnTopologyAbandoned {
            agent_identity: identity.clone(),
            generation: first,
            agent_runtime_id: Some(runtime(first)),
            fence_token: Some(crate::ids::FenceToken::new(1)),
        },
        spawned(later, 2),
        started(later, false),
        retired(later),
    ];
    mob.events
        .append_batch(
            rows.into_iter()
                .map(|kind| NewMobEvent {
                    mob_id: mob_id.clone(),
                    timestamp: None,
                    kind,
                })
                .collect(),
        )
        .await
        .expect("the rows 0.9.10 wrote");

    let resumed = mob
        .resume()
        .await
        .unwrap_or_else(|error| panic!("the mob resumes after the upgrade: {error}"));
    assert!(
        resumed
            .get_member(&identity)
            .await
            .expect("the member lookup")
            .is_none(),
        "the name holds no member"
    );
    mob.spawn(&resumed, &identity).await;
    let next = resumed
        .get_member(&identity)
        .await
        .expect("the member lookup")
        .expect("the name's next life");
    assert_eq!(
        next.agent_runtime_id.generation,
        crate::ids::Generation::new(2),
        "the next life takes the generation after the journal's"
    );
    resumed.shutdown().await.expect("shutdown resumed mob");
}
