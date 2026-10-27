//! Shared actual helper-model report fixture. The narrow source scope refuses
//! locally; the configured producer child below supplies correlated A targets.
//! Neither fixture is a Console approval or account-provider interaction.
use super::*;
use crate::installed_host::acquisition::InstalledSetupSelection;
use crate::installed_host::acquisition::InstalledServiceTaskSource;
use crate::private_originals::InstalledServiceTask;
use futures::StreamExt;
use meerkat_core::{AgentToolDispatcher, ToolCallView, ToolDispatchContext, ToolResult};
use meerkat_toolkit::setup::SETUP_REPORT;
use std::sync::{Mutex, OnceLock, atomic::AtomicBool};
use toolkit_store::ToolkitStore;

const REPORT_CALL: &str = "actual-helper-unavailable-report";
const DETAIL: &str = "The report fixture has no account or installed service available";
type Fixture = FirstFixture<meerkat::PersistentSessionService<FactoryAgentBuilder>>;

#[derive(Clone, Copy)]
enum ReportPreparation {
    Software,
    CapturedConsumer,
    ExplicitConsumer,
    PublicService,
}

struct ReportPreparationRequest {
    source: InstalledServiceTaskSource,
    kind: ReportPreparation,
    reply: tokio::sync::oneshot::Sender<Result<Option<InstalledServiceTask>, PrivateOriginalError>>,
}

pub(super) struct Report {
    leaf: Arc<ReportLeaf>,
    selection: OnceLock<InstalledSetupSelection>,
    first: AtomicBool,
    feedback: Mutex<Vec<ToolResult>>,
}
impl Report {
    fn new(inner: Arc<dyn AgentToolDispatcher>) -> Arc<Self> {
        Arc::new(Self {
            leaf: Arc::new(ReportLeaf { inner, context: Mutex::new(None),
                preparation: Mutex::new(None),
                arrived: tokio::sync::Notify::new(), released: AtomicBool::new(false),
                resume: tokio::sync::Notify::new() }),
            selection: OnceLock::new(), first: AtomicBool::new(true), feedback: Mutex::new(vec![]),
        })
    }
    pub(super) fn retain_selection(&self, selection: InstalledSetupSelection) {
        assert!(self.selection.set(selection).is_ok());
    }
    pub(super) fn dispatcher(&self) -> Arc<dyn AgentToolDispatcher> { self.leaf.clone() }
    pub(super) fn provider(self: &Arc<Self>, inner: Arc<dyn LlmClient>) -> Arc<dyn LlmClient> {
        Arc::new(ReportResponse { inner, report: self.clone() })
    }
    async fn take_context(&self) -> ToolDispatchContext {
        loop {
            let arrived = self.leaf.arrived.notified();
            if let Some(context) = self.leaf.context.lock().unwrap().take() { return context; }
            arrived.await;
        }
    }
    fn release(&self) {
        self.leaf.released.store(true, Ordering::SeqCst);
        self.leaf.resume.notify_one();
    }
    /// Ask the held real leaf to prepare while it still borrows the exact
    /// native call and resolved plan. Copies of those values are not custody.
    async fn prepare_task(
        &self, source: &InstalledServiceTaskSource, kind: ReportPreparation,
    ) -> Result<InstalledServiceTask, PrivateOriginalError> {
        assert!(!self.leaf.released.load(Ordering::SeqCst));
        let (reply, received) = tokio::sync::oneshot::channel();
        assert!(self.leaf.preparation.lock().unwrap().replace(ReportPreparationRequest {
            source: source.clone(), kind, reply,
        }).is_none());
        self.leaf.resume.notify_one();
        received.await.expect("held report dispatch must settle preparation")?
            .ok_or(PrivateOriginalError::Unavailable)
    }
}

/// Forward every native dispatcher seam unchanged. The pause retains the
/// actual context outside the immutable leaf and native bundle registration.
struct ReportLeaf {
    inner: Arc<dyn AgentToolDispatcher>,
    context: Mutex<Option<ToolDispatchContext>>,
    preparation: Mutex<Option<ReportPreparationRequest>>,
    arrived: tokio::sync::Notify,
    released: AtomicBool,
    resume: tokio::sync::Notify,
}
#[async_trait]
impl AgentToolDispatcher for ReportLeaf {
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
        self.inner.dispatch(call).await
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.pause(call, context, None).await;
        self.inner.dispatch_with_context(call, context).await
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        // Borrow the native plan only. Its root-dispatcher lease owns this
        // leaf, so retaining a clone here would keep the old route alive.
        self.pause(call, context, Some(plan)).await;
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
        // The actual report leaf has no lifecycle state to bind. Forward
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

impl ReportLeaf {
    async fn pause(
        &self, call: ToolCallView<'_>, context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    ) {
        assert!(self.context.lock().unwrap().replace(context.clone()).is_none());
        self.arrived.notify_one();
        loop {
            let resumed = self.resume.notified();
            let preparation = self.preparation.lock().unwrap().take();
            if let Some(request) = preparation {
                assert_eq!(call.id, REPORT_CALL);
                assert_eq!(call.name, SETUP_REPORT);
                let copied_args = serde_json::value::RawValue::from_string(call.args.get().to_owned()).unwrap();
                assert!(context.check_tool_authorization(ToolCallView {
                    id: call.id, name: call.name, args: &copied_args,
                }, plan).is_err(), "equal argument bytes are not native call custody");
                if let Some(plan) = plan {
                    let copied_plan = plan.clone();
                    assert!(context.check_tool_authorization(call, Some(&copied_plan)).is_err(),
                        "equal resolved plan values are not native plan custody");
                }
                let result = match context.check_tool_authorization(call, plan) {
                    Err(_) => Err(PrivateOriginalError::Denied),
                    Ok(_) => match request.kind {
                        ReportPreparation::Software => request.source.prepare_software(call, context, plan).await,
                        ReportPreparation::CapturedConsumer => request.source.prepare_captured_consumer(call, context, plan).await,
                        ReportPreparation::ExplicitConsumer => request.source.prepare(call, context, plan, MEMBER).await,
                        ReportPreparation::PublicService => request.source.prepare_public_service(call, context, plan).await,
                    },
                };
                // A timed-out test may have dropped its receiver; it still
                // releases and joins this real dispatch through normal cleanup.
                let _ = request.reply.send(result);
            }
            if self.released.load(Ordering::SeqCst) { return; }
            resumed.await;
        }
    }
}

struct ReportResponse { inner: Arc<dyn LlmClient>, report: Arc<Report> }
#[async_trait]
impl LlmClient for ReportResponse {
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
        // Only the separate helper profile offers this real registered tool.
        // Model scripting selects a response, never runtime identity/authority.
        let helper = request.request().tools.iter().any(|tool| tool.name == SETUP_REPORT);
        if helper {
            let mut feedback = self.report.feedback.lock().unwrap();
            for message in &request.request().messages {
                if let Message::ToolResults { results, .. } = message {
                    feedback.extend(results.iter().filter(|result| result.tool_use_id == REPORT_CALL).cloned());
                }
            }
        }
        let first = helper && self.report.first.swap(false, Ordering::SeqCst);
        let stream = self.inner.stream_prepared(request);
        if !first { return stream; }
        Box::pin(stream.map(|event| event.map(|event| match event {
            LlmEvent::TextDelta { .. } => LlmEvent::ToolCallComplete {
                id: REPORT_CALL.into(), name: SETUP_REPORT.into(),
                args: serde_json::json!({"kind":"unavailable","detail":DETAIL}), meta: None,
            },
            LlmEvent::Done { outcome: LlmDoneOutcome::Success { .. } } => LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::ToolUse),
            },
            other => other,
        })))
    }
    fn provider(&self) -> Provider {
        self.inner.provider()
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }
}


#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn narrow_original_report_scope_refuses_locally_and_helper_turn_completes() {
    let script = acquisition_denial::Script::for_approved();
    let report = Report::new(script.tools().result_tool());
    let (fixture, runtime_store) = Fixture::for_helper_report(script.clone(), report.clone()).await;
    let deadline = tokio::time::Instant::now() + WAIT;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout_at(deadline, Box::pin(async {
        let helper = AgentIdentity::parse(crate::installed_host::controller_first_input_fixture::HELPER).unwrap();
        let helper_member = MemberIdentity::from(helper.as_str());
        let helper_session = fixture.native.mob_handle().get_member(&helper_member).await.unwrap().unwrap()
            .bridge_session_id().cloned().unwrap();
        assert_ne!(helper_session, fixture.session);
        let helper_before = fixture.runtime.completion_cursor(&helper).await;
        acquisition_approved::select_reviewed_setup(&fixture).await;
        let first = fixture.submit().await;
        assert!(matches!(&first, Ok(PrivateOriginalSubmission::Admitted { turn_ticket: Some(_), .. })),
            "actual A admission required: {}", submission_context(&first));
        fixture.finish(&first).await.unwrap();
        let (acquisition_feedback, planned, entered) = script.observations();
        assert_eq!(acquisition_feedback.len(), 1);
        assert!(!acquisition_feedback[0].is_error);
        let reply: serde_json::Value = serde_json::from_str(&acquisition_feedback[0].text_content()).unwrap();
        assert_eq!(reply["worker"]["state"], "input_admitted", "{reply}");
        let original = fixture.binding().await.unwrap().unwrap();
        assert!(matches!(original.candidate().authority_basis,
            meerkat_authorization_contracts::work_association::WorkAuthorityBasis::GrantLineage { .. }));
        let helper_after = fixture.runtime.wait_for_completion(&helper, helper_before, WAIT).await.unwrap();
        assert_ne!(helper_before, helper_after, "refusal permits the same helper turn's completion");
        assert!(report.leaf.context.lock().unwrap().is_none(), "refusal precedes actual report dispatch");
        let feedback = report.feedback.lock().unwrap().clone();
        assert_eq!(feedback.len(), 1);
        assert_eq!(feedback[0].tool_use_id, REPORT_CALL);
        assert!(feedback[0].is_error, "{feedback:?}");
        let refusal: serde_json::Value = serde_json::from_str(&feedback[0].text_content()).unwrap();
        assert_eq!(refusal["error"], "operation_refused", "{refusal}");
        let helper_source = {
            let sources = fixture.source.helper_sources.lock().unwrap();
            assert_eq!(sources.len(), 1);
            sources[0].original().association().clone()
        };
        acquisition_approved::consumed_native_input(runtime_store.as_ref(), &helper_session,
            &helper_source, None, deadline).await;
        let rows = fixture.store.list_records(&acquisition_approved::scope(&fixture), None, 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        let row = fixture.store.get(&rows[0].key).await.unwrap().unwrap();
        assert!(row.data.body.get("helper_detail").is_none(), "refused report cannot change canonical setup result");
        assert!(planned > 0); assert_eq!(entered, 1);
    }))).catch_unwind().await;
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("narrow helper report refusal did not settle before the fixture deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

// The configured producer control separately supplies A's correlated targets.
#[path = "helper_report_producer_tests.rs"]
mod configured_parent;

#[path = "helper_report_preparation_tests.rs"]
mod legacy_preparation;

#[path = "warm_service_issuance_tests.rs"]
mod warm_service_issuance;

#[path = "warm_reacquisition_native_tests.rs"]
mod warm_reacquisition;
