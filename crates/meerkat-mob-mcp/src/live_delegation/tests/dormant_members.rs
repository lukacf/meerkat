//! Restart reconciliation of ClientContext delegations on a host whose
//! members are seated but not materialized: the default under lazy restore
//! (`member_restore: on_demand`), a deferred identity, or a member whose
//! materialization was released. Such a member has no runtime registration,
//! and the restart capture must never fail on it, never wake it, and still
//! reconcile its durable operations exactly once.

#![allow(
    clippy::panic,
    reason = "a fixture step that cannot happen names its cause and fails the row"
)]

use super::*;
use meerkat_mob::MobSessionService as _;

/// Hang guard of the rows below. Each wait it bounds is woken by the event it
/// waits for; the bound only turns a hang into a failure.
const WAIT: std::time::Duration = std::time::Duration::from_secs(60);

const TASK: &str = "voice work owed across the restart";

fn turn_driven(identity: &AgentIdentity) -> meerkat_mob::SpawnMemberSpec {
    let mut spec = meerkat_mob::SpawnMemberSpec::new("delegate", identity.as_str());
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    spec
}

fn persistent_runtime() -> Arc<meerkat_runtime::MeerkatMachine> {
    Arc::new(
        meerkat_runtime::MeerkatMachine::persistent_without_blobs(Arc::new(
            meerkat_runtime::InMemoryRuntimeStore::new(),
        ))
        .expect("construct runtime authority"),
    )
}

/// A control plane without a provider binding. A preparation that gets past
/// restart reconciliation stops at its binding check; none of the rows below
/// runs a channel.
struct UnboundControl;

#[async_trait::async_trait]
impl ExperimentalGptLiveControlPlane for UnboundControl {
    async fn active_binding(&self, _session_id: &SessionId) -> Option<ProviderWebrtcBinding> {
        None
    }

    async fn next_observation(
        &self,
        _binding: &ProviderWebrtcBinding,
    ) -> Result<
        Option<ExperimentalGptLiveControlObservation>,
        meerkat_live::ProviderWebrtcBrokerError,
    > {
        Ok(None)
    }

    async fn append_session_context(
        &self,
        _authority: meerkat_runtime::live_execution::LiveContextAppendAuthority,
        _text: String,
    ) -> Result<
        meerkat::experimental_gpt_live::ExperimentalGptLiveAppendDispatch,
        ExperimentalGptLiveBridgeError,
    > {
        Err(ExperimentalGptLiveBridgeError::ActiveBindingUnavailable)
    }

    async fn release_delegation_context(
        &self,
        _authority: LiveDelegationResultDeliveryAuthority,
        _delegation: LiveSidebandDelegationRef,
        _text: String,
    ) -> Result<ExperimentalGptLiveResultDeliveryDispatch, ExperimentalGptLiveBridgeError> {
        Err(ExperimentalGptLiveBridgeError::ActiveBindingUnavailable)
    }

    async fn narrate_delegation(
        &self,
        _authority: meerkat_runtime::live_execution::LiveDelegationNarrationAuthority,
        _delegation: LiveSidebandDelegationRef,
        _text: String,
    ) -> Result<ExperimentalGptLiveNarrationDispatch, ExperimentalGptLiveBridgeError> {
        Err(ExperimentalGptLiveBridgeError::ActiveBindingUnavailable)
    }
}

/// A mob the mob state manages, on the local session service over a
/// persistent runtime, whose members' comms identities live under a scratch
/// root, so a member can be seated dormant: seated, with no session, no
/// build and no runtime registration until its first wake.
struct SeatedMembers {
    _keys: tempfile::TempDir,
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    mobs: Arc<crate::MobMcpState>,
    handle: MobHandle,
}

impl SeatedMembers {
    async fn new(label: &str) -> Self {
        let keys = tempfile::tempdir().expect("a member comms identity root");
        let mut sessions = crate::LocalSessionService::new();
        sessions.runtime_adapter = persistent_runtime();
        let runtime = Arc::clone(&sessions.runtime_adapter);
        let sessions = Arc::new(sessions);
        let mobs = Arc::new(
            crate::MobMcpState::new(sessions.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct the mob state"),
        );
        let definition = meerkat_mob::MobDefinition::implicit(label, "claude-sonnet-4-5");
        let mob_id = definition.id.clone();
        let handle = meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
            .with_session_service(sessions)
            .allow_ephemeral_sessions(true)
            .with_member_comms_identity_root(keys.path())
            .create()
            .await
            .expect("create the mob");
        mobs.try_mob_insert_handle(mob_id, handle.clone())
            .await
            .expect("the mob state manages the mob");
        Self {
            _keys: keys,
            runtime,
            mobs,
            handle,
        }
    }

    /// Spawn `identity` live and return its session.
    async fn spawn(&self, identity: &AgentIdentity) -> SessionId {
        self.handle
            .spawn_spec(turn_driven(identity))
            .await
            .expect("spawn a live member");
        self.handle
            .resolve_bridge_session_id(identity)
            .await
            .expect("the live member's session")
    }

    /// Seat `identity` dormant and return the session its first wake creates.
    async fn seat(&self, identity: &AgentIdentity) -> SessionId {
        let outcomes = self
            .handle
            .seat_dormant_members(vec![turn_driven(identity)])
            .await
            .expect("the seat batch answers");
        let [meerkat_mob::DormantSeatOutcome::Seated { session_id, .. }] = outcomes.as_slice()
        else {
            panic!("the member is seated dormant: {outcomes:?}");
        };
        session_id.clone()
    }
}

/// Row (i) and (iv): a member that is seated and not materialized has no
/// runtime registration. The restart capture completes over it, still
/// captures the operations of the members that are materialized, and wakes
/// nothing: the dormant member stays unmaterialized, with no runtime
/// registration and no session.
///
/// Before the fix the capture read the dormant member's session from the
/// runtime, which answers `NotReady { Destroyed }` for a session it holds no
/// registration for, and the whole capture failed: the reconciler retried it
/// at 1 Hz, forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_restart_capture_completes_over_a_dormant_member_and_leaves_it_dormant() {
    let fixture = SeatedMembers::new("restart-capture-dormant").await;
    let live = AgentIdentity::from("live-source");
    let dormant = AgentIdentity::from("dormant-source");
    let live_session = fixture.spawn(&live).await;
    let dormant_session = fixture.seat(&dormant).await;
    let admission = fixture
        .runtime
        .__test_admit_confirmed_live_delegation(
            &live_session,
            live.as_str(),
            meerkat_runtime::live_execution::LiveDelegationWorkerOwnership::ExistingMember,
            TASK,
        )
        .await
        .expect("a durable ClientContext operation on the live member");
    assert!(
        fixture.handle.member_unmaterialized(&dormant),
        "the seated member is not materialized"
    );
    assert!(
        !fixture.runtime.contains_session(&dormant_session).await,
        "the seated member has no runtime registration"
    );

    let coordinator = ExperimentalLiveDelegationCoordinator::new(
        Arc::clone(&fixture.runtime),
        Arc::clone(&fixture.mobs),
    );
    let inventory = coordinator
        .collect_client_context_restart_inventory()
        .await
        .expect("the restart capture completes while a member is dormant");
    assert!(
        inventory.contains(&live_session, admission.operation().operation_id()),
        "the live member's operation is captured: {inventory:?}"
    );

    // No eager wake: the capture read nothing of the dormant member.
    assert!(
        fixture.handle.member_unmaterialized(&dormant),
        "the capture leaves the dormant member unmaterialized"
    );
    assert_eq!(
        fixture.handle.member_materialization_for_test(&dormant),
        Some(meerkat_mob::machines::mob_machine::MemberMaterializationPhase::Dormant),
        "the capture wakes no member"
    );
    assert!(
        !fixture.runtime.contains_session(&dormant_session).await,
        "the capture registers no runtime for the dormant member"
    );
    assert!(
        fixture.handle.session_unborn(&dormant_session),
        "the capture creates no session for the dormant member"
    );
}

/// Row (iii): a live channel's preparation waits for the restart inventory
/// for a bounded time and is then refused typed; it never waits forever.
/// Here the inventory can never be captured: the mob state's persistent root
/// cannot be created, so every Mob restore fails. Paused time makes the
/// bound elapse at once.
///
/// Before the fix the preparation waited on the inventory with no bound, so
/// a live channel open on such a host never completed.
#[tokio::test(start_paused = true)]
async fn a_live_channel_preparation_is_refused_when_the_restart_inventory_cannot_be_captured() {
    let scratch = tempfile::tempdir().expect("a scratch root");
    let blocking_file = scratch.path().join("not-a-directory");
    std::fs::write(
        &blocking_file,
        b"the mob root cannot be created under a file",
    )
    .expect("write the blocking file");
    let mut sessions = crate::LocalSessionService::new();
    sessions.runtime_adapter = persistent_runtime();
    let runtime = Arc::clone(&sessions.runtime_adapter);
    let mobs = Arc::new(
        crate::MobMcpState::new(Arc::new(sessions), meerkat_mob::MobControlPrincipal::Owner)
            .expect("construct the mob state")
            .with_persistent_storage_root(Some(blocking_file.join("runtime"))),
    );
    let coordinator = compose_experimental_live_delegation_coordinator(runtime, mobs);
    let binding = test_runtime_binding(SessionId::new(), "live-inventory-pending", 1);
    let control: Arc<dyn ExperimentalGptLiveControlPlane> = Arc::new(UnboundControl);

    let prepared = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        coordinator.prepare_bound_channel(binding, control),
    )
    .await
    .expect("the preparation ends instead of waiting forever for the restart inventory");
    let refusal = prepared.expect_err("the preparation is refused");
    assert!(
        refusal.contains("restart inventory"),
        "the refusal names the restart inventory it waited for: {refusal}"
    );
}

/// A project-local factory root, removed with its fixture.
struct Workspace(std::path::PathBuf);

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A mob on the persistent session service. A member keeps its session when
/// it is released, so a released member is dormant, with no runtime
/// registration, and its next wake resumes its session and recovers its
/// durable machine state, ClientContext operations included.
struct DurableMembers {
    _workspace: Workspace,
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    mobs: Arc<crate::MobMcpState>,
    mob_id: meerkat_mob::MobId,
    handle: MobHandle,
}

impl DurableMembers {
    async fn new(label: &str) -> Self {
        let workspace = Workspace(std::path::PathBuf::from(format!(
            ".live-dormant-{label}-{}",
            uuid::Uuid::new_v4(),
        )));
        std::fs::create_dir(&workspace.0).expect("project-local test root");
        for name in ["user", "runtime", "project", "context"] {
            std::fs::create_dir(workspace.0.join(name)).expect("factory root");
        }
        let factory = meerkat::AgentFactory::new(workspace.0.join("factory"))
            .user_config_root(workspace.0.join("user"))
            .runtime_root(workspace.0.join("runtime"))
            .project_root(workspace.0.join("project"))
            .context_root(workspace.0.join("context"))
            .builtins(false)
            .comms(true);
        let mut builder = meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        )));
        let store = Arc::new(meerkat_store::MemoryStore::new());
        builder.default_session_store =
            Some(Arc::new(meerkat_store::StoreAdapter::new(store.clone())));
        let service = Arc::new(meerkat_session::PersistentSessionService::new(
            builder,
            8,
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ));
        let runtime = service
            .acquire_runtime_adapter(None)
            .expect("acquire runtime authority")
            .expect("runtime");
        let mobs = Arc::new(
            crate::MobMcpState::new(service, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct the mob state"),
        );
        let mob_id = mobs
            .mob_create_definition(meerkat_mob::MobDefinition::implicit(
                &format!("{label}-{}", uuid::Uuid::new_v4()),
                "gpt-5.5",
            ))
            .await
            .expect("create the mob");
        let handle = mobs.handle_for(&mob_id).await.expect("mob handle");
        Self {
            _workspace: workspace,
            runtime,
            mobs,
            mob_id,
            handle,
        }
    }

    /// Spawn `identity` live and return its session.
    async fn spawn(&self, identity: &AgentIdentity) -> SessionId {
        self.mobs
            .mob_spawn_spec(&self.mob_id, turn_driven(identity))
            .await
            .expect("spawn a live member");
        self.handle
            .resolve_bridge_session_id(identity)
            .await
            .expect("the live member's session")
    }

    /// Release `identity`'s materialization and wait until MobMachine holds
    /// it unmaterialized. A release refused retryably (the member's runtime
    /// binding has not landed yet) is asked again at the next machine state
    /// change, which is the one that records that binding.
    async fn release(&self, identity: &AgentIdentity) {
        let mut changes = self.handle.machine_state_changes();
        tokio::time::timeout(WAIT, async {
            loop {
                match self.handle.release_member(identity).await {
                    Ok(()) => break,
                    Err(meerkat_mob::MobError::LifecycleOperationPending { .. }) => {
                        changes
                            .changed()
                            .await
                            .expect("the mob keeps publishing its machine state");
                    }
                    Err(error) => panic!("release {identity}: {error}"),
                }
            }
            while !self.handle.member_unmaterialized(identity) {
                changes
                    .changed()
                    .await
                    .expect("the mob keeps publishing its machine state");
            }
        })
        .await
        .expect("the release settles");
    }
}

/// Row (ii) and (iv): a ClientContext operation of a member that is dormant
/// at the restart capture is not read there. It stays in the member's
/// durable machine state, the capture completes without waking the member,
/// and the operation is reconciled when the member is materialized again,
/// once.
///
/// The operation's worker is an owned member that ran its work to a terminal
/// before the source member was released; the provider channel was revoked,
/// as a restart leaves it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dormant_members_operation_is_reconciled_once_when_the_member_is_materialized() {
    let fixture = DurableMembers::new("restart-reconcile-at-wake").await;
    let source = AgentIdentity::from("voice-source");
    let worker = AgentIdentity::from("voice-worker");
    let source_session = fixture.spawn(&source).await;
    let admission = fixture
        .runtime
        .__test_admit_confirmed_live_delegation(
            &source_session,
            worker.as_str(),
            meerkat_runtime::live_execution::LiveDelegationWorkerOwnership::OwnedMember,
            TASK,
        )
        .await
        .expect("a durable ClientContext operation with an owned worker");
    let operation_id = admission.operation().operation_id().clone();
    let channel_id = admission
        .operation()
        .domain_correlation()
        .channel_id()
        .clone();
    let execution = DelegationExecutionService::new(fixture.handle.clone())
        .start(DelegationExecutionRequest::new_live(
            worker.clone(),
            TASK,
            BoundedResultSpec::new("voice", 256).expect("bound"),
            admission.clone(),
        ))
        .await
        .expect("start the owned worker");
    let binding = fixture
        .runtime
        .live_delegation_runtime_binding(&source_session, &channel_id)
        .await
        .expect("live binding");
    fixture
        .runtime
        .resolve_live_delegation_worker_start(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            &admission,
            true,
        )
        .await
        .expect("publish the worker start");
    assert!(
        matches!(
            execution.await_terminal().await.terminal(),
            DelegationTurnTerminal::Completed(_)
        ),
        "the owned worker completes its work"
    );
    fixture
        .runtime
        .abandon_live_open_admission(&source_session, &channel_id)
        .await
        .expect("revoke the provider channel, as a restart leaves it");
    fixture.release(&source).await;
    assert!(
        !fixture.runtime.contains_session(&source_session).await,
        "the released source member has no runtime registration"
    );

    let coordinator = Arc::new(ExperimentalLiveDelegationCoordinator::new(
        Arc::clone(&fixture.runtime),
        Arc::clone(&fixture.mobs),
    ));
    let inventory = coordinator
        .collect_client_context_restart_inventory()
        .await
        .expect("the restart capture completes while the source member is dormant");
    assert!(
        !inventory.contains(&source_session, &operation_id),
        "the dormant member's operation is not read at the capture: {inventory:?}"
    );

    let reconciler = try_arm_client_context_restart_reconciler(
        &coordinator.client_context_restart_reconciler_armed,
        Arc::clone(&coordinator),
        Arc::clone(&coordinator.client_context_restart_inventory_ready),
        ClientContextRestartReconcileTiming::SHIPPING,
    )
    .expect("arm restart reconciliation");
    tokio::time::timeout(
        WAIT,
        coordinator.client_context_restart_inventory_ready.wait(),
    )
    .await
    .expect("the restart inventory is ready while the source member is dormant");
    assert!(
        fixture.handle.member_unmaterialized(&source),
        "restart reconciliation wakes no member"
    );
    assert!(
        !fixture.runtime.contains_session(&source_session).await,
        "restart reconciliation registers no runtime for the dormant member"
    );
    assert!(
        !reconciler.is_finished(),
        "restart reconciliation still owes the dormant member's operation"
    );

    fixture
        .handle
        .ensure_member_live(&source)
        .await
        .expect("the member's wake materializes it");
    tokio::time::timeout(WAIT, reconciler)
        .await
        .expect("restart reconciliation ends once the member is materialized")
        .expect("restart reconciliation exits cleanly");
    let reconciled = fixture
        .runtime
        .live_delegation_recovery_snapshots(&source_session)
        .await
        .expect("the source member's operations")
        .into_iter()
        .find(|snapshot| snapshot.operation_id() == &operation_id)
        .expect("the reconciled operation stays in durable truth");
    assert_eq!(
        reconciled.phase(),
        meerkat_runtime::live_execution::LiveDelegationRecoveryPhase::Retired,
        "the restart reconciliation retired the operation's worker custody"
    );
    assert!(reconciled.late(), "the recovered terminal is recorded late");
    assert!(
        !reconciled.result_eligible(),
        "a recovered terminal is never delivered to a provider"
    );
    assert!(matches!(
        coordinator
            .reconcile_one_client_context_snapshot(
                &fixture.handle,
                &reconciled,
                tokio::time::Instant::now(),
            )
            .await,
        ExperimentalClientContextRestartDisposition::AlreadyReconciled
    ));
}
