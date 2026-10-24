//! A graceful Shutdown releases the identity reconcile lease of every member
//! it reports unregistered, so the next incarnation takes the identity at
//! once instead of waiting the lease out.
//!
//! Only a member whose runtime is gone is released: a member whose unregister
//! is still running at the Shutdown's deadline keeps its lease, and so does
//! every member of a mob that ends without a Shutdown. Those leases expire as
//! before.
//!
//! The identity store's clock stands still, an hour ahead of the system clock
//! the reconcile pass reads, so every lease it grants stays live for the whole
//! test: a lease left held never expires here, and the next incarnation can
//! take an identity only after a release.
//!
//! A member's session actor holds the session's hosting claim until it exits,
//! and the runtime unregister does not wait for that exit. The lease of a
//! member whose actor still lives is not released: a successor given the
//! identity would find the session still hosted here.
//!
//! A restart over a persistent store takes at once a lease that an earlier
//! incarnation left in a process that has exited (see
//! `identity_incarnation_custody`); an earlier incarnation of this process,
//! one whose exit this process cannot prove, and a store without a file wait
//! the lease out as before.

use super::actor_isolation::FaultInjectingRuntimeStore;
use super::*;
use crate::identity::{IdentityLeaseClaim, IdentityReconcileDecision, IdentityStoredObservation};
use crate::runtime::{MemberShutdownOutcome, ShutdownOptions};

/// Bound for one step (a convergence, a resume); it only turns a hang into a
/// failure.
const STEP: Duration = Duration::from_secs(10);

/// An identity store clock that stands still.
struct FrozenIdentityClock {
    now_ms: u64,
}

impl crate::store::MobIdentityStoreClock for FrozenIdentityClock {
    fn now_ms(&self) -> Result<u64, crate::store::MobStoreError> {
        Ok(self.now_ms)
    }
}

/// Holds one session's next actor exit. An exiting actor's agent releases what
/// it owns before the actor reports its exit, and the session task owns the
/// session's hosting claim until then: a held actor keeps the claim until the
/// test lets it go.
#[derive(Clone)]
struct ActorExitGate {
    held: Arc<std::sync::Mutex<Option<SessionId>>>,
    reached: Arc<tokio::sync::watch::Sender<bool>>,
    released: Arc<tokio::sync::Semaphore>,
}

impl ActorExitGate {
    fn new() -> Self {
        Self {
            held: Arc::new(std::sync::Mutex::new(None)),
            reached: Arc::new(tokio::sync::watch::channel(false).0),
            released: Arc::new(tokio::sync::Semaphore::new(0)),
        }
    }

    /// Hold the next exit of an actor for `session`.
    fn hold(&self, session: &SessionId) {
        *self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session.clone());
    }

    /// Wait until the held actor waits at its exit.
    async fn wait_reached(&self) {
        let mut reached = self.reached.subscribe();
        tokio::time::timeout(STEP, reached.wait_for(|reached| *reached))
            .await
            .expect("the held actor reaches its exit")
            .expect("the exit gate is alive");
    }

    /// Let the held actor exit.
    fn release(&self) {
        self.released.add_permits(1);
    }

    async fn pass(&self, session: &SessionId) {
        let held = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take_if(|held| held == session)
            .is_some();
        if held {
            self.reached.send_replace(true);
            if let Ok(permit) = self.released.acquire().await {
                permit.forget();
            }
        }
    }
}

/// The persistent mock agent, with its exit passing the fixture's gate.
struct ExitGatedAgent {
    inner: PersistentMockAgent,
    gate: ActorExitGate,
}

#[async_trait]
impl SessionAgent for ExitGatedAgent {
    async fn run_with_events(
        &mut self,
        prompt: meerkat_core::types::ContentInput,
        event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.inner.run_with_events(prompt, event_tx).await
    }

    fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
        self.inner.set_skill_references(refs);
    }

    fn set_turn_tool_overlay(
        &mut self,
        overlay: Option<meerkat_core::service::TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.inner.set_turn_tool_overlay(overlay)
    }

    fn cancel(&mut self) {
        self.inner.cancel();
    }

    fn observed_session_tail(&self) -> meerkat_core::pending_continuation::ObservedSessionTailKind {
        self.inner.observed_session_tail()
    }

    fn hot_swap_llm_identity(
        &mut self,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        identity: meerkat_core::SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.inner
            .hot_swap_llm_identity(client, identity, request_policy)
    }

    fn session_id(&self) -> SessionId {
        self.inner.session_id()
    }

    fn snapshot(&self) -> SessionSnapshot {
        self.inner.snapshot()
    }

    fn session_clone(&self) -> Result<Session, meerkat_core::error::AgentError> {
        self.inner.session_clone()
    }

    fn session_transcript_authority(
        &self,
    ) -> Result<
        meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot,
        meerkat_core::error::AgentError,
    > {
        self.inner.session_transcript_authority()
    }

    fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
        self.inner.durable_llm_identity()
    }

    fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
        self.inner.transient_turn_context_state()
    }

    async fn release_owned_resources(&mut self) {
        self.inner.release_owned_resources().await;
        let session = self.inner.session_id();
        self.gate.pass(&session).await;
    }
}

struct ExitGatedBuilder {
    gate: ActorExitGate,
}

#[async_trait]
impl SessionAgentBuilder for ExitGatedBuilder {
    type Agent = ExitGatedAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        Ok(ExitGatedAgent {
            inner: PersistentMockBuilder.build_agent(req, event_tx).await?,
            gate: self.gate.clone(),
        })
    }
}

/// In-memory mob stores with a frozen identity clock, a session service over a
/// runtime store that can park a session's unregister finalization, and the
/// definition every incarnation runs.
struct LeaseFixture {
    definition: MobDefinition,
    events: Arc<dyn MobEventStore>,
    runs: Arc<InMemoryMobRunStore>,
    specs: Arc<InMemoryMobSpecStore>,
    runtime_metadata: Arc<InMemoryMobRuntimeMetadataStore>,
    identity: Arc<InMemoryMobIdentityStore>,
    identity_status: Arc<InMemoryMobIdentityStatusStore>,
    service: Arc<meerkat_session::PersistentSessionService<ExitGatedBuilder>>,
    runtime_store: Arc<FaultInjectingRuntimeStore>,
    exit_gate: ActorExitGate,
    /// SQLite mob stores that replace the in-memory ones, when set.
    persistent: Option<MobStorage>,
}

impl LeaseFixture {
    fn new(label: &str) -> Self {
        let system_now_ms = meerkat_core::time_compat::SystemTime::now()
            .duration_since(meerkat_core::time_compat::SystemTime::UNIX_EPOCH)
            .expect("system clock after the epoch")
            .as_millis();
        let clock = Arc::new(FrozenIdentityClock {
            now_ms: u64::try_from(system_now_ms).expect("system clock in range") + 3_600_000,
        });
        let event_store = InMemoryMobEventStore::new();
        let identity = Arc::new(InMemoryMobIdentityStore::paired_with_event_store(
            &event_store,
            clock,
        ));
        let runtime_store = FaultInjectingRuntimeStore::wrapping(Arc::new(
            meerkat_runtime::InMemoryRuntimeStore::new(),
        ));
        let session_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let exit_gate = ActorExitGate::new();
        let service = Arc::new(meerkat_session::PersistentSessionService::new(
            ExitGatedBuilder {
                gate: exit_gate.clone(),
            },
            16,
            session_store,
            Arc::clone(&runtime_store) as Arc<dyn meerkat_runtime::RuntimeStore>,
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ));
        Self {
            definition: with_unique_mob_id(sample_definition(), label),
            events: Arc::new(event_store),
            runs: Arc::new(InMemoryMobRunStore::new()),
            specs: Arc::new(InMemoryMobSpecStore::new()),
            runtime_metadata: Arc::new(InMemoryMobRuntimeMetadataStore::new()),
            identity,
            identity_status: Arc::new(InMemoryMobIdentityStatusStore::new()),
            service,
            runtime_store,
            exit_gate,
            persistent: None,
        }
    }

    /// The fixture over SQLite mob stores at `store` (the system clock).
    fn persistent(label: &str, store: &std::path::Path) -> Self {
        let mut fixture = Self::new(label);
        fixture.persistent = Some(MobStorage::persistent(store).expect("persistent mob storage"));
        fixture
    }

    /// The runtime machine the session service owns.
    fn machine(&self) -> Arc<meerkat_runtime::MeerkatMachine> {
        MobSessionService::acquire_runtime_adapter(self.service.as_ref(), None)
            .expect("the service owns a runtime machine")
            .expect("a persistent service has a machine")
    }

    fn storage(&self) -> MobStorage {
        if let Some(storage) = &self.persistent {
            return storage.clone();
        }
        MobStorage {
            events: self.events.clone(),
            runs: self.runs.clone(),
            specs: self.specs.clone(),
            definition_projection_composition:
                crate::storage::DefinitionProjectionComposition::Independent,
            runtime_metadata: self.runtime_metadata.clone(),
            identity: self.identity.clone(),
            identity_member: Some(self.identity.clone()),
            identity_status: self.identity_status.clone(),
            identity_status_projection_order: Default::default(),
            realm_profiles: None,
            forked_participants: None,
        }
    }

    fn base_prompt() -> Arc<crate::StaticSpawnBasePromptSource> {
        Arc::new(crate::StaticSpawnBasePromptSource(
            "lease release test base prompt".to_string(),
        ))
    }

    /// Create the mob, spawn `members`, adopt each one's identity and wait
    /// until every identity converged under this incarnation's lease.
    async fn create(&self, members: &[&str]) -> (MobHandle, Vec<(AgentIdentity, SessionId)>) {
        let handle = MobBuilder::new(self.definition.clone(), self.storage())
            .with_session_service(self.service.clone())
            .with_spawn_base_prompt_source(Self::base_prompt())
            .create()
            .await
            .expect("create the lease test mob");
        let mut seated = Vec::with_capacity(members.len());
        for name in members {
            let member = AgentIdentity::from(*name);
            let mut spawn = SpawnMemberSpec::new(ProfileName::from("worker"), member.clone());
            spawn.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
            handle.spawn_spec(spawn).await.expect("spawn the member");
            let session_id = handle
                .resolve_bridge_session_id(&member)
                .await
                .expect("session-backed member");
            self.adopt(&handle, &member, &session_id).await;
            seated.push((member, session_id));
        }
        for (member, _) in &seated {
            if let Err(latest) = wait_converged(&handle, member, 0).await {
                panic!("{member}: the adopted identity converges; latest status: {latest}");
            }
        }
        (handle, seated)
    }

    async fn adopt(&self, handle: &MobHandle, member: &AgentIdentity, session_id: &SessionId) {
        let adoption = adoption(&self.definition.id, member, session_id);
        assert!(matches!(
            handle
                .adopt_member_identity_declaration(adoption)
                .await
                .expect("adopt the existing member")
                .adoption,
            crate::identity::IdentityAdoptionOutcome::Adopted {
                desired_revision: 1
            }
        ));
    }

    /// The next incarnation, resumed over the same stores.
    async fn resume(&self) -> MobHandle {
        tokio::time::timeout(
            STEP,
            MobBuilder::for_resume(self.storage())
                .with_session_service(self.service.clone())
                .with_spawn_base_prompt_source(Self::base_prompt())
                .notify_orchestrator_on_resume(false)
                .resume(),
        )
        .await
        .expect("the resume answers")
        .expect("resume the lease test mob")
    }

    /// Replace `member`'s lease with a live one of this mob's incarnation
    /// `incarnation`, as an earlier incarnation elsewhere left it.
    async fn hand_lease_to(&self, member: &AgentIdentity, incarnation: &str) -> IdentityLeaseClaim {
        let identity = self.storage().identity;
        let held = self
            .lease(member)
            .await
            .expect("a converged identity holds a lease");
        assert!(
            identity
                .release_identity_lease(&self.definition.id, member, &held)
                .await
                .expect("release the lease"),
            "{member}: the crashed incarnation's lease is released"
        );
        match identity
            .claim_or_renew_identity_lease(
                &self.definition.id,
                member,
                &format!("mob:{}", self.definition.id),
                incarnation,
                crate::identity::IDENTITY_LEASE_MAX_TTL_MS,
            )
            .await
            .expect("claim the lease")
        {
            crate::identity::IdentityLeaseClaimOutcome::Acquired(claim) => claim,
            other => panic!("{member}: the free lease is acquired: {other:?}"),
        }
    }

    /// The member's active identity lease, if any.
    async fn lease(&self, member: &AgentIdentity) -> Option<IdentityLeaseClaim> {
        match self
            .storage()
            .identity
            .observe_identity_lease(&self.definition.id, member)
            .await
            .expect("read the identity lease")
        {
            IdentityStoredObservation::Valid(record) => record.active,
            IdentityStoredObservation::Missing => None,
            other => panic!("{member}: the identity lease is unreadable: {other:?}"),
        }
    }
}

/// The adoption of `member`'s existing session as its identity's desired
/// state, converging by drain.
pub(super) fn adoption(
    mob_id: &MobId,
    member: &AgentIdentity,
    session_id: &SessionId,
) -> crate::identity::AdoptMemberIdentityDeclaration {
    crate::identity::AdoptMemberIdentityDeclaration {
        mob_id: mob_id.clone(),
        agent_identity: member.clone(),
        request_id: crate::identity::IdentityAdoptionId::new(format!("adopt-{member}"))
            .expect("valid adoption id"),
        precondition: crate::identity::IdentityAdoptionPrecondition::ExpectedAbsent,
        declaration_scope: crate::identity::IdentityDeclarationScopeId::new("example-policy")
            .expect("valid declaration scope"),
        declaration_revision: 1,
        session: crate::identity::DesiredSessionTarget {
            session_id: session_id.clone(),
            lineage_id: meerkat_core::SessionLineageId::for_session(session_id),
            lineage_generation: meerkat_core::SessionGeneration::INITIAL,
            authority_policy: crate::identity::DesiredSessionAuthorityPolicy::RequireExisting,
        },
        member: crate::identity::IdentityProfileMemberDeclaration {
            profile_name: ProfileName::from("worker"),
            profile_override: None,
            model_override: None,
            external_addressable_override: None,
            context: None,
            labels: None,
            additional_instructions: None,
            system_prompt_override: None,
            tool_access_policy: None,
            auth_binding: None,
            budget_limits: None,
            runtime_mode: Some(meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven),
            required_env_keys: Vec::new(),
            required_local_callback_tools: Vec::new(),
            execution: crate::identity::DesiredExecution::ControllingSession,
        },
        wiring_custody: crate::identity::IdentityWiringCustody::ExternalManaged,
        owned_wiring: BTreeSet::new(),
        convergence: crate::identity::IdentityConvergenceMode::Drain { max_wait_ms: 5_000 },
    }
}

/// Wait until `member`'s identity converged at intent revision 1 under a lease
/// epoch above `after_epoch`; the latest status when it does not within
/// [`STEP`].
async fn wait_converged(
    handle: &MobHandle,
    member: &AgentIdentity,
    after_epoch: u64,
) -> Result<(), String> {
    wait_converged_within(handle, member, after_epoch, STEP).await
}

/// [`wait_converged`] within `bound`.
pub(super) async fn wait_converged_within(
    handle: &MobHandle,
    member: &AgentIdentity,
    after_epoch: u64,
    bound: Duration,
) -> Result<(), String> {
    let converged =
        |status: &IdentityStoredObservation<crate::identity::IdentityConvergenceStatus>| {
            matches!(
                status,
                IdentityStoredObservation::Valid(crate::identity::IdentityConvergenceStatus {
                    intent_revision: Some(1),
                    decision: Some(IdentityReconcileDecision::Converged),
                    lease_epoch: Some(epoch),
                    ..
                }) if *epoch > after_epoch
            )
        };
    let waited = tokio::time::timeout(bound, async {
        while !converged(
            &handle
                .identity_convergence_status(member)
                .await
                .expect("read the convergence status"),
        ) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    match waited {
        Ok(()) => Ok(()),
        Err(_) => Err(format!(
            "{:?}",
            handle
                .identity_convergence_status(member)
                .await
                .expect("read the final convergence status")
        )),
    }
}

/// After a graceful Shutdown that unregistered the member, the next
/// incarnation takes the member's identity at once. Its lease never expires
/// here, so the next incarnation converges only because the Shutdown released
/// it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_next_incarnation_takes_an_identity_a_graceful_shutdown_released() {
    let fixture = LeaseFixture::new("lease-graceful-restart");
    let (handle, members) = fixture.create(&["worker-1"]).await;
    let member = &members[0].0;
    let previous = fixture
        .lease(member)
        .await
        .expect("a converged identity holds a live lease");

    let report = handle
        .shutdown_with_report(ShutdownOptions::default())
        .await
        .expect("the shutdown answers");
    assert!(
        matches!(
            report.members.get(member),
            Some(MemberShutdownOutcome::Unregistered)
        ),
        "the Shutdown unregisters the member: {report:?}"
    );

    let resumed = fixture.resume().await;
    let converged = wait_converged(&resumed, member, previous.epoch).await;
    let lease = fixture.lease(member).await;
    assert!(
        converged.is_ok(),
        "the next incarnation converges without waiting on the previous one's lease; \
         latest status: {converged:?}; lease: {lease:?}"
    );
    assert!(
        matches!(
            &lease,
            Some(claim) if claim.incarnation_id != previous.incarnation_id
                && claim.epoch > previous.epoch
        ),
        "the next incarnation holds the lease: {lease:?}"
    );
    resumed.shutdown().await.expect("shut the resumed mob down");
}

/// A Shutdown releases the lease of each member it reports unregistered, and
/// only theirs: a member whose unregister is still running at the Shutdown's
/// deadline keeps its lease unchanged, because its runtime is still being
/// torn down.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_releases_only_the_leases_of_members_it_unregistered() {
    let fixture = LeaseFixture::new("lease-partial-release");
    let (handle, members) = fixture.create(&["worker-1", "worker-2"]).await;
    let (settled, _) = &members[0];
    let (pending, pending_session) = &members[1];
    let pending_lease = fixture
        .lease(pending)
        .await
        .expect("a converged identity holds a live lease");
    // The pending member's unregister cannot finish while its finalization
    // is parked.
    fixture
        .runtime_store
        .park_unregister_finalizations(pending_session);

    let report = handle
        .shutdown_with_report(
            ShutdownOptions::default()
                .with_deadline(meerkat_core::time_compat::Instant::now() + Duration::from_secs(2)),
        )
        .await
        .expect("the shutdown answers");
    assert!(
        matches!(
            report.members.get(settled),
            Some(MemberShutdownOutcome::Unregistered)
        ),
        "{report:?}"
    );
    assert!(
        matches!(
            report.members.get(pending),
            Some(MemberShutdownOutcome::UnregisterPending { .. })
        ),
        "{report:?}"
    );
    let settled_lease = fixture.lease(settled).await;
    let kept_lease = fixture.lease(pending).await;
    fixture.runtime_store.release_unregister_finalizations();
    assert_eq!(
        settled_lease, None,
        "the unregistered member's lease is released"
    );
    assert_eq!(
        kept_lease,
        Some(pending_lease),
        "the member still unregistering keeps its lease"
    );
}

/// A mob that ends without a Shutdown, as a crash does, releases nothing: the
/// next incarnation waits on the previous one's lease, and its own Shutdown
/// leaves a lease it does not hold alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_that_ends_without_a_shutdown_keeps_its_identity_lease() {
    let fixture = LeaseFixture::new("lease-crash-keeps");
    let (handle, members) = fixture.create(&["worker-1"]).await;
    let member = &members[0].0;
    let previous = fixture
        .lease(member)
        .await
        .expect("a converged identity holds a live lease");

    crash_stop_and_release_routes(handle).await;
    assert_eq!(
        fixture.lease(member).await.as_ref(),
        Some(&previous),
        "a crash releases no lease"
    );

    let resumed = fixture.resume().await;
    let awaited = tokio::time::timeout(STEP, async {
        loop {
            if matches!(
                resumed
                    .identity_convergence_status(member)
                    .await
                    .expect("read the convergence status"),
                IdentityStoredObservation::Valid(crate::identity::IdentityConvergenceStatus {
                    decision: Some(IdentityReconcileDecision::AwaitLease),
                    ..
                })
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        awaited.is_ok(),
        "the next incarnation waits on the previous one's live lease"
    );
    resumed.shutdown().await.expect("shut the resumed mob down");
    assert_eq!(
        fixture.lease(member).await.as_ref(),
        Some(&previous),
        "a Shutdown never releases a lease another incarnation holds"
    );
}

/// A Shutdown whose deadline passes while the member's session actor still
/// lives keeps the member's lease: the actor holds the session's hosting claim
/// until it exits. The Shutdown has returned by the time the actor exits, so
/// the lease stays until it expires, and the next incarnation waits for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_keeps_the_lease_of_a_member_whose_session_actor_outlives_it() {
    let fixture = LeaseFixture::new("lease-actor-outlives");
    let (handle, members) = fixture.create(&["worker-1"]).await;
    let (member, session) = &members[0];
    let previous = fixture
        .lease(member)
        .await
        .expect("a converged identity holds a live lease");
    fixture.exit_gate.hold(session);

    let report = handle
        .shutdown_with_report(
            ShutdownOptions::default()
                .with_deadline(meerkat_core::time_compat::Instant::now() + Duration::from_secs(2)),
        )
        .await
        .expect("the shutdown answers");
    assert!(
        matches!(
            report.members.get(member),
            Some(MemberShutdownOutcome::Unregistered)
        ),
        "{report:?}"
    );
    fixture.exit_gate.wait_reached().await;
    assert_eq!(
        fixture.lease(member).await.as_ref(),
        Some(&previous),
        "the lease stays while the member's session actor lives"
    );

    fixture.exit_gate.release();
    tokio::time::timeout(STEP, fixture.service.await_removed_actor_exit(session))
        .await
        .expect("the released actor exits");
    assert_eq!(
        fixture.lease(member).await.as_ref(),
        Some(&previous),
        "nothing releases the lease after the Shutdown returned"
    );

    let resumed = fixture.resume().await;
    let awaited = tokio::time::timeout(STEP, async {
        loop {
            if matches!(
                resumed
                    .identity_convergence_status(member)
                    .await
                    .expect("read the convergence status"),
                IdentityStoredObservation::Valid(crate::identity::IdentityConvergenceStatus {
                    decision: Some(IdentityReconcileDecision::AwaitLease),
                    ..
                })
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        awaited.is_ok(),
        "the next incarnation waits for the kept lease"
    );
    resumed.shutdown().await.expect("shut the resumed mob down");
}

/// Within its deadline, a Shutdown releases a member's lease once the member's
/// session actor has exited, and never while the actor still lives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_releases_the_lease_once_the_member_session_actor_has_exited() {
    let fixture = LeaseFixture::new("lease-actor-exits");
    let (handle, members) = fixture.create(&["worker-1"]).await;
    let (member, session) = &members[0];
    let previous = fixture
        .lease(member)
        .await
        .expect("a converged identity holds a live lease");
    fixture.exit_gate.hold(session);
    let machine = fixture.machine();

    let shutdown = tokio::spawn({
        let handle = handle.clone();
        async move {
            handle
                .shutdown_with_report(ShutdownOptions::default().with_deadline(
                    meerkat_core::time_compat::Instant::now() + Duration::from_secs(10),
                ))
                .await
        }
    });
    fixture.exit_gate.wait_reached().await;
    tokio::time::timeout(STEP, async {
        while machine
            .current_session_registration_witness(session)
            .await
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the Shutdown unregisters the member's runtime");
    let released_early = tokio::time::timeout(Duration::from_secs(1), async {
        while fixture.lease(member).await.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        released_early.is_err(),
        "the lease is released only once the member's session actor has exited"
    );
    assert_eq!(fixture.lease(member).await.as_ref(), Some(&previous));

    fixture.exit_gate.release();
    let report = tokio::time::timeout(STEP, shutdown)
        .await
        .expect("the shutdown returns once the actor exited")
        .expect("the shutdown task")
        .expect("the shutdown answers");
    assert!(
        matches!(
            report.members.get(member),
            Some(MemberShutdownOutcome::Unregistered)
        ),
        "{report:?}"
    );
    assert_eq!(
        fixture.lease(member).await,
        None,
        "the lease is released once the member's session actor exited"
    );
}

/// The bound within which a restart takes the lease an exited process left:
/// well inside the 30 s lifetime a restart that cannot prove the exit waits
/// out.
const RECLAIM_BOUND: Duration = Duration::from_secs(2);

/// A marked incarnation this process did not mint: one of a process that has
/// exited, as far as a process holding the store's custody can tell.
fn exited_process_incarnation() -> String {
    format!(
        "{}{}",
        crate::runtime::identity_incarnation_custody::CUSTODY_MARK,
        meerkat_core::ops::OperationId::new()
    )
}

/// The restart waits on `held`: the identity reaches AwaitLease and the lease
/// is still `held` a reclaim bound later.
async fn assert_waits_out(
    fixture: &LeaseFixture,
    resumed: &MobHandle,
    member: &AgentIdentity,
    held: &IdentityLeaseClaim,
) {
    let awaited = tokio::time::timeout(STEP, async {
        while !matches!(
            resumed
                .identity_convergence_status(member)
                .await
                .expect("read the convergence status"),
            IdentityStoredObservation::Valid(crate::identity::IdentityConvergenceStatus {
                decision: Some(IdentityReconcileDecision::AwaitLease),
                ..
            })
        ) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(awaited.is_ok(), "{member}: the restart waits on the lease");
    tokio::time::sleep(RECLAIM_BOUND).await;
    assert_eq!(
        fixture.lease(member).await.as_ref(),
        Some(held),
        "{member}: the restart leaves the lease to expire"
    );
}

/// A restart over a persistent store takes at once the lease an earlier
/// incarnation left in a process that has exited, and the identity converges
/// under the restart's lease.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_takes_at_once_the_lease_an_exited_process_left() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = LeaseFixture::persistent("lease-exited-process", &dir.path().join("mob.db"));
    let (handle, members) = fixture.create(&["worker-1"]).await;
    let member = &members[0].0;
    crash_stop_and_release_routes(handle).await;
    let left = fixture
        .hand_lease_to(member, &exited_process_incarnation())
        .await;

    let resumed = fixture.resume().await;
    if let Err(latest) = wait_converged_within(&resumed, member, left.epoch, RECLAIM_BOUND).await {
        panic!(
            "{member}: the restart takes the exited process's lease at once; latest status: {latest}"
        );
    }
    resumed.shutdown().await.expect("shut the resumed mob down");
}

/// An earlier incarnation of this same process may still have work running
/// here: a restart in the process waits its lease out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_waits_out_a_lease_an_earlier_incarnation_of_its_process_holds() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = LeaseFixture::persistent("lease-same-process", &dir.path().join("mob.db"));
    let (handle, members) = fixture.create(&["worker-1"]).await;
    let member = &members[0].0;
    let held = fixture
        .lease(member)
        .await
        .expect("a converged identity holds a lease");
    crash_stop_and_release_routes(handle).await;

    let resumed = fixture.resume().await;
    assert_waits_out(&fixture, &resumed, member, &held).await;
    resumed.shutdown().await.expect("shut the resumed mob down");
}

/// An unmarked incarnation (minted without the store's custody, or by a
/// version before it) may belong to a live process: a restart waits its lease
/// out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_waits_out_the_lease_of_an_unmarked_incarnation() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = LeaseFixture::persistent("lease-unmarked", &dir.path().join("mob.db"));
    let (handle, members) = fixture.create(&["worker-1"]).await;
    let member = &members[0].0;
    crash_stop_and_release_routes(handle).await;
    let held = fixture
        .hand_lease_to(member, &meerkat_core::ops::OperationId::new().to_string())
        .await;

    let resumed = fixture.resume().await;
    assert_waits_out(&fixture, &resumed, member, &held).await;
    resumed.shutdown().await.expect("shut the resumed mob down");
}

/// An incarnation minted while another holder kept the store's custody is
/// unmarked: a later incarnation that does hold the custody still cannot
/// tell whether that one's process lives, so it waits its lease out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_waits_out_the_lease_of_an_incarnation_minted_while_custody_was_busy() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("mob.db");
    let fixture = LeaseFixture::persistent("lease-minted-busy", &store);
    let mut lock_path = std::fs::canonicalize(&store)
        .expect("the store file exists")
        .into_os_string();
    lock_path.push(".incarnation-custody");
    let other_holder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .expect("open the custody lock file");
    other_holder
        .try_lock()
        .expect("the other holder takes the custody");
    let (handle, members) = fixture.create(&["worker-1"]).await;
    let member = &members[0].0;
    let held = fixture
        .lease(member)
        .await
        .expect("a converged identity holds a lease");
    assert!(
        !held
            .incarnation_id
            .starts_with(crate::runtime::identity_incarnation_custody::CUSTODY_MARK),
        "{member}: an incarnation minted while the custody was busy is unmarked: {}",
        held.incarnation_id
    );
    crash_stop_and_release_routes(handle).await;
    // The busy holder is gone: the restart takes the custody, and its own
    // incarnation is marked.
    drop(other_holder);

    let resumed = fixture.resume().await;
    assert_waits_out(&fixture, &resumed, member, &held).await;
    resumed.shutdown().await.expect("shut the resumed mob down");
}

/// While another holder (a live process) keeps the store's custody, this
/// process cannot prove any incarnation's exit: a restart waits out even a
/// marked incarnation's lease.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_waits_out_every_lease_while_another_holder_keeps_the_custody() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("mob.db");
    let fixture = LeaseFixture::persistent("lease-custody-held", &store);
    let mut lock_path = std::fs::canonicalize(&store)
        .expect("the store file exists")
        .into_os_string();
    lock_path.push(".incarnation-custody");
    let other_holder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .expect("open the custody lock file");
    other_holder
        .try_lock()
        .expect("the other holder takes the custody");
    let (handle, members) = fixture.create(&["worker-1"]).await;
    let member = &members[0].0;
    crash_stop_and_release_routes(handle).await;
    let held = fixture
        .hand_lease_to(member, &exited_process_incarnation())
        .await;

    let resumed = fixture.resume().await;
    assert_waits_out(&fixture, &resumed, member, &held).await;
    resumed.shutdown().await.expect("shut the resumed mob down");
    drop(other_holder);
}

/// A store without a file has no custody to prove an exit with: a restart
/// waits out even a marked incarnation's lease.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_over_a_store_without_a_file_waits_out_every_lease() {
    let fixture = LeaseFixture::new("lease-no-file");
    let (handle, members) = fixture.create(&["worker-1"]).await;
    let member = &members[0].0;
    crash_stop_and_release_routes(handle).await;
    let held = fixture
        .hand_lease_to(member, &exited_process_incarnation())
        .await;

    let resumed = fixture.resume().await;
    assert_waits_out(&fixture, &resumed, member, &held).await;
    resumed.shutdown().await.expect("shut the resumed mob down");
}
