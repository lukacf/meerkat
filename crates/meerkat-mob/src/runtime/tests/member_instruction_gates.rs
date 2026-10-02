#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! Member-level safe-boundary instruction activation.
//!
//! A restored member's standing instructions cannot change through build
//! options: resume inherits persisted prompt state. These gates drive the
//! member-level door onto the native activation seam with a real factory, a
//! real persistent session service over SQLite session and runtime stores,
//! and a member restored onto an existing durable session through a Resume
//! launch. Turns are held at a deterministic barrier in the model double.

use super::*;
use crate::MemberInstructionActivationError;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::service::MobToolAuthorityContext;
use meerkat_core::{
    AgentExecutionSnapshot, ExternalToolSurfaceSnapshot, InputId, PeerIngressRuntimeSnapshot,
    RunApplyBoundary, SessionHistoryPage, SessionHistoryQuery, SessionTranscriptRevisionList,
    SessionTranscriptRevisionListQuery, SessionTranscriptRevisionPage,
    SessionTranscriptRevisionQuery, StageToolResultsRequest, StageToolResultsResult,
    ToolScopeSnapshot,
};
use meerkat_core::{
    InstructionActivationAdmissionErrorCode, InstructionActivationDisposition,
    InstructionActivationExpectation, InstructionActivationId, InstructionActivationReadQuery,
    InstructionActivationRequest, InstructionContentDigest, InstructionKey, InstructionNamespace,
    InstructionRevisionId, InstructionRevisionRef,
};
use meerkat_runtime::RuntimeStore;

const WAIT: Duration = Duration::from_secs(30);
const BODY: &str = "Always state your uncertainty explicitly.";

/// Records every model request and holds each reply until released.
#[derive(Clone)]
struct ScriptedClient {
    requests: Arc<Mutex<Vec<meerkat_client::LlmRequest>>>,
    released: tokio::sync::watch::Sender<bool>,
    started: tokio::sync::watch::Sender<usize>,
}

impl ScriptedClient {
    fn new() -> Self {
        let (released, _) = tokio::sync::watch::channel(true);
        let (started, _) = tokio::sync::watch::channel(0);
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            released,
            started,
        }
    }

    fn block(&self) {
        self.released.send_replace(false);
    }

    fn release(&self) {
        self.released.send_replace(true);
    }

    fn requests(&self) -> Vec<meerkat_client::LlmRequest> {
        self.requests.lock().expect("recorded requests").clone()
    }

    /// Wait until `count` model requests have started (a typed signal from
    /// the double, not a clock).
    async fn wait_started(&self, count: usize) {
        let mut started = self.started.subscribe();
        tokio::time::timeout(WAIT, started.wait_for(|seen| *seen >= count))
            .await
            .expect("the model request starts")
            .expect("started channel open");
    }
}

#[async_trait]
impl meerkat_client::LlmClient for ScriptedClient {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let sequence = {
            let mut requests = self.requests.lock().expect("record actual request");
            requests.push(request.clone());
            requests.len()
        };
        self.started.send_replace(sequence);
        let mut released = self.released.subscribe();
        Box::pin(async_stream::try_stream! {
            while !*released.borrow_and_update() {
                if released.changed().await.is_err() {
                    break;
                }
            }
            yield meerkat_client::LlmEvent::TextDelta {
                delta: format!("answer-{sequence}"),
                meta: None,
            };
            yield meerkat_client::LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::OpenAI,
                    &request.model,
                    Usage::default(),
                ),
            };
            yield meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            };
        })
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

type Service = meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>;

fn request(
    activation: &str,
    expectation: InstructionActivationExpectation,
) -> InstructionActivationRequest {
    InstructionActivationRequest {
        revision: InstructionRevisionRef {
            namespace: InstructionNamespace::new("host.standing").expect("namespace"),
            key: InstructionKey::new("member").expect("key"),
            revision_id: InstructionRevisionId::new(format!("rev-{activation}")).expect("revision"),
            content_sha256: InstructionContentDigest::for_body(BODY),
        },
        activation_id: InstructionActivationId::new(activation).expect("activation id"),
        expectation,
        supersedes: None,
        body: BODY.to_string(),
    }
}

fn system_rows_with_body(request: &meerkat_client::LlmRequest) -> usize {
    request
        .messages
        .iter()
        .filter(
            |message| matches!(message, Message::System(system) if system.content.contains(BODY)),
        )
        .count()
}

struct Fixture {
    _root: tempfile::TempDir,
    handle: MobHandle,
    identity: AgentIdentity,
    client: ScriptedClient,
}

impl Fixture {
    /// A turn-driven member restored onto a durable session that existed
    /// before the member: the case build-time instructions cannot reach.
    async fn restored_member() -> Self {
        Self::restored_member_through(false).await
    }

    /// [`Self::restored_member`], with the mob's session service optionally
    /// behind a decorator that forwards everything except the activation
    /// seam (the regression the typed default guards against).
    async fn restored_member_through(non_forwarding_decorator: bool) -> Self {
        let root = tempfile::Builder::new()
            .prefix("meerkat-member-instruction-")
            .tempdir()
            .expect("fixture storage in the system temp dir");
        let root_path = root.path().canonicalize().expect("absolute fixture root");
        for directory in ["config", "runtime", "project", "context"] {
            std::fs::create_dir_all(root_path.join(directory)).expect("fixture root");
        }
        let factory = meerkat::AgentFactory::new(root_path.join("factory-store"))
            .user_config_root(root_path.join("config"))
            .runtime_root(root_path.join("runtime"))
            .project_root(root_path.join("project"))
            .context_root(root_path.join("context"))
            .builtins(false)
            .comms(true);
        let mut config = meerkat::Config::default();
        config.agent.model = "gpt-5.5".to_string();
        config.compaction.auto_compact_threshold = 1_000_000;
        let client = ScriptedClient::new();
        let mut builder = meerkat::FactoryAgentBuilder::new(factory, config);
        builder.default_llm_client = Some(Arc::new(client.clone()));
        let db = root_path.join("realm.db");
        let session_store =
            Arc::new(meerkat_store::SqliteSessionStore::open(&db).expect("session store"));
        builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
            session_store.clone(),
        )));
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(
            meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&db)
                .expect("head-canonical runtime store"),
        );
        let service = Arc::new(Service::new(
            builder,
            16,
            session_store,
            runtime_store,
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ));
        let mut definition = with_unique_mob_id(sample_definition(), "member-instruction");
        let lead = definition
            .profiles
            .get_mut(&ProfileName::from("lead"))
            .and_then(ProfileBinding::as_inline_mut)
            .expect("inline lead");
        lead.model = "gpt-5.5".to_string();
        lead.runtime_mode = crate::MobRuntimeMode::TurnDriven;
        lead.tools = ToolConfig {
            comms: true,
            ..ToolConfig::default()
        };
        let mob_id = definition.id.clone();
        let mob_service: Arc<dyn MobSessionService> = if non_forwarding_decorator {
            Arc::new(NonForwardingDecorator {
                inner: service.clone(),
            })
        } else {
            service.clone()
        };
        let handle = MobBuilder::new(definition, MobStorage::in_memory())
            .with_session_service(mob_service)
            .with_default_llm_client(Arc::new(client.clone()))
            .create()
            .await
            .expect("create real turn-driven mob");

        // The durable session exists before the member: created, persisted
        // and its creating actor discarded, so the member adopts it through
        // a Resume launch instead of minting it.
        let identity = AgentIdentity::from("restored-member");
        let created = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "gpt-5.5".to_string(),
                prompt: "adopted session".to_string().into(),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    comms_name: Some(
                        super::actor::render_member_comms_name(
                            mob_id.as_str(),
                            "lead",
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
            .expect("persist the session the member restores");
        MobSessionService::discard_live_session(&*service, &created.session_id)
            .await
            .expect("discard the creating actor");
        handle
            .spawn_spec(
                SpawnMemberSpec::new(ProfileName::from("lead"), identity.clone()).with_launch_mode(
                    crate::launch::MemberLaunchMode::Resume {
                        bridge_session_id: created.session_id.clone(),
                        resume_from_role: None,
                    },
                ),
            )
            .await
            .expect("restore the member onto its durable session");
        assert_eq!(
            handle.resolve_bridge_session_id(&identity).await.as_ref(),
            Some(&created.session_id),
            "the member runs on the restored session"
        );
        Self {
            _root: root,
            handle,
            identity,
            client,
        }
    }

    async fn activate(
        &self,
        request: InstructionActivationRequest,
    ) -> Result<meerkat_core::InstructionActivationReceipt, MemberInstructionActivationError> {
        tokio::time::timeout(
            WAIT,
            self.handle
                .activate_member_instruction(&self.identity, request),
        )
        .await
        .expect("activation answers")
    }

    async fn records(&self) -> usize {
        self.handle
            .read_member_instruction_activations(
                &self.identity,
                InstructionActivationReadQuery::default(),
            )
            .await
            .expect("read activations")
            .records
            .len()
    }

    /// Start one turn and return its handle unawaited.
    async fn start_turn(&self, text: &str) -> WorkTurnHandle {
        let entry = self
            .handle
            .get_member(&self.identity)
            .await
            .expect("roster read")
            .expect("member binding");
        self.handle
            .start_work_with_mode(
                entry.agent_runtime_id,
                entry.fence_token,
                WorkRef::new(),
                WorkSpec::new(text, WorkOrigin::Internal),
                HandlingMode::Queue,
            )
            .await
            .expect("turn admitted")
    }

    async fn run_turn(&self, text: &str) {
        let turn = self.start_turn(text).await;
        tokio::time::timeout(WAIT, turn.wait())
            .await
            .expect("turn finishes")
            .expect("turn succeeds");
    }

    async fn finish(self) {
        self.client.release();
        let _ = tokio::time::timeout(WAIT, self.handle.shutdown()).await;
    }
}

/// An activation on a restored member applies durably and reaches the
/// member's next turn as an ordered System row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_activation_on_a_restored_member_reaches_its_next_turn() {
    let fixture = Fixture::restored_member().await;
    let receipt = fixture
        .activate(request(
            "standing-1",
            InstructionActivationExpectation::Absent,
        ))
        .await
        .expect("activation applies at a safe boundary");
    assert_eq!(
        receipt.disposition,
        InstructionActivationDisposition::Applied
    );
    assert_eq!(fixture.records().await, 1);

    let before = fixture.client.requests().len();
    fixture.run_turn("hello after the activation").await;
    let requests = fixture.client.requests();
    let next = requests
        .get(before)
        .expect("the member's next turn reached the model");
    assert_eq!(
        system_rows_with_body(next),
        1,
        "the next turn carries the activation exactly once"
    );
    fixture.finish().await;
}

/// Re-applying the effective activation is a typed Duplicate: no second
/// durable record and no accreting System row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reapplied_activation_is_a_typed_duplicate() {
    let fixture = Fixture::restored_member().await;
    let first = fixture
        .activate(request(
            "standing-1",
            InstructionActivationExpectation::Absent,
        ))
        .await
        .expect("first activation applies");
    assert_eq!(first.disposition, InstructionActivationDisposition::Applied);
    let again = fixture
        .activate(request(
            "standing-1",
            InstructionActivationExpectation::Absent,
        ))
        .await
        .expect("a re-apply answers typed");
    assert_eq!(
        again.disposition,
        InstructionActivationDisposition::Duplicate
    );
    assert_eq!(
        again.record, first.record,
        "the duplicate names the original record"
    );
    assert_eq!(fixture.records().await, 1);

    let before = fixture.client.requests().len();
    fixture.run_turn("hello after the re-apply").await;
    let requests = fixture.client.requests();
    assert_eq!(
        system_rows_with_body(requests.get(before).expect("next turn")),
        1,
        "a re-apply never adds a second System row"
    );
    fixture.finish().await;
}

/// An activation issued mid-turn never interleaves with that turn: it waits
/// for the member's runtime turn-finalization boundary, which the running
/// turn holds, and applies only after the turn ends. The held turn's own
/// model request carries no activation; the next turn carries it once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_activation_mid_turn_waits_for_the_safe_boundary_and_never_interleaves() {
    let fixture = Fixture::restored_member().await;
    fixture.client.block();
    let before = fixture.client.requests().len();
    let held = fixture.start_turn("a turn that holds the boundary").await;
    fixture.client.wait_started(before + 1).await;

    let activation = tokio::spawn({
        let handle = fixture.handle.clone();
        let identity = fixture.identity.clone();
        async move {
            handle
                .activate_member_instruction(
                    &identity,
                    request("standing-1", InstructionActivationExpectation::Absent),
                )
                .await
        }
    });
    fixture.client.release();
    tokio::time::timeout(WAIT, held.wait())
        .await
        .expect("held turn finishes")
        .expect("held turn succeeds");
    let receipt = tokio::time::timeout(WAIT, activation)
        .await
        .expect("activation answers once the boundary is free")
        .expect("activation task")
        .expect("activation applies at the boundary after the turn");
    assert_eq!(
        receipt.disposition,
        InstructionActivationDisposition::Applied
    );

    let requests = fixture.client.requests();
    assert_eq!(
        system_rows_with_body(requests.get(before).expect("held turn request")),
        0,
        "the held turn never saw the activation"
    );
    let next_index = requests.len();
    fixture.run_turn("hello after the boundary").await;
    let requests = fixture.client.requests();
    assert_eq!(
        system_rows_with_body(requests.get(next_index).expect("next turn request")),
        1,
        "the next turn carries the activation once"
    );
    fixture.finish().await;
}

/// A session-service decorator that forgets to forward the activation seam
/// gets the trait's typed default: the activation is refused as
/// `DurabilityUnavailable` and nothing is appended, never a silent success.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_decorator_that_does_not_forward_the_seam_is_refused_typed() {
    let fixture = Fixture::restored_member_through(true).await;
    let error = fixture
        .activate(request(
            "standing-1",
            InstructionActivationExpectation::Absent,
        ))
        .await
        .expect_err("a non-forwarding decorator never applies silently");
    assert_eq!(
        error.admission_code(),
        Some(InstructionActivationAdmissionErrorCode::DurabilityUnavailable),
        "{error:?}"
    );
    assert_eq!(fixture.records().await, 0, "nothing was appended");
    let before = fixture.client.requests().len();
    fixture.run_turn("hello through the decorator").await;
    assert_eq!(
        system_rows_with_body(fixture.client.requests().get(before).expect("next turn")),
        0
    );
    fixture.finish().await;
}

/// A member without a session binding is refused typed before any boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_member_is_refused_not_materialized() {
    let fixture = Fixture::restored_member().await;
    let error = fixture
        .handle
        .activate_member_instruction(
            &AgentIdentity::from("no-such-member"),
            request("standing-1", InstructionActivationExpectation::Absent),
        )
        .await
        .expect_err("no member, no activation");
    assert_eq!(
        error.admission_code(),
        Some(InstructionActivationAdmissionErrorCode::TargetNotMaterialized)
    );
    fixture.finish().await;
}

/// Forwards every session-service method to the real persistent service
/// except `activate_instruction_under_runtime_turn_boundary`, which keeps the
/// trait default. Generated mechanically from the trait signatures.
struct NonForwardingDecorator {
    inner: Arc<Service>,
}

#[async_trait]
impl meerkat_core::service::SessionService for NonForwardingDecorator {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        <Service as meerkat_core::service::SessionService>::create_session(&self.inner, req).await
    }
    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        <Service as meerkat_core::service::SessionService>::start_turn(&self.inner, id, req).await
    }
    async fn reconcile_runtime_compaction_projections(
        &self,
        id: &SessionId,
        intents: Vec<meerkat_core::CompactionProjectionIntent>,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::reconcile_runtime_compaction_projections(&self.inner, id, intents).await
    }
    async fn abort_uncommitted_compaction_projections(
        &self,
        id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::abort_uncommitted_compaction_projections(&self.inner, id).await
    }
    async fn abort_rejected_runtime_run_projections(
        &self,
        id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::abort_rejected_runtime_run_projections(
            &self.inner,
            id,
        )
        .await
    }
    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::interrupt(&self.inner, id).await
    }
    async fn interrupt_run_if_current(
        &self,
        id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
    ) -> Result<bool, SessionError> {
        <Service as meerkat_core::service::SessionService>::interrupt_run_if_current(
            &self.inner,
            id,
            expected_run_id,
        )
        .await
    }
    async fn cancel_after_boundary(&self, id: &SessionId) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::cancel_after_boundary(&self.inner, id)
            .await
    }
    async fn cancel_after_boundary_for_run(
        &self,
        id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::cancel_after_boundary_for_run(
            &self.inner,
            id,
            expected_run_id,
        )
        .await
    }
    async fn set_session_client(
        &self,
        id: &SessionId,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::set_session_client(
            &self.inner,
            id,
            client,
        )
        .await
    }
    async fn hot_swap_session_llm_identity(
        &self,
        id: &SessionId,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        identity: SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::hot_swap_session_llm_identity(
            &self.inner,
            id,
            client,
            identity,
            request_policy,
        )
        .await
    }
    async fn set_session_tool_visibility_state(
        &self,
        id: &SessionId,
        state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::set_session_tool_visibility_state(
            &self.inner,
            id,
            state,
        )
        .await
    }
    async fn update_session_mob_authority_context(
        &self,
        id: &SessionId,
        authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::update_session_mob_authority_context(
            &self.inner,
            id,
            authority_context,
        )
        .await
    }
    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        <Service as meerkat_core::service::SessionService>::has_live_session(&self.inner, id).await
    }
    async fn set_session_tool_filter(
        &self,
        id: &SessionId,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::set_session_tool_filter(
            &self.inner,
            id,
            filter,
        )
        .await
    }
    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        <Service as meerkat_core::service::SessionService>::read(&self.inner, id).await
    }
    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        <Service as meerkat_core::service::SessionService>::list(&self.inner, query).await
    }
    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::archive(&self.inner, id).await
    }
    async fn subscribe_session_events(&self, id: &SessionId) -> Result<EventStream, StreamError> {
        <Service as meerkat_core::service::SessionService>::subscribe_session_events(
            &self.inner,
            id,
        )
        .await
    }
    async fn subscribe_session_events_from(
        &self,
        id: &SessionId,
        cursor: meerkat_core::comms::SessionEventCursor,
    ) -> Result<meerkat_core::comms::SessionEventSubscription, StreamError> {
        <Service as meerkat_core::service::SessionService>::subscribe_session_events_from(
            &self.inner,
            id,
            cursor,
        )
        .await
    }
    async fn record_live_terminal_error(
        &self,
        id: &SessionId,
        cause: meerkat_core::live_adapter::LiveAdapterErrorCode,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::record_live_terminal_error(
            &self.inner,
            id,
            cause,
        )
        .await
    }
    async fn record_live_output_audio_degraded(
        &self,
        id: &SessionId,
        dropped: u64,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::record_live_output_audio_degraded(
            &self.inner,
            id,
            dropped,
        )
        .await
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceCommsExt for NonForwardingDecorator {
    async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        <Service as meerkat_core::service::SessionServiceCommsExt>::comms_runtime(
            &self.inner,
            session_id,
        )
        .await
    }
    async fn send_comms(
        &self,
        session_id: &SessionId,
        command: meerkat_core::CommsCommand,
    ) -> Option<Result<meerkat_core::SendReceipt, meerkat_core::SendError>> {
        <Service as meerkat_core::service::SessionServiceCommsExt>::send_comms(
            &self.inner,
            session_id,
            command,
        )
        .await
    }
    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        <Service as meerkat_core::service::SessionServiceCommsExt>::event_injector(
            &self.inner,
            session_id,
        )
        .await
    }
    #[doc(hidden)]
    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        <Service as meerkat_core::service::SessionServiceCommsExt>::interaction_event_injector(
            &self.inner,
            session_id,
        )
        .await
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceControlExt for NonForwardingDecorator {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        <Service as meerkat_core::service::SessionServiceControlExt>::append_system_context(
            &self.inner,
            id,
            req,
        )
        .await
    }
    async fn stage_tool_results(
        &self,
        id: &SessionId,
        req: StageToolResultsRequest,
    ) -> Result<StageToolResultsResult, SessionError> {
        <Service as meerkat_core::service::SessionServiceControlExt>::stage_tool_results(
            &self.inner,
            id,
            req,
        )
        .await
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceHistoryExt for NonForwardingDecorator {
    async fn read_instruction_activation_records(
        &self,
        id: &SessionId,
        query: meerkat_core::InstructionActivationReadQuery,
    ) -> Result<meerkat_core::InstructionActivationReadPage, SessionError> {
        <Service as meerkat_core::service::SessionServiceHistoryExt>::read_instruction_activation_records(&self.inner, id, query).await
    }
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        <Service as meerkat_core::service::SessionServiceHistoryExt>::read_history(
            &self.inner,
            id,
            query,
        )
        .await
    }
    async fn read_transcript_revision(
        &self,
        id: &SessionId,
        query: SessionTranscriptRevisionQuery,
    ) -> Result<SessionTranscriptRevisionPage, SessionError> {
        <Service as meerkat_core::service::SessionServiceHistoryExt>::read_transcript_revision(
            &self.inner,
            id,
            query,
        )
        .await
    }
    async fn list_transcript_revisions(
        &self,
        id: &SessionId,
        query: SessionTranscriptRevisionListQuery,
    ) -> Result<SessionTranscriptRevisionList, SessionError> {
        <Service as meerkat_core::service::SessionServiceHistoryExt>::list_transcript_revisions(
            &self.inner,
            id,
            query,
        )
        .await
    }
}

#[async_trait]
impl MobSessionService for NonForwardingDecorator {
    async fn append_system_notice_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
        record: meerkat_core::types::SystemNoticeRecord,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::append_system_notice_under_runtime_turn_boundary(
            &self.inner,
            session_id,
            record,
        )
        .await
    }
    async fn commit_live_delegation_final_transcript(
        &self,
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
        <Service as MobSessionService>::commit_live_delegation_final_transcript(
            &self.inner,
            machine,
            session_id,
            provisional,
            final_event,
        )
        .await
    }
    async fn commit_live_delegation_final_transcript_at_turn_boundary(
        &self,
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
        bound: std::time::Duration,
    ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
        <Service as MobSessionService>::commit_live_delegation_final_transcript_at_turn_boundary(
            &self.inner,
            machine,
            session_id,
            provisional,
            final_event,
            bound,
        )
        .await
    }
    async fn commit_live_delegation_represented_transcript_at_turn_boundary(
        &self,
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
        represented: Vec<meerkat_core::RepresentedLiveUserRow>,
        bound: std::time::Duration,
    ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
        <Service as MobSessionService>::commit_live_delegation_represented_transcript_at_turn_boundary(&self.inner, machine, session_id, provisional, final_event, represented, bound).await
    }
    #[cfg(feature = "openai-live")]
    async fn validate_live_bridge_member_eligibility(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::validate_live_bridge_member_eligibility(
            &self.inner,
            session_id,
        )
        .await
    }
    #[cfg(feature = "openai-live")]
    async fn capture_live_bridge_execution_snapshot(
        &self,
        session_id: &SessionId,
        agent_identity: &str,
    ) -> Result<super::LiveBridgeExecutionSnapshot, SessionError> {
        <Service as MobSessionService>::capture_live_bridge_execution_snapshot(
            &self.inner,
            session_id,
            agent_identity,
        )
        .await
    }
    #[cfg(feature = "openai-live")]
    async fn start_live_bridge_member_operation(
        &self,
        request: super::LiveBridgeOperationRequest,
        cancellation: super::LiveBridgeOperationCancellationSignal,
    ) -> Result<super::LiveBridgeOperationTerminalFuture, super::LiveBridgeOperationStartError>
    {
        <Service as MobSessionService>::start_live_bridge_member_operation(
            &self.inner,
            request,
            cancellation,
        )
        .await
    }
    fn forked_participant_source_runtime(
        self: Arc<Self>,
    ) -> Option<Arc<dyn crate::forked_participant::ForkedParticipantSourceRuntime>> {
        <Service as MobSessionService>::forked_participant_source_runtime(Arc::clone(&self.inner))
    }
    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Service as MobSessionService>::create_session_under_runtime_turn_boundary(&self.inner, req)
            .await
    }
    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        resume_preparation: Option<SessionResumePreparationReceipt>,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Service as MobSessionService>::create_session_with_actor_witness_under_runtime_turn_boundary(&self.inner, req, resume_preparation, actor_witness_slot).await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Service as MobSessionService>::create_session_with_machine_archived_resume_authority(
            &self.inner,
            req,
            authorization,
        )
        .await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Service as MobSessionService>::create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(&self.inner, req, authorization).await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
        resume_preparation: SessionResumePreparationReceipt,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Service as MobSessionService>::create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(&self.inner, req, authorization, resume_preparation, actor_witness_slot).await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn authorize_revivable_retired_session(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::PreparedArchivedResumeCommitLease,
    ) -> Result<meerkat_runtime::AuthorizedArchivedResumeCommitLease, SessionError> {
        <Service as MobSessionService>::authorize_revivable_retired_session(
            &self.inner,
            session_id,
            authority,
        )
        .await
    }
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        <Service as MobSessionService>::subscribe_session_events(&self.inner, session_id).await
    }
    async fn subscribe_agent_session_events_from(
        &self,
        session_id: &SessionId,
        cursor: meerkat_core::comms::SessionEventCursor,
    ) -> Result<AgentEventSubscription, StreamError> {
        <Service as MobSessionService>::subscribe_agent_session_events_from(
            &self.inner,
            session_id,
            cursor,
        )
        .await
    }
    fn supports_persistent_sessions(&self) -> bool {
        <Service as MobSessionService>::supports_persistent_sessions(&self.inner)
    }
    fn persisted_session_authority_read_cost(&self) -> PersistedSessionAuthorityReadCost {
        <Service as MobSessionService>::persisted_session_authority_read_cost(&self.inner)
    }
    async fn observe_persisted_session_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<crate::identity::IdentitySessionStoreAuthority>, SessionError> {
        <Service as MobSessionService>::observe_persisted_session_authority(&self.inner, session_id)
            .await
    }
    async fn subscribe_session_activity(
        &self,
        session_id: &SessionId,
    ) -> Result<MemberSessionActivity, SessionError> {
        <Service as MobSessionService>::subscribe_session_activity(&self.inner, session_id).await
    }
    async fn live_session_actor_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::live_session_actor_registered(&self.inner, session_id).await
    }
    async fn start_turn_with_admission_notification(
        &self,
        session_id: &SessionId,
        req: meerkat_core::service::StartTurnRequest,
        admitted: tokio::sync::oneshot::Sender<()>,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Service as MobSessionService>::start_turn_with_admission_notification(
            &self.inner,
            session_id,
            req,
            admitted,
        )
        .await
    }
    #[cfg(feature = "runtime-adapter")]
    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        <Service as MobSessionService>::runtime_adapter(&self.inner)
    }
    #[cfg(feature = "runtime-adapter")]
    fn supports_runtime_turn_apply(&self) -> bool {
        <Service as MobSessionService>::supports_runtime_turn_apply(&self.inner)
    }
    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::interrupt_with_machine_authority(
            &self.inner,
            session_id,
            authority,
        )
        .await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_run_with_machine_authority(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::interrupt_run_with_machine_authority(
            &self.inner,
            session_id,
            expected_run_id,
            authority,
        )
        .await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::cancel_after_boundary_with_machine_authority(
            &self.inner,
            session_id,
            expected_run_id,
            authority,
        )
        .await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn cancel_current_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::cancel_current_after_boundary_with_machine_authority(
            &self.inner,
            session_id,
            authority,
        )
        .await
    }
    async fn execution_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        <Service as MobSessionService>::execution_snapshot(&self.inner, session_id).await
    }
    async fn observe_member_status_view(
        &self,
        session_id: &SessionId,
    ) -> Result<MemberStatusSessionView, SessionError> {
        <Service as MobSessionService>::observe_member_status_view(&self.inner, session_id).await
    }
    async fn observe_live_durable_source(
        &self,
        session_id: &SessionId,
    ) -> Result<LiveDurableSourceObservation, SessionError> {
        <Service as MobSessionService>::observe_live_durable_source(&self.inner, session_id).await
    }
    async fn tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        <Service as MobSessionService>::tool_scope_snapshot(&self.inner, session_id).await
    }
    async fn external_tool_surface_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        <Service as MobSessionService>::external_tool_surface_snapshot(&self.inner, session_id)
            .await
    }
    async fn peer_ingress_runtime_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        <Service as MobSessionService>::peer_ingress_runtime_snapshot(&self.inner, session_id).await
    }
    async fn session_known_to_archive_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::session_known_to_archive_authority(&self.inner, session_id)
            .await
    }
    async fn session_belongs_to_mob(
        &self,
        session_id: &SessionId,
        mob_id: &crate::ids::MobId,
    ) -> bool {
        <Service as MobSessionService>::session_belongs_to_mob(&self.inner, session_id, mob_id)
            .await
    }
    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        <Service as MobSessionService>::load_persisted_session(&self.inner, session_id).await
    }
    async fn fork_persisted_session(
        &self,
        source_session_id: &SessionId,
        message_count: Option<usize>,
        tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        target: meerkat_core::DurableSessionForkTarget,
    ) -> Result<meerkat_core::SessionForkResult, SessionError> {
        <Service as MobSessionService>::fork_persisted_session(
            &self.inner,
            source_session_id,
            message_count,
            tool_access_policy,
            target,
        )
        .await
    }
    async fn fork_persisted_session_at_turn_boundary(
        &self,
        source_session_id: &SessionId,
        message_count: Option<usize>,
        tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        target: meerkat_core::DurableSessionForkTarget,
        bound: std::time::Duration,
    ) -> Result<meerkat_core::DurableForkAtTurnBoundary, SessionError> {
        <Service as MobSessionService>::fork_persisted_session_at_turn_boundary(
            &self.inner,
            source_session_id,
            message_count,
            tool_access_policy,
            target,
            bound,
        )
        .await
    }
    async fn load_revivable_retired_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        <Service as MobSessionService>::load_revivable_retired_session(&self.inner, session_id)
            .await
    }
    async fn load_session_for_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<ResumeSessionLoad, SessionError> {
        <Service as MobSessionService>::load_session_for_resume(&self.inner, session_id).await
    }
    async fn observe_session_resume_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionResumeAuthority, SessionError> {
        <Service as MobSessionService>::observe_session_resume_authority(&self.inner, session_id)
            .await
    }
    async fn revalidate_session_resume_authority(
        &self,
        session_id: &SessionId,
        expected: &SessionResumeAuthority,
    ) -> Result<Result<(), SessionResumeRejection>, SessionError> {
        <Service as MobSessionService>::revalidate_session_resume_authority(
            &self.inner,
            session_id,
            expected,
        )
        .await
    }
    async fn materialize_session_resume_verdict(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionResumeVerdict, SessionError> {
        <Service as MobSessionService>::materialize_session_resume_verdict(&self.inner, session_id)
            .await
    }
    async fn load_persisted_session_metadata(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
        <Service as MobSessionService>::load_persisted_session_metadata(&self.inner, session_id)
            .await
    }
    async fn session_projection_visible(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::session_projection_visible(&self.inner, session_id).await
    }
    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::archive_with_mob_lifecycle_authority(
            &self.inner,
            session_id,
        )
        .await
    }
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(&self.inner, session_id).await
    }
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
        &self,
        session_id: &SessionId,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(&self.inner, session_id, deadline).await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(
        &self,
        session_id: &SessionId,
        deadline: meerkat_core::time_compat::Instant,
        post_commit_hook: Option<Arc<dyn meerkat_runtime::MachineSessionArchivePostCommitHook>>,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(&self.inner, session_id, deadline, post_commit_hook).await
    }
    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        <Service as MobSessionService>::apply_runtime_turn(
            &self.inner,
            session_id,
            run_id,
            req,
            boundary,
            contributing_input_ids,
        )
        .await
    }
    async fn prepare_turn_boundary_delivery_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
        delivery: meerkat_core::TurnBoundaryDelivery,
    ) -> Result<meerkat_core::CoreBoundaryStageOutput, meerkat_core::CoreBoundaryStageError> {
        <Service as MobSessionService>::prepare_turn_boundary_delivery_for_active_turn(
            &self.inner,
            session_id,
            expected_run_id,
            delivery,
        )
        .await
    }
    async fn checkpoint_committed_runtime_session_snapshot(
        &self,
        session_id: &SessionId,
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::checkpoint_committed_runtime_session_snapshot(
            &self.inner,
            session_id,
            session_snapshot,
        )
        .await
    }
    async fn acquire_runtime_turn_finalization_guard(
        &self,
        session_id: &SessionId,
    ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
    {
        <Service as MobSessionService>::acquire_runtime_turn_finalization_guard(
            &self.inner,
            session_id,
        )
        .await
    }
    async fn checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(&self.inner, session_id, session_snapshot).await
    }
    async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
        authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(&self.inner, session_id, authority).await
    }
    async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
        &self,
        session_id: &SessionId,
        runtime_adapter: &meerkat_runtime::MeerkatMachine,
    ) -> Result<usize, SessionError> {
        <Service as MobSessionService>::enqueue_committed_parent_session_boundary_after_runtime_turn(&self.inner, session_id, runtime_adapter).await
    }
    async fn discard_live_session_after_runtime_stop_terminalized(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::discard_live_session_after_runtime_stop_terminalized(
            &self.inner,
            session_id,
        )
        .await
    }
    async fn discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(&self.inner, session_id).await
    }
    async fn publish_boundary_appends_discarded_for_actor(
        &self,
        actor_witness: &meerkat_session::LiveSessionActorWitness,
        discarded: &meerkat_core::event::BoundaryAppendsDiscarded,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::publish_boundary_appends_discarded_for_actor(
            &self.inner,
            actor_witness,
            discarded,
        )
        .await
    }
    async fn publish_interaction_terminals_for_actor(
        &self,
        actor_witness: &meerkat_session::LiveSessionActorWitness,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        SessionError,
    > {
        <Service as MobSessionService>::publish_interaction_terminals_for_actor(
            &self.inner,
            actor_witness,
            events,
        )
        .await
    }
    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        <Service as MobSessionService>::discard_live_session(&self.inner, session_id).await
    }
    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::discard_live_session_under_runtime_turn_boundary(
            &self.inner,
            session_id,
        )
        .await
    }
    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::discard_live_session_actor_under_runtime_turn_boundary(
            &self.inner,
            witness,
        )
        .await
    }
    async fn discard_live_session_actor_after_durability_reload_required(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::discard_live_session_actor_after_durability_reload_required(
            &self.inner,
            witness,
        )
        .await
    }
    async fn await_event_projection_drain(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::await_event_projection_drain(&self.inner, session_id).await
    }
    async fn cancel_all_checkpointers(&self) {
        <Service as MobSessionService>::cancel_all_checkpointers(&self.inner).await;
    }
    async fn rearm_all_checkpointers(&self) {
        <Service as MobSessionService>::rearm_all_checkpointers(&self.inner).await;
    }
}
