//! Manual release qualification for a Meta-hosted input whose actor run never starts.
//!
//! The test blocks at the session-service boundary beneath Meta's real runtime
//! executor. Meerkat stages the input and invokes that executor, but no actor run
//! begins and no primitive is applied. The terminal is asserted only after
//! rebuilding the runtime and reading through its public session-service adapter.
//! It is ignored because the authoritative production watchdog uses a private
//! real-time runtime and intentionally takes one hour.

#![allow(clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_client::TestClient;
use meerkat_core::{
    AppendSystemContextRequest, AppendSystemContextResult, CommsRuntime, CreateSessionRequest,
    EventStream, RunResult, SessionControlError, SessionError, SessionHistoryPage,
    SessionHistoryQuery, SessionId, SessionQuery, SessionService, SessionServiceCommsExt,
    SessionServiceControlExt, SessionServiceHistoryExt, SessionSummary, SessionView,
    StartTurnRequest, StreamError,
};
use meerkat_meta::{
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, UnifiedRuntime,
};
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::{MobDefinition, MobId, MobSessionService, MobStorage, SpawnMemberSpec};
use meerkat_runtime::input::{Input, PromptInput};
use meerkat_runtime::input_state::{
    InputAbandonReason, InputLifecycleState, InputTerminalOutcome, StoredInputState,
};
use meerkat_runtime::{IdempotencyKey, SessionServiceRuntimeExt};
use tokio::sync::Notify;

const IDEMPOTENCY_KEY: &str = "meta-never-executed-durable";
const MEMBER_ID: &str = "blocked-worker";

struct NeverStartsActorRunService {
    inner: Arc<dyn MobSessionService>,
    attempted: Arc<Notify>,
}

#[async_trait::async_trait]
impl SessionService for NeverStartsActorRunService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        self.inner.create_session(req).await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.inner.start_turn(id, req).await
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.interrupt(id).await
    }

    async fn interrupt_run_if_current(
        &self,
        id: &SessionId,
        expected_run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<bool, SessionError> {
        self.inner
            .interrupt_run_if_current(id, expected_run_id)
            .await
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        self.inner.read(id).await
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        self.inner.list(query).await
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.archive(id).await
    }

    async fn subscribe_session_events(&self, id: &SessionId) -> Result<EventStream, StreamError> {
        SessionService::subscribe_session_events(self.inner.as_ref(), id).await
    }
}

#[async_trait::async_trait]
impl SessionServiceCommsExt for NeverStartsActorRunService {
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CommsRuntime>> {
        self.inner.comms_runtime(session_id).await
    }
}

#[async_trait::async_trait]
impl SessionServiceControlExt for NeverStartsActorRunService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        self.inner.append_system_context(id, req).await
    }
}

#[async_trait::async_trait]
impl SessionServiceHistoryExt for NeverStartsActorRunService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        self.inner.read_history(id, query).await
    }
}

#[async_trait::async_trait]
impl MobSessionService for NeverStartsActorRunService {
    async fn subscribe_session_activity(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_mob::MemberSessionActivity, meerkat_core::SessionError> {
        self.inner.subscribe_session_activity(session_id).await
    }

    // Forwarded to the wrapped service, like the production decorators
    // (required since meerkat 0.8.47).
    async fn observe_live_durable_source(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_mob::LiveDurableSourceObservation, meerkat_core::SessionError> {
        self.inner.observe_live_durable_source(session_id).await
    }

    async fn observe_member_status_view(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_mob::MemberStatusSessionView, meerkat_core::SessionError> {
        self.inner.observe_member_status_view(session_id).await
    }

    async fn commit_live_delegation_final_transcript(
        &self,
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
        self.inner
            .commit_live_delegation_final_transcript(machine, session_id, provisional, final_event)
            .await
    }

    // meerkat 0.8.41 made `fork_persisted_session_at_turn_boundary` REQUIRED so
    // every wrapper forwards the boundary-then-fork as ONE contract to the owner.
    // This decorator owns no session authority, so it forwards to `inner`.
    async fn commit_live_delegation_final_transcript_at_turn_boundary(
        &self,
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &meerkat_core::types::SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
        bound: std::time::Duration,
    ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
        self.inner
            .commit_live_delegation_final_transcript_at_turn_boundary(
                machine,
                session_id,
                provisional,
                final_event,
                bound,
            )
            .await
    }

    async fn fork_persisted_session_at_turn_boundary(
        &self,
        source_session_id: &meerkat_core::types::SessionId,
        message_count: Option<usize>,
        tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        target: meerkat_core::DurableSessionForkTarget,
        bound: std::time::Duration,
    ) -> Result<meerkat_core::DurableForkAtTurnBoundary, SessionError> {
        self.inner
            .fork_persisted_session_at_turn_boundary(
                source_session_id,
                message_count,
                tool_access_policy,
                target,
                bound,
            )
            .await
    }

    async fn load_session_for_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_mob::ResumeSessionLoad, SessionError> {
        self.inner.load_session_for_resume(session_id).await
    }

    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: CreateSessionRequest,
    ) -> Result<RunResult, SessionError> {
        self.inner
            .create_session_under_runtime_turn_boundary(req)
            .await
    }

    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        req: CreateSessionRequest,
        resume_preparation: Option<meerkat_mob::SessionResumePreparationReceipt>,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<RunResult, SessionError> {
        self.inner
            .create_session_with_actor_witness_under_runtime_turn_boundary(
                req,
                resume_preparation,
                actor_witness_slot,
            )
            .await
    }

    async fn observe_session_resume_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_mob::SessionResumeAuthority, SessionError> {
        self.inner
            .observe_session_resume_authority(session_id)
            .await
    }

    async fn materialize_session_resume_verdict(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_mob::SessionResumeVerdict, SessionError> {
        self.inner
            .materialize_session_resume_verdict(session_id)
            .await
    }

    async fn apply_runtime_turn(
        &self,
        _session_id: &SessionId,
        _run_id: meerkat_core::lifecycle::RunId,
        _req: StartTurnRequest,
        _boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        _contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
        self.attempted.notify_one();
        std::future::pending().await
    }

    async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
        authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), SessionError> {
        self.inner
            .acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
                session_id, authority,
            )
            .await
    }

    async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
        &self,
        session_id: &SessionId,
        runtime_adapter: &meerkat_runtime::MeerkatMachine,
    ) -> Result<usize, SessionError> {
        self.inner
            .enqueue_committed_parent_session_boundary_after_runtime_turn(
                session_id,
                runtime_adapter,
            )
            .await
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
            .await
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
        &self,
        session_id: &SessionId,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<(), SessionError> {
        self.inner
            .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
                session_id, deadline,
            )
            .await
    }

    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .discard_live_session_under_runtime_turn_boundary(session_id)
            .await
    }

    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        self.inner
            .discard_live_session_actor_under_runtime_turn_boundary(witness)
            .await
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        self.inner.discard_live_session(session_id).await
    }

    fn supports_persistent_sessions(&self) -> bool {
        self.inner.supports_persistent_sessions()
    }

    fn acquire_runtime_adapter(
        &self,
        explicit: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    ) -> Result<Option<Arc<meerkat_runtime::MeerkatMachine>>, meerkat_runtime::RuntimeDriverError>
    {
        self.inner.acquire_runtime_adapter(explicit)
    }

    async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
        self.inner.session_belongs_to_mob(session_id, mob_id).await
    }

    async fn cancel_all_checkpointers(&self) {
        self.inner.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.inner.rearm_all_checkpointers().await;
    }
}

async fn boot_runtime(
    state: &Path,
    refuse_actor_runs: bool,
) -> (UnifiedRuntime, Option<Arc<Notify>>) {
    std::fs::create_dir_all(state).expect("state directory");
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))
            .expect("runtime store"),
    );
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let factory = AgentFactory::new(state)
        .session_store(session_store.clone())
        .comms(true)
        .builtins(false);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_blob_store = Some(blob_store.clone());
    let service: Arc<dyn MobSessionService> =
        Arc::new(meerkat_session::PersistentSessionService::new(
            builder,
            1,
            session_store,
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        ));
    let attempted = refuse_actor_runs.then(|| Arc::new(Notify::new()));
    let service: Arc<dyn MobSessionService> = if let Some(attempted) = attempted.as_ref() {
        Arc::new(NeverStartsActorRunService {
            inner: service,
            attempted: Arc::clone(attempted),
        })
    } else {
        service
    };
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(runtime_store, blob_store)
            .expect("acquire the never-executed persistence fixture runtime machine"),
    );
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "never-executed-persistence"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#,
    )
    .expect("mob definition");
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), service)
        .with_session_runtime_adapter(machine)
        .expect("acquire the fixture session runtime owner")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "never-executed-persistence".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };

    (
        UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
            .await
            .expect("Meta runtime bootstrap"),
        attempted,
    )
}

fn runtime_machine(runtime: &UnifiedRuntime) -> Arc<meerkat_runtime::MeerkatMachine> {
    let service = runtime
        .mob_runtime()
        .session_service()
        .expect("Meta runtime exposes its session service");
    MobSessionService::acquire_runtime_adapter(service.as_ref(), None)
        .expect("acquire the Meta session service's runtime adapter")
        .expect("Meta session service exposes its runtime adapter")
}

fn assert_never_executed(stored: &StoredInputState, expected_input_id: &meerkat_core::InputId) {
    assert_eq!(&stored.state.input_id, expected_input_id);
    assert_eq!(stored.seed.phase, InputLifecycleState::Abandoned);
    assert_eq!(
        stored.seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::NeverExecuted,
        }),
        "restart must preserve the authoritative NeverExecuted terminal instead of \
         silently upgrading it to completion or relabeling it as Cancelled"
    );
    assert_ne!(
        stored.seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Cancelled,
        }),
        "a run the runtime never executed was not cancelled by its caller"
    );
}

#[tokio::test]
#[ignore = "real 3600s production run-start watchdog"]
async fn never_executed_terminal_survives_meta_runtime_restart() {
    let temp = tempfile::tempdir().expect("state tempdir");
    let state = temp.path().join("state");
    let (first, attempted) = boot_runtime(&state, true).await;
    first
        .spawn(SpawnMemberSpec::new(
            "worker",
            AgentIdentity::from(MEMBER_ID),
        ))
        .await
        .expect("spawn the real Meta member");
    let session_id = first
        .mob_handle()
        .resolve_bridge_session_id(&AgentIdentity::from(MEMBER_ID))
        .await
        .expect("spawned member has a bridge session");
    let first_adapter = runtime_machine(&first);

    let mut prompt = PromptInput::new("work that must never be reported as completed", None);
    prompt.header.idempotency_key = Some(IdempotencyKey::new(IDEMPOTENCY_KEY));
    let input = Input::Prompt(prompt);
    let input_id = input.id().clone();
    let (accepted, completion) = SessionServiceRuntimeExt::accept_input_with_completion(
        first_adapter.as_ref(),
        &session_id,
        input,
    )
    .await
    .expect("submit the durable input");
    assert!(accepted.is_accepted(), "the input must be newly admitted");
    let completion = completion.expect("accepted prompt carries a completion waiter");

    let mut staged = false;
    for _ in 0..128 {
        tokio::task::yield_now().await;
        let state =
            SessionServiceRuntimeExt::input_state(first_adapter.as_ref(), &session_id, &input_id)
                .await
                .expect("observe the submitted input before restart")
                .expect("the submitted input must be tracked");
        if state.seed.phase == InputLifecycleState::Staged {
            staged = true;
            break;
        }
    }
    assert!(
        staged,
        "the input must be staged before the actor run is refused"
    );
    attempted
        .expect("blocking service exposes its attempt signal")
        .notified()
        .await;

    let (terminalized, completed) = tokio::time::timeout(Duration::from_secs(3_700), async {
        let terminalized = loop {
            let state = SessionServiceRuntimeExt::input_state(
                first_adapter.as_ref(),
                &session_id,
                &input_id,
            )
            .await
            .expect("observe the input after the execution-start bound")
            .expect("the submitted input must remain tracked");
            if state.seed.phase == InputLifecycleState::Abandoned {
                break state;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        let completed = completion
            .wait()
            .await
            .expect("completion waiter must resolve through runtime authority");
        (terminalized, completed)
    })
    .await
    .expect("the production watchdog must terminalize the staged input within 3700 seconds");
    assert_never_executed(&terminalized, &input_id);
    assert!(
        matches!(
            completed,
            meerkat_runtime::completion::CompletionOutcome::Abandoned { .. }
                | meerkat_runtime::completion::CompletionOutcome::AbandonedWithError { .. }
        ),
        "the live completion must remain abandoned, got {completed:?}"
    );

    let shutdown = first.shutdown().await;
    assert!(
        shutdown.cleanup_completed(),
        "first Meta runtime must shut down cleanly before restart: {shutdown:?}"
    );
    drop(first_adapter);

    let (restarted, _) = boot_runtime(&state, false).await;
    let restarted_adapter = runtime_machine(&restarted);
    let by_id =
        SessionServiceRuntimeExt::input_state(restarted_adapter.as_ref(), &session_id, &input_id)
            .await
            .expect("read durable input state by id after restart")
            .expect("the exact durable input survives restart");
    let by_key = SessionServiceRuntimeExt::input_state_by_idempotency_key(
        restarted_adapter.as_ref(),
        &session_id,
        IDEMPOTENCY_KEY,
    )
    .await
    .expect("read durable input state by idempotency key after restart")
    .expect("the durable idempotency binding survives restart");

    assert_never_executed(&by_id, &input_id);
    assert_never_executed(&by_key, &input_id);
    assert_eq!(
        by_id.state.input_id, by_key.state.input_id,
        "both public selectors must resolve the one durable terminal"
    );

    let shutdown = restarted.shutdown().await;
    assert!(
        shutdown.cleanup_completed(),
        "restarted Meta runtime must shut down cleanly: {shutdown:?}"
    );
}
