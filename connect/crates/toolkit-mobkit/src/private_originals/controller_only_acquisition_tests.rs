//! Actual registered acquisition call during the same native controller-only A
//! turn. The physical model response is scripted; authority and tool feedback
//! are native. This does not claim a successful setup/helper or ready bootstrap.
use super::*;
use crate::installed_host::acquisition::{InstalledAcquisition, InstalledAcquisitionTools};
use futures::StreamExt;
use meerkat_core::{AgentToolDispatcher, ToolCallView, ToolDispatchContext, ToolResult};
use std::sync::{Mutex, atomic::AtomicBool};
use toolkit_store::ToolkitStore;

#[path = "acquisition_custody_progress_tests.rs"]
mod custody_progress;

#[async_trait]
trait FindDispatchProbe: Send + Sync {
    async fn inspect(&self, call: ToolCallView<'_>, context: &ToolDispatchContext);
}

const CALL: &str = "controller-only-find-and-install";

pub(super) struct Script {
    tools: InstalledAcquisitionTools,
    dispatch: Arc<ObservedLeaf>,
    owner: Mutex<Option<Arc<InstalledAcquisition>>>,
    setup_timeout_ms: Option<u64>,
    results: Mutex<Vec<ToolResult>>,
    hold_first_response: bool,
    first_response_entered: tokio::sync::Notify,
    first_response_release: tokio::sync::Semaphore,
}
impl Script {
    pub(super) fn for_approved() -> Arc<Self> { Self::new() }
    pub(super) fn for_expiring_setup(timeout_ms: u64) -> Arc<Self> {
        let mut script = Self::new();
        Arc::get_mut(&mut script).unwrap().setup_timeout_ms = Some(timeout_ms);
        script
    }
    pub(super) fn setup_limits(&self) -> crate::installed_host::acquisition::InstalledSetupLimits {
        let mut limits = crate::installed_host::acquisition::InstalledSetupLimits::default();
        if let Some(timeout) = self.setup_timeout_ms { limits.setup_timeout_ms = timeout; }
        limits
    }
    pub(super) fn installed_owner(&self) -> Arc<InstalledAcquisition> {
        self.owner.lock().unwrap().as_ref().unwrap().clone()
    }
    pub(super) fn paused_for_helper_entry() -> Arc<Self> { Self::new_with_hold(true) }
    pub(super) fn paused_for_expiring_helper_entry(timeout_ms: u64) -> Arc<Self> {
        let mut script = Self::new_with_hold(true);
        Arc::get_mut(&mut script).unwrap().setup_timeout_ms = Some(timeout_ms);
        script
    }
    pub(super) async fn first_response_waiting(&self) {
        assert!(self.hold_first_response);
        self.first_response_entered.notified().await;
    }
    pub(super) fn release_first_response(&self) {
        assert!(self.hold_first_response);
        self.first_response_release.add_permits(1);
    }
    pub(super) fn observations(&self) -> (Vec<ToolResult>, usize, usize) {
        (self.results.lock().unwrap().clone(), self.dispatch.resolved.load(Ordering::SeqCst),
            self.dispatch.entered.load(Ordering::SeqCst))
    }
    fn new() -> Arc<Self> { Self::new_with_hold(false) }
    fn new_with_hold(hold_first_response: bool) -> Arc<Self> {
        let tools = InstalledAcquisitionTools::new(Duration::from_secs(10)).unwrap();
        let dispatch = Arc::new(ObservedLeaf {
            inner: tools.acquisition_tool(),
            resolved: AtomicUsize::new(0),
            entered: AtomicUsize::new(0),
            probe: Mutex::new(None),
        });
        Arc::new(Self {
            tools,
            dispatch,
            owner: Mutex::new(None),
            setup_timeout_ms: None,
            results: Mutex::new(vec![]),
            hold_first_response,
            first_response_entered: tokio::sync::Notify::new(),
            first_response_release: tokio::sync::Semaphore::new(0),
        })
    }
    pub(super) fn tools(&self) -> InstalledAcquisitionTools {
        self.tools.clone()
    }
    pub(super) fn dispatcher(&self) -> Arc<dyn AgentToolDispatcher> {
        self.dispatch.clone()
    }
    pub(super) fn retain_owner(&self, owner: Arc<InstalledAcquisition>) {
        assert!(Arc::ptr_eq(&owner.tool(), &self.tools.acquisition_tool()));
        assert!(self.owner.lock().unwrap().replace(owner).is_none());
    }
    // Durable fixture teardown calls this only after its actual close joins.
    // Drop our retained acquisition owner, never native or source-owned state.
    pub(super) fn release_owner_after_close(&self) {
        let owner = self.owner.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        drop(owner);
    }
    pub(super) fn provider(self: &Arc<Self>, inner: Arc<dyn LlmClient>) -> Arc<dyn LlmClient> {
        Arc::new(AcquisitionResponse {
            inner,
            script: self.clone(),
            first: AtomicBool::new(true),
        })
    }
}

/// Forward every native dispatcher seam unchanged. Counting is outside the real
/// immutable leaf, which remains wrapped by the native bundle registration.
struct ObservedLeaf {
    inner: Arc<dyn AgentToolDispatcher>,
    resolved: AtomicUsize,
    entered: AtomicUsize,
    probe: Mutex<Option<Arc<dyn FindDispatchProbe>>>,
}
#[async_trait]
impl AgentToolDispatcher for ObservedLeaf {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        self.inner.tools()
    }
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }
    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        self.inner.tool_catalog()
    }
    fn tool_mutation_class(&self, name: &str) -> meerkat_core::ToolMutationClass {
        self.inner.tool_mutation_class(name)
    }
    fn live_bridge_effect_kind(&self, name: &str) -> meerkat_core::LiveBridgeEffectKind {
        self.inner.live_bridge_effect_kind(name)
    }
    fn review_entry_support(
        &self,
        name: &str,
    ) -> meerkat_core::approval::review::ReviewEntrySupport {
        self.inner.review_entry_support(name)
    }
    fn execution_binding_epoch(&self, name: &str) -> u64 {
        self.inner.execution_binding_epoch(name)
    }
    fn execution_binding_fingerprint(
        &self,
        name: &str,
    ) -> Result<
        meerkat_core::EphemeralToolBindingFingerprint,
        meerkat_core::ToolExecutionResolutionError,
    > {
        self.inner.execution_binding_fingerprint(name)
    }
    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        resolution: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let plan = self
            .inner
            .resolve_execution_plan(call, context, resolution)?;
        self.resolved.fetch_add(1, Ordering::SeqCst);
        Ok(plan)
    }
    fn validate_resolved_execution_plan(
        &self,
        call: ToolCallView<'_>,
        resolution: &meerkat_core::ToolExecutionResolutionContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<(), meerkat_core::ToolExecutionResolutionError> {
        self.inner
            .validate_resolved_execution_plan(call, resolution, plan)
    }
    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.inner.dispatch(call).await
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let probe = self.probe.lock().unwrap().clone();
        if let Some(probe) = probe { probe.inspect(call, context).await; }
        self.inner.dispatch_with_context(call, context).await
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let probe = self.probe.lock().unwrap().clone();
        if let Some(probe) = probe { probe.inspect(call, context).await; }
        self.inner
            .dispatch_resolved_with_context(call, context, plan)
            .await
    }
    async fn poll_external_updates(&self) -> meerkat_core::agent::ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }
    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        self.inner.external_tool_surface_snapshot()
    }
    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        self.inner.capabilities()
    }
    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        session: SessionId,
    ) -> Result<meerkat_core::agent::BindOutcome, meerkat_core::agent::OpsLifecycleBindError> {
        // The actual acquisition leaf has no lifecycle state to bind. Forward
        // its refusal rather than inventing a successful fixture binding.
        assert!(!self.inner.capabilities().ops_lifecycle);
        self.inner.clone().bind_ops_lifecycle(registry, session)
    }
    fn completion_enrichment(
        &self,
    ) -> Option<Arc<dyn meerkat_core::completion_feed::CompletionEnrichmentProvider>> {
        self.inner.completion_enrichment()
    }
    fn bind_mcp_server_lifecycle_handle(
        &self,
        handle: Arc<dyn meerkat_core::handles::McpServerLifecycleHandle>,
    ) {
        self.inner.bind_mcp_server_lifecycle_handle(handle);
    }
    fn bind_external_tool_surface_handle(
        &self,
        handle: Arc<dyn meerkat_core::handles::ExternalToolSurfaceHandle>,
    ) {
        self.inner.bind_external_tool_surface_handle(handle);
    }
}

struct AcquisitionResponse {
    inner: Arc<dyn LlmClient>,
    script: Arc<Script>,
    first: AtomicBool,
}
#[async_trait]
impl LlmClient for AcquisitionResponse {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }
    fn plain_model_route(
        &self,
        model: &str,
    ) -> Result<PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        self.inner.plain_model_route(model)
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        self.inner.controller_model_selection()
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.inner.stream(request)
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let first = self.first.swap(false, Ordering::SeqCst);
        if !first {
            let mut captured = self.script.results.lock().unwrap();
            for message in request.request().messages.iter() {
                if let Message::ToolResults { results, .. } = message {
                    captured.extend(
                        results
                            .iter()
                            .filter(|result| result.tool_use_id == CALL)
                            .cloned(),
                    );
                }
            }
        }
        // The underlying provider still consumes native authorization, checks
        // currentness, and records physical model entry/outcome for every call.
        let stream = self.inner.stream_prepared(request);
        if !first {
            return stream;
        }
        let script = self.script.clone();
        Box::pin(futures::stream::once(async move {
            if script.hold_first_response {
                // Hold only physical scripted output. Native authorization and
                // input admission remain on their ordinary owner paths.
                script.first_response_entered.notify_one();
                script.first_response_release.acquire().await.unwrap().forget();
            }
            stream.map(|event| {
            event.map(|event| match event {
                LlmEvent::TextDelta { .. } => LlmEvent::ToolCallComplete {
                    id: CALL.into(),
                    name: crate::FIND_AND_INSTALL.into(),
                    args: serde_json::json!({"request":"Find a tool for this original request"}),
                    meta: None,
                },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success { .. },
                } => LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                },
                other => other,
            })
            })
        }).flatten())
    }
    fn provider(&self) -> Provider {
        self.inner.provider()
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controller_only_original_denies_enrolled_acquisition_in_its_real_turn() {
    let script = Script::new();
    let fixture = Box::pin(FirstFixture::new_selected_acquisition(
        true,
        false,
        false,
        None,
        Some(script.clone()),
    ))
    .await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT,
        Box::pin(async {
            let owner = script
                .owner
                .lock()
                .unwrap()
                .clone()
                .expect("the exact leaf has a retained installed owner");
            owner.require_native_runtime().unwrap();
            let scope = toolkit_store::StoreScope {
                realm: fixture.scope.realm.clone(),
                owner: format!("toolkit/acquisition/{}", fixture.scope.owner),
            };
            assert!(
                fixture
                    .store
                    .list_records(&scope, None, 10)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(matches!(fixture.binding().await, Ok(None)));
            assert_eq!(fixture.inputs().await, Ok(0));
            let first = fixture.submit().await;
            fixture.finish(&first).await.unwrap();
            let association = fixture.binding().await.unwrap().unwrap();
            assert!(crate::original_ingress::controller_only_basis(
                &association.candidate().authority_basis
            ));
            assert_eq!(
                association
                    .candidate()
                    .authority_basis
                    .grant_lineages()
                    .count(),
                0
            );
            assert_eq!(association.candidate().controller_grant_lineage.len(), 2);
            let results = script.results.lock().unwrap().clone();
            assert_eq!(
                results.len(),
                1,
                "actual native feedback is required: {results:?}"
            );
            assert!(results[0].is_error, "{results:?}");
            let payload: serde_json::Value =
                serde_json::from_str(&results[0].text_content()).unwrap();
            let expected = meerkat_core::ToolError::AuthorizationRefused {
                refusal: meerkat_core::OperationRefused::new(
                    meerkat_core::OperationRefusalKind::Denied,
                ),
            }
            .to_error_payload();
            assert_eq!(
                payload, expected,
                "must be the native operation denial, not surface/parser/owner unavailability"
            );
            assert!(
                script.dispatch.resolved.load(Ordering::SeqCst) > 0,
                "real leaf resolution must be reached"
            );
            assert_eq!(
                script.dispatch.entered.load(Ordering::SeqCst),
                0,
                "no acquisition leaf body entry"
            );
            assert_eq!(
                fixture.model_requests(),
                2,
                "denial must return to the same native controller turn"
            );
            assert_eq!(fixture.inputs().await, Ok(1));
            assert!(
                fixture
                    .store
                    .list_records(&scope, None, 10)
                    .await
                    .unwrap()
                    .is_empty(),
                "no canonical acquisition/helper work"
            );
            let sources = fixture
                .store
                .list_records(&fixture.scope, None, 10)
                .await
                .unwrap();
            assert_eq!(
                sources.len(),
                1,
                "only the original A exists, with no helper/B source"
            );
            let replay = fixture.submit().await;
            fixture.finish(&replay).await.unwrap();
            let (
                Ok(PrivateOriginalSubmission::Admitted {
                    input_id: first_id,
                    turn_ticket: first_ticket,
                    deduplicated: false,
                    source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                    ..
                }),
                Ok(PrivateOriginalSubmission::Admitted {
                    input_id: replay_id,
                    turn_ticket: replay_ticket,
                    deduplicated: true,
                    source_settlement: PrivateSourceSettlement::RetryWithdrawn,
                    ..
                }),
            ) = (&first, &replay)
            else {
                panic!(
                    "canonical input and replay observations required: {} / {}",
                    submission_context(&first),
                    submission_context(&replay)
                );
            };
            assert_eq!(first_id, replay_id);
            // Native tickets track deliveries; replay retains one canonical input.
            assert!(first_ticket.is_some());
            assert!(replay_ticket.is_some());
            assert_eq!(fixture.inputs().await, Ok(1));
            assert_eq!(
                fixture.model_requests(),
                2,
                "replay must not run the tool or model again"
            );
            assert_eq!(script.dispatch.entered.load(Ordering::SeqCst), 0);
            assert!(
                fixture
                    .store
                    .list_records(&scope, None, 10)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(matches!(fixture.binding().await, Ok(Some(saved)) if saved == association));
        }),
    ))
    .catch_unwind()
    .await;
    // The same native runtime is joined on both success and assertion failure.
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("controller-only acquisition turn control timed out"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
