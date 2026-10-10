//! Native FactoryAgent service tests: owned App IO must not borrow the running Agent.

use super::*;
use async_trait::async_trait;
use futures::{StreamExt, stream};
use meerkat_client::{LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_core::hooks::{HookBackgroundSkip, HookBackgroundSkipReason};
use meerkat_core::service::{InitialTurnPolicy, SessionService, SessionServiceHistoryExt};
use meerkat_core::tool_application::{ToolApplicationBinding, ToolApplicationResolution};
use meerkat_core::{
    AgentError, AgentToolDispatcher, AssistantBlock, HookEngine, HookEngineError,
    HookExecutionReport, HookId, HookInvocation, HookPoint, HookRunOverrides, InputId,
    OperationAuthorizationError, Provider, RunId, SessionEffect, SessionId,
    ToolApplicationControlRequest, ToolApplicationIngress, ToolApplicationOperation,
    ToolApplicationRequest, ToolCallView, ToolDef, ToolDispatchContext, ToolDispatchOutcome,
    ToolError, ToolResult,
};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input_state::{InputStatePersistenceRecord, StoredInputState};
use meerkat_runtime::store::{
    CommittedWholeBlobSnapshot, PreparedWholeBlobProvisionalTail, RuntimeStore,
    WholeBlobStoreAuthority,
};
use meerkat_runtime::{InMemoryRuntimeStore, MeerkatMachine};
use meerkat_session::PersistentSessionService;
use meerkat_store::{MemoryBlobStore, MemoryStore};
use serde_json::{Value, json};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Semaphore;

const DEADLINE: Duration = Duration::from_secs(10);
const EXTENSION: &str = "test.app";
const LAUNCH_ID: &str = "native-app-launch";

struct HeldModel {
    requests: AtomicUsize,
    entered: Semaphore,
    release: Semaphore,
}

impl HeldModel {
    fn new() -> Self {
        Self {
            requests: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }
}

#[async_trait]
impl LlmClient for HeldModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>>
    {
        let first = self.requests.fetch_add(1, Ordering::SeqCst) == 0;
        let usage = meerkat_core::TurnUsage::host_declared(
            Provider::Anthropic,
            &request.model,
            Default::default(),
        );
        if first {
            return Box::pin(stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: LAUNCH_ID.into(),
                    name: "show".into(),
                    args: json!({}),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate { usage }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::ToolUse,
                    },
                }),
            ]));
        }
        assert!(
            !serde_json::to_string(&request.messages)
                .expect("model messages")
                .contains("private-app")
        );
        Box::pin(
            stream::once(async move {
                self.entered.add_permits(1);
                self.release
                    .acquire()
                    .await
                    .expect("release model")
                    .forget();
                vec![
                    Ok(LlmEvent::TextDelta {
                        delta: "model complete".into(),
                        meta: None,
                    }),
                    Ok(LlmEvent::UsageUpdate { usage }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::EndTurn,
                        },
                    }),
                ]
            })
            .flat_map(stream::iter),
        )
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

struct AppTools {
    calls: AtomicUsize,
    entered: Semaphore,
    release: Semaphore,
    block: AtomicBool,
    append: AtomicBool,
}
impl AppTools {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            block: AtomicBool::new(false),
            append: AtomicBool::new(false),
        }
    }
}
#[async_trait]
impl AgentToolDispatcher for AppTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![
            Arc::new(ToolDef::new(
                "show",
                "show an app",
                json!({"type":"object"}),
            )),
            Arc::new(
                ToolDef::new("poll", "app action", json!({"type":"object"}))
                    .with_audience(meerkat_core::ToolAudience::App),
            ),
        ]
        .into()
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        assert_eq!(call.name, "show");
        let mut result = ToolResult::new(call.id.into(), "launch accepted".into(), false);
        result
            .host_metadata
            .insert(EXTENSION.into(), json!({"private":"private-app-launch"}));
        Ok(ToolDispatchOutcome::sync_result(result))
    }
    async fn resolve_tool_application(
        &self,
        source: &str,
        request: &ToolApplicationRequest,
        invocation: &Value,
        _: &ToolDispatchContext,
    ) -> Result<ToolApplicationResolution, ToolError> {
        assert_eq!(source, "show");
        assert_eq!(invocation["private"], "private-app-launch");
        assert!(matches!(
            request.operation,
            ToolApplicationOperation::CallTool { .. }
        ));
        Ok(ToolApplicationResolution::Call {
            name: "poll".into(),
            binding: ToolApplicationBinding::new(EXTENSION, json!({})),
            project_result: |result| Ok(json!({"content": result.text_content()})),
        })
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name == "show" {
            return self.dispatch(call).await;
        }
        assert_eq!(call.name, "poll");
        assert!(context.tool_application_control().is_some());
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        if self.block.load(Ordering::SeqCst) {
            self.release
                .acquire()
                .await
                .expect("release App IO")
                .forget();
        }
        let effects = if self.append.load(Ordering::SeqCst) {
            vec![SessionEffect::AppendAssistantBlocks {
                blocks: vec![AssistantBlock::Text {
                    text: "private-app-effect".into(),
                    meta: None,
                }],
            }]
        } else {
            Vec::new()
        };
        let mut result = ToolResult::new(call.id.into(), "private-app-result".into(), false);
        result.settlement_failures.push(settlement_marker());
        Ok(ToolDispatchOutcome::new(result, Vec::new(), effects))
    }
}

fn settlement_marker() -> meerkat_core::ops::ToolDispatchSettlementFailure {
    meerkat_core::ops::ToolDispatchSettlementFailure {
        admission_source: meerkat_core::ops::ToolDispatchAdmissionSource::ContextGate,
        effect_kind: meerkat_core::LiveBridgeEffectKind::ToolDispatch,
        physical_outcome: meerkat_core::LiveBridgeEffectOutcome::Committed,
        failure_kind: meerkat_core::ToolDispatchTerminalErrorKind::ExecutionFailed,
    }
}

struct AppHooks {
    calls: AtomicUsize,
    notice: AtomicBool,
    fail_post: AtomicBool,
    post_finished: Semaphore,
}
impl Default for AppHooks {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            notice: AtomicBool::new(false),
            fail_post: AtomicBool::new(false),
            post_finished: Semaphore::new(0),
        }
    }
}
#[async_trait]
impl HookEngine for AppHooks {
    async fn execute(
        &self,
        invocation: HookInvocation,
        overrides: Option<&HookRunOverrides>,
    ) -> Result<HookExecutionReport, HookEngineError> {
        if !matches!(
            invocation.point,
            HookPoint::PreToolExecution | HookPoint::PostToolExecution
        ) || invocation
            .tool_call
            .as_ref()
            .map(|call| call.name.as_str())
            .or_else(|| {
                invocation
                    .tool_result
                    .as_ref()
                    .map(|result| result.name.as_str())
            })
            != Some("poll")
        {
            return Ok(HookExecutionReport::empty());
        }
        assert!(
            overrides.is_none(),
            "App actions cannot reuse model hook overrides"
        );
        assert!(invocation.run_id.is_none());
        assert!(invocation.turn_number.is_none());
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut report = HookExecutionReport::empty();
        if invocation.point == HookPoint::PostToolExecution {
            let hook_id = HookId::new("native-app-notice");
            if self.notice.load(Ordering::SeqCst) {
                report.background_skips.push(HookBackgroundSkip {
                    hook_id: hook_id.clone(),
                    point: invocation.point,
                    reason: HookBackgroundSkipReason::ConcurrencyFull,
                });
            }
            self.post_finished.add_permits(1);
            if self.fail_post.load(Ordering::SeqCst) {
                return Err(HookEngineError::WithReport {
                    report: Box::new(report),
                    error: Box::new(HookEngineError::Timeout {
                        hook_id,
                        timeout_ms: 17,
                    }),
                });
            }
        }
        Ok(report)
    }
}

struct RevocableIngress(AtomicBool);
impl ToolApplicationIngress for RevocableIngress {
    fn revalidate(&self) -> Result<(), OperationAuthorizationError> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(OperationAuthorizationError::Unavailable)
        }
    }
    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

// Inject a real native Agent through the same public factory build configuration
// and session-owned runtime bindings as FactoryAgentBuilder. Only test clients,
// tools and hooks are overridden; no standalone authority is fabricated.
struct NativeBuilder {
    factory: AgentFactory,
    client: Arc<HeldModel>,
    tools: Arc<AppTools>,
    hooks: Arc<AppHooks>,
}
#[async_trait]
impl SessionAgentBuilder for NativeBuilder {
    type Agent = FactoryAgent;
    async fn build_agent(
        &self,
        request: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<FactoryAgent, SessionError> {
        let mut config = AgentBuildConfig::from_create_session_request(request, event_tx);
        config.llm_client_override = Some(self.client.clone());
        config.tool_dispatcher_override = Some(self.tools.clone());
        config.hook_engine_override = Some(self.hooks.clone());
        let session_context = match request
            .build
            .as_ref()
            .map(|build| &build.runtime_build_mode)
        {
            Some(meerkat_core::RuntimeBuildMode::SessionOwned(bindings)) => {
                Some(Arc::clone(bindings.session_context()))
            }
            _ => None,
        };
        let factory = self.factory.clone();
        let agent = meerkat_runtime::stack_relief::relieve_caller_stack(move || async move {
            factory.build_agent(config, &Config::default()).await
        })
        .await
        .map_err(|error| SessionError::Agent(AgentError::InternalError(error.to_string())))?;
        Ok(FactoryAgent {
            agent,
            session_context,
            pending_head_canonical_boundary: None,
            acknowledged_head_canonical_boundary: None,
        })
    }
}

type NativeService = PersistentSessionService<NativeBuilder>;
struct Fixture {
    _temp: TempDir,
    service: Arc<NativeService>,
    machine: Arc<MeerkatMachine>,
    id: SessionId,
    client: Arc<HeldModel>,
    tools: Arc<AppTools>,
    hooks: Arc<AppHooks>,
    store: Arc<MemoryStore>,
    runtime_store: Arc<CountingRuntimeStore>,
    turn: tokio::task::JoinHandle<
        Result<meerkat_core::RunResult, crate::surface::SurfaceRuntimeMaterializeError>,
    >,
}
impl Fixture {
    async fn start() -> Self {
        let temp = TempDir::new().expect("temp");
        let store = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(CountingRuntimeStore::new());
        let blob_store = Arc::new(MemoryBlobStore::new());
        let machine = Arc::new(
            MeerkatMachine::persistent(runtime_store.clone(), blob_store.clone())
                .expect("canonical machine"),
        );
        let client = Arc::new(HeldModel::new());
        let tools = Arc::new(AppTools::new());
        let hooks = Arc::new(AppHooks::default());
        let builder = NativeBuilder {
            factory: AgentFactory::new(temp.path().join("sessions")).session_store(store.clone()),
            client: client.clone(),
            tools: tools.clone(),
            hooks: hooks.clone(),
        };
        let service = Arc::new(
            PersistentSessionService::new(
                builder,
                1,
                store.clone(),
                runtime_store.clone(),
                blob_store,
            )
            .with_canonical_runtime_adapter(machine.clone()),
        );
        let request = CreateSessionRequest {
            model: "claude-sonnet-4-5".into(),
            prompt: "show an app".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        };
        let (request, initial_turn) =
            crate::surface::split_runtime_backed_eager_create_request(request);
        let created = Box::pin(crate::surface::materialize_session(
            &service,
            &machine,
            meerkat_core::Session::new(),
            request,
            {
                let service = service.clone();
                let machine = machine.clone();
                move |id| crate::surface::default_persistent_executor(service, machine, id)
            },
        ))
        .await
        .expect("materialize native session and executor");
        let id = created.session_id;
        let mut turn = tokio::spawn({
            let service = service.clone();
            let machine = machine.clone();
            let id = id.clone();
            async move {
                crate::surface::run_runtime_backed_initial_turn_with_machine(
                    &service,
                    &machine,
                    &id,
                    initial_turn.expect("initial turn"),
                )
                .await
            }
        });
        tokio::time::timeout(DEADLINE, async {
            tokio::select! {
                permit = client.entered.acquire() => {
                    permit.expect("model gate").forget();
                    Ok(())
                }
                terminal = &mut turn => Err(format!(
                    "initial native turn ended before final model: requests={}, result={terminal:?}",
                    client.requests.load(Ordering::SeqCst),
                )),
            }
        })
            .await
            .expect("final model entered")
            .expect("initial native turn must reach final model");
        assert!(!turn.is_finished(), "model must remain held");
        let observations = service
            .read_tool_application_observations(&id)
            .await
            .expect("live App observation");
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].tool_call_id, LAUNCH_ID);
        Self {
            _temp: temp,
            service,
            machine,
            id,
            client,
            tools,
            hooks,
            store,
            runtime_store,
            turn,
        }
    }
    fn control(&self, ingress: Arc<RevocableIngress>) -> Arc<ToolApplicationControlRequest> {
        ToolApplicationControlRequest::from_trusted_ingress(
            self.id.clone(),
            ToolApplicationRequest {
                tool_call_id: LAUNCH_ID.into(),
                extension: EXTENSION.into(),
                operation: ToolApplicationOperation::CallTool {
                    name: "poll".into(),
                    arguments: json!({}),
                },
            },
            ingress,
        )
        .expect("trusted ingress")
    }
    fn call(&self) -> tokio::task::JoinHandle<Result<Value, SessionError>> {
        tokio::spawn(
            self.service
                .clone()
                .tool_application(self.control(Arc::new(RevocableIngress(AtomicBool::new(true))))),
        )
    }
    async fn release_model(&self) {
        self.client.release.add_permits(1);
        tokio::time::timeout(DEADLINE, async {
            while !self.turn.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("model turn settled");
    }
    async fn retained(&self) -> String {
        serde_json::to_string(
            &self
                .service
                .read_history(&self.id, Default::default())
                .await
                .expect("retained history"),
        )
        .expect("history JSON")
    }
}

#[tokio::test]
async fn native_tool_application_polls_complete_during_model_without_body_reads_or_writes() {
    let fixture = Fixture::start().await;
    let session_before = fixture.store.stats().await;
    let runtime_before = fixture.runtime_store.cost();
    for _ in 0..48 {
        let result = tokio::time::timeout(DEADLINE, fixture.call())
            .await
            .expect("poll must not wait for model")
            .expect("poll task")
            .expect("poll");
        assert_eq!(result["content"], "private-app-result");
    }
    assert!(!fixture.turn.is_finished());
    assert_eq!(fixture.tools.calls.load(Ordering::SeqCst), 48);
    assert_eq!(fixture.hooks.calls.load(Ordering::SeqCst), 96);
    assert_eq!(fixture.client.requests.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture.store.stats().await,
        session_before,
        "polls cannot read/save the accumulated session"
    );
    assert_eq!(
        fixture.runtime_store.cost(),
        runtime_before,
        "polls cannot read/write durable bodies"
    );
    fixture.release_model().await;
    assert!(!fixture.retained().await.contains("private-app-result"));
    fixture.service.try_shutdown().await.expect("shutdown");
    fixture.turn.await.expect("turn task").expect("turn");
}

#[tokio::test]
async fn native_tool_application_revocation_after_wait_withholds_append_but_persists_notice() {
    let fixture = Fixture::start().await;
    fixture.tools.append.store(true, Ordering::SeqCst);
    fixture.hooks.notice.store(true, Ordering::SeqCst);
    let ingress = Arc::new(RevocableIngress(AtomicBool::new(true)));
    let call = tokio::spawn(
        fixture
            .service
            .clone()
            .tool_application(fixture.control(ingress.clone())),
    );
    tokio::time::timeout(DEADLINE, fixture.tools.entered.acquire())
        .await
        .expect("action entered")
        .expect("gate")
        .forget();
    // This is a current-thread test. The action publishes this synchronous
    // hook completion and keeps running until the held model's actor/mutation
    // barrier suspends it. Revocation therefore occurs after native IO and
    // post-hook validation, exercising the separate settlement-time check.
    tokio::time::timeout(DEADLINE, fixture.hooks.post_finished.acquire())
        .await
        .expect("post hooks completed")
        .expect("post hook gate")
        .forget();
    assert!(!call.is_finished(), "actual mutation must wait for actor");
    ingress.0.store(false, Ordering::SeqCst);
    fixture.release_model().await;
    assert!(
        tokio::time::timeout(DEADLINE, call)
            .await
            .expect("settled")
            .expect("call task")
            .is_err()
    );
    let history = fixture.retained().await;
    assert!(
        history.contains("native-app-notice"),
        "entered hook notice must persist"
    );
    assert!(
        !history.contains("private-app-effect"),
        "revoked request cannot append private result"
    );
    assert_eq!(fixture.tools.calls.load(Ordering::SeqCst), 1);
    fixture.service.try_shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn native_tool_application_post_error_and_delivery_cancel_keep_dirty_notice() {
    for cancel_delivery in [false, true] {
        let fixture = Fixture::start().await;
        fixture.hooks.notice.store(true, Ordering::SeqCst);
        fixture.hooks.fail_post.store(true, Ordering::SeqCst);
        let call = fixture.call();
        tokio::time::timeout(DEADLINE, fixture.tools.entered.acquire())
            .await
            .expect("action entered")
            .expect("gate")
            .forget();
        if cancel_delivery {
            call.abort();
        }
        fixture.release_model().await;
        if cancel_delivery {
            assert!(call.await.expect_err("delivery aborted").is_cancelled());
        } else {
            let error = tokio::time::timeout(DEADLINE, call)
                .await
                .expect("settled")
                .expect("call task")
                .expect_err("post hook timeout");
            assert!(
                matches!(error.primary_error(), SessionError::Agent(error) if matches!(error.primary_error(), AgentError::HookTimeout { timeout_ms:17, .. }))
            );
            assert_eq!(
                error.settlement_failures().cloned().collect::<Vec<_>>(),
                vec![settlement_marker()]
            );
        }
        fixture
            .service
            .try_shutdown()
            .await
            .expect("shutdown drains notice persistence");
        let history = fixture.retained().await;
        assert!(history.contains("native-app-notice"));
        assert!(!history.contains("private-app-result"));
        assert_eq!(fixture.tools.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn native_tool_application_entered_work_refuses_discard_and_drains_on_shutdown() {
    let fixture = Fixture::start().await;
    fixture.tools.block.store(true, Ordering::SeqCst);
    fixture.hooks.notice.store(true, Ordering::SeqCst);
    let call = fixture.call();
    tokio::time::timeout(DEADLINE, fixture.tools.entered.acquire())
        .await
        .expect("action entered")
        .expect("gate")
        .forget();
    let error = tokio::time::timeout(DEADLINE, fixture.service.discard_live_session(&fixture.id))
        .await
        .expect("discard must be nonblocking")
        .expect_err("entered custody is busy");
    assert!(matches!(error.primary_error(), SessionError::Busy { .. }));
    let error = tokio::time::timeout(
        DEADLINE,
        fixture.service.archive_with_machine_protocol(
            &fixture.id,
            meerkat_session::MachineSessionArchiveProtocol::from_machine(fixture.machine.as_ref()),
        ),
    )
    .await
    .expect("archive must refuse before waiting for model")
    .expect_err("entered custody is busy");
    assert!(matches!(error.primary_error(), SessionError::Busy { .. }));
    let shutdown = tokio::spawn({
        let service = fixture.service.clone();
        async move { service.try_shutdown().await }
    });
    tokio::time::timeout(DEADLINE, async {
        while !fixture.turn.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("shutdown sealed admission and interrupted the held model");
    assert!(!shutdown.is_finished(), "entered App IO still owns custody");
    fixture.tools.release.add_permits(1);
    tokio::time::timeout(DEADLINE, shutdown)
        .await
        .expect("shutdown interrupts model and drains entered action")
        .expect("shutdown task")
        .expect("shutdown");
    assert!(
        call.await.expect("action task").is_err(),
        "closed admission withholds delivery"
    );
    assert!(fixture.retained().await.contains("native-app-notice"));
    assert_eq!(fixture.tools.calls.load(Ordering::SeqCst), 1);
    assert!(fixture.turn.is_finished());
}

// Transparent WholeBlob runtime-store decorator. The shared native authority
// implementation is retained, and all body/persistence seams count attempts.
struct CountingRuntimeStore {
    inner: InMemoryRuntimeStore,
    body_reads: AtomicUsize,
    writes: AtomicUsize,
    fail_snapshot_commits: AtomicBool,
    failed_snapshot_commits: AtomicUsize,
}
impl CountingRuntimeStore {
    fn new() -> Self {
        Self {
            inner: InMemoryRuntimeStore::new(),
            body_reads: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
            fail_snapshot_commits: AtomicBool::new(false),
            failed_snapshot_commits: AtomicUsize::new(0),
        }
    }
    fn cost(&self) -> (usize, usize) {
        (
            self.body_reads.load(Ordering::SeqCst),
            self.writes.load(Ordering::SeqCst),
        )
    }
}
#[async_trait]
impl RuntimeStore for CountingRuntimeStore {
    fn session_authority_ops(&self) -> &dyn meerkat_runtime::store::RuntimeSessionAuthorityOps {
        self.inner.session_authority_ops()
    }

    fn session_persistence_profile(
        &self,
    ) -> meerkat_runtime::store::RuntimeSessionPersistenceProfile {
        RuntimeStore::session_persistence_profile(&self.inner)
    }

    fn session_boundary_authority_read_cost(
        &self,
    ) -> meerkat_runtime::store::RuntimeSessionAuthorityReadCost {
        self.inner.session_boundary_authority_read_cost()
    }

    fn supports_compaction_projection_outbox(&self) -> bool {
        self.inner.supports_compaction_projection_outbox()
    }

    fn input_state_batch_cas_implementation_profile(
        &self,
    ) -> meerkat_runtime::store::InputStateBatchCasImplementationProfile {
        self.inner.input_state_batch_cas_implementation_profile()
    }

    async fn load_runtime_delivery_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        Option<meerkat_runtime::store::RuntimeDeliveryAuthorityRecord>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.load_runtime_delivery_authority(runtime_id).await
    }

    async fn load_runtime_delivery_record(
        &self,
        runtime_id: &LogicalRuntimeId,
        delivery_id: &str,
    ) -> Result<
        Option<meerkat_runtime::store::RuntimeDeliveryStoreRecord>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_runtime_delivery_record(runtime_id, delivery_id)
            .await
    }

    async fn compare_and_swap_runtime_delivery_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: Option<u64>,
        replacement: meerkat_runtime::store::RuntimeDeliveryAuthorityRecord,
        inserted_delivery: Option<meerkat_runtime::store::RuntimeDeliveryStoreRecord>,
    ) -> Result<
        meerkat_runtime::store::RuntimeDeliveryAuthorityCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_runtime_delivery_authority(
                runtime_id,
                expected_revision,
                replacement,
                inserted_delivery,
            )
            .await
    }

    async fn list_runtime_delivery_records(
        &self,
        runtime_id: &LogicalRuntimeId,
        after_sequence: u64,
        limit: usize,
    ) -> Result<
        Vec<meerkat_runtime::store::RuntimeDeliveryStoreRecord>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .list_runtime_delivery_records(runtime_id, after_sequence, limit)
            .await
    }

    fn persist_auth_oauth_flow_snapshot(
        &self,
        snapshot_json: &[u8],
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.persist_auth_oauth_flow_snapshot(snapshot_json)
    }

    fn load_auth_oauth_flow_snapshot(
        &self,
    ) -> Result<Option<Vec<u8>>, meerkat_runtime::store::RuntimeStoreError> {
        self.inner.load_auth_oauth_flow_snapshot()
    }

    fn update_auth_oauth_flow_snapshot(
        &self,
        update: &mut meerkat_runtime::store::AuthOAuthFlowSnapshotUpdate<'_>,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.update_auth_oauth_flow_snapshot(update)
    }

    async fn load_session_boundary_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        Option<meerkat_runtime::store::RuntimeSessionAuthority>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.load_session_boundary_authority(runtime_id).await
    }

    async fn load_whole_blob_store_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<WholeBlobStoreAuthority>, meerkat_runtime::store::RuntimeStoreError> {
        self.inner.load_whole_blob_store_authority(runtime_id).await
    }

    async fn load_committed_whole_blob_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<CommittedWholeBlobSnapshot>, meerkat_runtime::store::RuntimeStoreError> {
        self.body_reads.fetch_add(1, Ordering::SeqCst);
        self.inner
            .load_committed_whole_blob_snapshot(runtime_id)
            .await
    }

    async fn commit_prepared_whole_blob_snapshot_cas(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: meerkat_runtime::store::PreparedWholeBlobSnapshotCas,
    ) -> Result<
        meerkat_runtime::store::WholeBlobSnapshotCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.writes.fetch_add(1, Ordering::SeqCst);
        if self.fail_snapshot_commits.load(Ordering::SeqCst) {
            self.failed_snapshot_commits.fetch_add(1, Ordering::SeqCst);
            return Err(meerkat_runtime::store::RuntimeStoreError::WriteFailed(
                "App settlement store failure".into(),
            ));
        }

        self.inner
            .commit_prepared_whole_blob_snapshot_cas(runtime_id, prepared)
            .await
    }

    async fn delete_runtime_session_catalog_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .delete_runtime_session_catalog_entry(runtime_id)
            .await
    }

    async fn load_runtime_session_catalog_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        Option<meerkat_runtime::store::RuntimeSessionCatalogEntry>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_runtime_session_catalog_entry(runtime_id)
            .await
    }

    async fn list_runtime_session_catalog_entries(
        &self,
        filter: meerkat_core::SessionFilter,
    ) -> Result<
        Vec<meerkat_runtime::store::RuntimeSessionCatalogEntry>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .list_runtime_session_catalog_entries(filter)
            .await
    }

    async fn write_prepared_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobProvisionalTail,
    ) -> Result<
        meerkat_runtime::store::WholeBlobProvisionalTailAuthority,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner
            .write_prepared_whole_blob_provisional_tail(runtime_id, prepared)
            .await
    }

    async fn load_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        Option<meerkat_runtime::store::CommittedWholeBlobProvisionalTail>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.body_reads.fetch_add(1, Ordering::SeqCst);
        self.inner
            .load_whole_blob_provisional_tail(runtime_id)
            .await
    }

    async fn discard_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &meerkat_runtime::store::WholeBlobProvisionalTailAuthority,
    ) -> Result<bool, meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .discard_whole_blob_provisional_tail(runtime_id, expected)
            .await
    }

    async fn observe_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        meerkat_runtime::store::MachineLifecycleObservation,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.observe_machine_lifecycle(runtime_id).await
    }

    async fn compare_and_swap_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: meerkat_runtime::store::MachineLifecycleExpectedVersion,
        replacement: meerkat_runtime::store::MachineLifecycleCommit,
    ) -> Result<
        meerkat_runtime::store::MachineLifecycleCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_machine_lifecycle(runtime_id, expected, replacement)
            .await
    }

    async fn compare_and_swap_machine_lifecycle_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: meerkat_runtime::store::MachineLifecycleExpectedVersion,
        replacement: meerkat_runtime::store::MachineLifecycleCommit,
        write_fence: Arc<dyn meerkat_runtime::store::RuntimeStoreWriteFence>,
    ) -> Result<
        meerkat_runtime::store::FencedMachineLifecycleCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_machine_lifecycle_with_fence(
                runtime_id,
                expected,
                replacement,
                write_fence,
            )
            .await
    }

    async fn commit_prepared_session_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        request: meerkat_runtime::PreparedRuntimeSessionCommit,
    ) -> Result<
        meerkat_runtime::store::PreparedRuntimeSessionCommitResult,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.writes.fetch_add(1, Ordering::SeqCst);
        if self.fail_snapshot_commits.load(Ordering::SeqCst) {
            self.failed_snapshot_commits.fetch_add(1, Ordering::SeqCst);
            return Err(meerkat_runtime::store::RuntimeStoreError::WriteFailed(
                "App settlement store failure".into(),
            ));
        }
        self.inner
            .commit_prepared_session_boundary(runtime_id, request)
            .await
    }

    async fn commit_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: meerkat_runtime::store::SerializedSessionSnapshot,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner
            .commit_session_snapshot(runtime_id, session_delta)
            .await
    }

    async fn commit_prepared_whole_blob_rewrite_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        boundary: meerkat_runtime::store::PreparedWholeBlobRewriteStoreParts,
    ) -> Result<
        meerkat_runtime::store::WholeBlobStoreAuthority,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner
            .commit_prepared_whole_blob_rewrite_boundary(runtime_id, boundary)
            .await
    }

    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<meerkat_runtime::store::SerializedSessionSnapshot>,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: Option<SessionId>,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner
            .atomic_apply(
                runtime_id,
                session_delta,
                receipt,
                input_updates,
                session_store_key,
            )
            .await
    }

    async fn atomic_apply_with_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: meerkat_runtime::store::SerializedSessionSnapshot,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        machine_lifecycle: meerkat_runtime::store::MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: SessionId,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner
            .atomic_apply_with_machine_lifecycle(
                runtime_id,
                session_delta,
                receipt,
                machine_lifecycle,
                input_updates,
                session_store_key,
            )
            .await
    }

    async fn load_committed_boundary_receipts(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
    ) -> Result<
        Vec<meerkat_core::lifecycle::RunBoundaryReceipt>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_committed_boundary_receipts(runtime_id, run_id)
            .await
    }

    async fn load_durable_tail_recovery_receipts(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
    ) -> Result<
        Vec<meerkat_runtime::store::PreparedRecoveryReceiptSource>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_durable_tail_recovery_receipts(runtime_id, run_id)
            .await
    }

    async fn load_committed_recovery_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate_id: &str,
    ) -> Result<
        Option<meerkat_runtime::store::CommittedRecoveryBoundary>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_committed_recovery_boundary(runtime_id, candidate_id)
            .await
    }

    async fn load_input_states(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<meerkat_runtime::InputStateRow>, meerkat_runtime::store::RuntimeStoreError>
    {
        self.inner.load_input_states(runtime_id).await
    }

    async fn load_input_states_with_versions(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        meerkat_runtime::store::PreparedRecoveryInputSnapshot,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.load_input_states_with_versions(runtime_id).await
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<
        Option<meerkat_core::lifecycle::RunBoundaryReceipt>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_boundary_receipt(runtime_id, run_id, sequence)
            .await
    }

    async fn load_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Arc<Vec<u8>>>, meerkat_runtime::store::RuntimeStoreError> {
        self.body_reads.fetch_add(1, Ordering::SeqCst);
        self.inner.load_session_snapshot(runtime_id).await
    }

    async fn load_pending_compaction_projections(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        Vec<meerkat_core::CompactionProjectionIntent>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_pending_compaction_projections(runtime_id)
            .await
    }

    async fn mark_compaction_projection_finalized(
        &self,
        runtime_id: &LogicalRuntimeId,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .mark_compaction_projection_finalized(runtime_id, projection)
            .await
    }

    async fn clear_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.clear_session_snapshot(runtime_id).await
    }

    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, meerkat_runtime::store::RuntimeStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner
            .replace_session_snapshot_if_current(runtime_id, expected_current, replacement)
            .await
    }

    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, meerkat_runtime::store::RuntimeStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner
            .clear_session_snapshot_if_current(runtime_id, expected_current)
            .await
    }

    async fn is_runtime_projection_quarantined(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<bool, meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .is_runtime_projection_quarantined(runtime_id)
            .await
    }

    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &InputStatePersistenceRecord,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.persist_input_state(runtime_id, state).await
    }

    async fn persist_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        states: &[InputStatePersistenceRecord],
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .persist_input_states_atomically(runtime_id, states)
            .await
    }

    async fn compare_and_swap_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
    ) -> Result<
        meerkat_runtime::store::InputStateBatchCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_input_states_atomically(runtime_id, expected, replacements)
            .await
    }

    async fn compare_and_swap_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
        write_fence: Arc<dyn meerkat_runtime::store::RuntimeStoreWriteFence>,
    ) -> Result<
        meerkat_runtime::store::FencedInputStateBatchCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_input_states_atomically_with_fence(
                runtime_id,
                expected,
                replacements,
                write_fence,
            )
            .await
    }

    async fn compare_and_swap_recovery_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: meerkat_runtime::store::RecoveryInputSetRevision,
        mutations: &[meerkat_runtime::store::RecoveryInputStateMutation],
    ) -> Result<
        meerkat_runtime::store::InputStateBatchCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_recovery_input_states_atomically(
                runtime_id,
                expected_revision,
                mutations,
            )
            .await
    }

    async fn compare_and_swap_recovery_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: meerkat_runtime::store::RecoveryInputSetRevision,
        mutations: &[meerkat_runtime::store::RecoveryInputStateMutation],
        write_fence: Arc<dyn meerkat_runtime::store::RuntimeStoreWriteFence>,
    ) -> Result<
        meerkat_runtime::store::FencedInputStateBatchCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_recovery_input_states_atomically_with_fence(
                runtime_id,
                expected_revision,
                mutations,
                write_fence,
            )
            .await
    }

    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, meerkat_runtime::store::RuntimeStoreError> {
        self.inner.load_input_state(runtime_id, input_id).await
    }

    async fn load_input_state_by_idempotency_key(
        &self,
        runtime_id: &LogicalRuntimeId,
        key: &meerkat_runtime::IdempotencyKey,
    ) -> Result<
        Option<meerkat_runtime::store::ExactInputStateObservation>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_input_state_by_idempotency_key(runtime_id, key)
            .await
    }

    async fn load_input_states_by_ids(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_ids: &[InputId],
    ) -> Result<Vec<Option<StoredInputState>>, meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .load_input_states_by_ids(runtime_id, input_ids)
            .await
    }

    async fn load_pending_terminal_owner_ids_page(
        &self,
        runtime_id: &LogicalRuntimeId,
        after: Option<&InputId>,
        limit: usize,
    ) -> Result<Vec<InputId>, meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .load_pending_terminal_owner_ids_page(runtime_id, after, limit)
            .await
    }

    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, meerkat_runtime::store::RuntimeStoreError> {
        self.inner.load_machine_lifecycle_record(runtime_id).await
    }

    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: meerkat_runtime::store::MachineLifecycleCommit,
        input_states: &[InputStatePersistenceRecord],
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .commit_machine_lifecycle(runtime_id, commit, input_states)
            .await
    }

    async fn commit_unregister_finalization(
        &self,
        runtime_id: &LogicalRuntimeId,
        finalization: meerkat_runtime::store::UnregisterFinalizationCommit,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .commit_unregister_finalization(runtime_id, finalization)
            .await
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        snapshot: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.persist_ops_lifecycle(runtime_id, snapshot).await
    }

    async fn initialize_ops_lifecycle_if_absent(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<
        meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .initialize_ops_lifecycle_if_absent(runtime_id, candidate)
            .await
    }

    async fn load_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        Option<meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.load_ops_lifecycle(runtime_id).await
    }

    async fn delete_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.delete_ops_lifecycle(runtime_id).await
    }
}

#[tokio::test]
async fn native_tool_application_effect_waits_for_actor_then_persists_once() {
    let fixture = Fixture::start().await;
    fixture.tools.append.store(true, Ordering::SeqCst);
    let call = fixture.call();
    tokio::time::timeout(DEADLINE, fixture.tools.entered.acquire())
        .await
        .expect("action entered")
        .expect("gate")
        .forget();
    assert!(
        !call.is_finished(),
        "returned effect must wait for exact actor"
    );
    fixture.release_model().await;
    let result = tokio::time::timeout(DEADLINE, call)
        .await
        .expect("settled")
        .expect("task")
        .expect("action");
    assert_eq!(result["content"], "private-app-result");
    fixture.service.try_shutdown().await.expect("shutdown");
    assert_eq!(
        fixture
            .retained()
            .await
            .matches("private-app-effect")
            .count(),
        1
    );
    assert_eq!(fixture.tools.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn native_tool_application_failed_save_retires_exact_actor_with_another_entered_lease() {
    let fixture = Fixture::start().await;
    fixture.release_model().await;
    fixture.tools.append.store(true, Ordering::SeqCst);
    fixture.tools.block.store(true, Ordering::SeqCst);
    fixture.hooks.notice.store(true, Ordering::SeqCst);
    let first = fixture.call();
    tokio::time::timeout(DEADLINE, fixture.tools.entered.acquire())
        .await
        .expect("first entered")
        .expect("gate")
        .forget();
    let second = fixture.call();
    tokio::time::timeout(DEADLINE, fixture.tools.entered.acquire())
        .await
        .expect("second entered")
        .expect("gate")
        .forget();
    fixture
        .runtime_store
        .fail_snapshot_commits
        .store(true, Ordering::SeqCst);
    // Tokio semaphore waiters are FIFO: only the first entered action may
    // reach settlement before we inspect the exact owner's fatal retirement.
    fixture.tools.release.add_permits(1);
    let first_error = tokio::time::timeout(DEADLINE, first)
        .await
        .expect("failed persistence returns without waiting for second lease")
        .expect("task")
        .expect_err("injected save failure");
    assert!(
        matches!(first_error.primary_error(), SessionError::Agent(error)
            if matches!(error.primary_error(), AgentError::InternalError(message)
                if message.contains("runtime session control snapshot persistence failed")
                    && message.contains("App settlement store failure"))),
        "native persistence must preserve its existing typed failure: {first_error:?}"
    );
    assert_eq!(
        fixture
            .runtime_store
            .failed_snapshot_commits
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        first_error
            .settlement_failures()
            .cloned()
            .collect::<Vec<_>>(),
        vec![settlement_marker()]
    );
    assert!(
        !second.is_finished(),
        "second physical IO remains owned until release"
    );
    fixture.tools.release.add_permits(1);
    let second_error = tokio::time::timeout(DEADLINE, second)
        .await
        .expect("retired actor refuses later settlement")
        .expect("task")
        .expect_err("exact owner is retired");
    assert_eq!(
        second_error
            .settlement_failures()
            .cloned()
            .collect::<Vec<_>>(),
        vec![settlement_marker()]
    );
    fixture
        .runtime_store
        .fail_snapshot_commits
        .store(false, Ordering::SeqCst);
    assert!(
        fixture.call().await.expect("later call task").is_err(),
        "retired actor cannot serve App IO"
    );
    assert_eq!(fixture.tools.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture
            .runtime_store
            .failed_snapshot_commits
            .load(Ordering::SeqCst),
        1
    );
    assert!(
        !fixture.retained().await.contains("private-app-effect"),
        "failed write cannot become durable authority"
    );
    fixture.service.try_shutdown().await.expect("shutdown");
}
