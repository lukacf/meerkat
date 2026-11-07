//! Stage exact workflow selection after the existing native registration resolves.
use super::*;
use crate::private_originals::setup_workflow::execution::WorkflowCustodyRefusal;
use meerkat_core::{
    AgentToolDispatcher, ResolvedToolExecutionPlan, ToolCallView, ToolCatalogCapabilities,
    ToolCatalogEntry, ToolDef, ToolDispatchContext, ToolError, ToolExecutionResolutionContext,
    ToolExecutionResolutionError, ToolMutationClass, ops::ToolDispatchOutcome,
};

// A view of the existing host owner, not a new source or grant registry.
struct WorkflowCalls {
    inner: Arc<dyn AgentToolDispatcher>,
    source: InstalledServiceTaskSource,
}

fn workflow_failure(name: &str, error: PrivateOriginalError) -> ToolError {
    match error {
        PrivateOriginalError::Unavailable => ToolError::unavailable(
            name,
            meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
        ),
        PrivateOriginalError::Denied | PrivateOriginalError::Conflict => {
            ToolError::access_denied(name)
        }
    }
}

impl InstalledServiceTaskSource {
    /// Typed local refusal can retire an exact unused reservation. Transport
    /// loss, generic failures and callbacks retain custody until its owner
    /// proves settlement. The gate itself refuses to settle entered calls.
    pub fn settle_workflow_dispatch_refusal(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&ResolvedToolExecutionPlan>,
        error: ToolError,
    ) -> ToolError {
        if !known_refusal(&error) {
            return error;
        }
        match self.settle_unentered_workflow_call(call, context, plan) {
            Ok(()) => error,
            Err(_) => {
                error.with_settlement_failures(vec![meerkat_core::ToolDispatchSettlementFailure {
                    admission_source: meerkat_core::ToolDispatchAdmissionSource::ContextGate,
                    effect_kind: meerkat_core::LiveBridgeEffectKind::ToolDispatch,
                    // A failed custody lookup cannot prove the source gate's state.
                    physical_outcome: meerkat_core::LiveBridgeEffectOutcome::Unknown,
                    failure_kind: meerkat_core::ToolDispatchTerminalErrorKind::Unavailable,
                }])
            }
        }
    }
    async fn workflow_entry(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<
        Option<Arc<crate::original_ingress::workflow_custody::WorkflowCallCustody>>,
        WorkflowCustodyRefusal,
    > {
        let selected = self
            .selected
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let owner = selected
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .configured();
        match owner {
            Some(owner) => {
                let mut churn = context.owner_change_churn();
                owner
                    .host
                    .originals
                    .workflow_task_custody(call, context, plan, &mut churn)
                    .await
                    .map(|custody| custody.map(|custody| custody.entry))
            }
            None => Ok(None),
        }
    }
    /// Wrap an already registered dispatcher; its real native target is kept.
    pub fn with_workflow_calls(
        &self,
        inner: Arc<dyn AgentToolDispatcher>,
    ) -> Arc<dyn AgentToolDispatcher> {
        Arc::new(WorkflowCalls {
            inner,
            source: self.clone(),
        })
    }
}

fn known_refusal(error: &ToolError) -> bool {
    matches!(
        error.primary_error(),
        ToolError::AccessDenied { .. }
            | ToolError::InvalidArguments { .. }
            | ToolError::AuthorizationRefused { .. }
            | ToolError::OperationObservationUnavailable
            | ToolError::OperationAuthorizationUnavailable
            | ToolError::ReviewUnsatisfied { .. }
            | ToolError::ReviewUnavailable { .. }
            | ToolError::PolicyDenied { .. }
            | ToolError::PolicyIndeterminate { .. }
    )
}

impl WorkflowCalls {
    fn stage(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<(), ToolExecutionResolutionError> {
        // Context-free catalog probes can resolve an ordinary plan. A selector
        // still requires the actual native input, and entry always rechecks it.
        if context.work_authorization().is_none()
            || context.retained_work().is_none()
            || context.origin_session_id().is_none()
        {
            let value: serde_json::Value = serde_json::from_str(call.args.get()).map_err(|_| {
                ToolExecutionResolutionError::AccessDenied {
                    tool_name: call.name.into(),
                }
            })?;
            return if value.get("setup_acquisition").is_none() {
                Ok(())
            } else {
                Err(ToolExecutionResolutionError::AccessDenied {
                    tool_name: call.name.into(),
                })
            };
        }
        let staged = self.source.stage_workflow_call(call, context, plan);
        staged.map_err(|error| match error {
            PrivateOriginalError::Unavailable => ToolExecutionResolutionError::Unavailable {
                tool_name: call.name.into(),
                reason: meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
            },
            PrivateOriginalError::Denied | PrivateOriginalError::Conflict => {
                ToolExecutionResolutionError::AccessDenied {
                    tool_name: call.name.into(),
                }
            }
        })
    }
}

#[async_trait]
impl AgentToolDispatcher for WorkflowCalls {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }
    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        request: &meerkat_core::ToolApplicationRequest,
        invocation: &serde_json::Value,
        context: &ToolDispatchContext,
    ) -> Result<meerkat_core::tool_application::ToolApplicationResolution, ToolError> {
        self.inner
            .resolve_tool_application(source_tool, request, invocation, context)
            .await
    }
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }
    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.inner.tool_catalog()
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
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
    ) -> Result<meerkat_core::EphemeralToolBindingFingerprint, ToolExecutionResolutionError> {
        self.inner.execution_binding_fingerprint(name)
    }
    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        resolution: &ToolExecutionResolutionContext,
    ) -> Result<ResolvedToolExecutionPlan, ToolExecutionResolutionError> {
        let plan = self.inner.resolve_execution_plan(call, context, resolution)
            .inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.resolve.inner"}}"#))?;
        self.stage(call, context, &plan).inspect_err(|_| {
            eprintln!(
                r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.resolve.stage"}}"#
            )
        })?;
        Ok(plan)
    }
    fn validate_resolved_execution_plan(
        &self,
        call: ToolCallView<'_>,
        resolution: &ToolExecutionResolutionContext,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<(), ToolExecutionResolutionError> {
        self.inner
            .validate_resolved_execution_plan(call, resolution, plan)
    }
    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.inner.dispatch(call).await
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.inner.dispatch_with_context(call, context).await
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        // MCP uses the existing shared Controller entry owner. The registered
        // public operation has no Controller transport, so retain its exact
        // waiter gate across the actual request/review and synchronous result.
        let entry = self
            .source
            .workflow_entry(call, context, plan)
            .await
            .map_err(|refusal| match refusal {
                WorkflowCustodyRefusal::Refused(error) => {
                    super::task_approval::shared_task_refusal("workflow.entry", error);
                    workflow_failure(call.name, error)
                }
                // A refusal to rebuild stays Core's typed answer.
                WorkflowCustodyRefusal::Current(error) => error,
            })?;
        if call.name == meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST
            && let Some(entry) = &entry
        {
            entry.enter_source_gate().map_err(|error| {
                workflow_failure(
                    call.name,
                    match error {
                        crate::original_ingress::OriginalWriterError::Unavailable => {
                            PrivateOriginalError::Unavailable
                        }
                        _ => PrivateOriginalError::Denied,
                    },
                )
            })?;
        }
        let result = self
            .inner
            .dispatch_resolved_with_context(call, context, plan)
            .await;
        // Only a completed synchronous result proves the request owner joined.
        // An error after entry may follow issuance or failed compensation, so
        // even a typed refusal retains custody until its owner settles it.
        let settled = match &result {
            Ok(outcome) => outcome.async_ops.is_empty() && outcome.settlement_failures().is_empty(),
            Err(_) => false,
        };
        if settled && let Some(entry) = entry {
            entry.settle().map_err(|_| {
                ToolError::unavailable(
                    call.name,
                    meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
                )
            })?;
        }
        result.map_err(|error| {
            self.source
                .settle_workflow_dispatch_refusal(call, context, Some(plan), error)
        })
    }
    async fn poll_external_updates(&self) -> meerkat_core::ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }
    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        self.inner.external_tool_surface_snapshot()
    }
    fn capabilities(&self) -> meerkat_core::DispatcherCapabilities {
        self.inner.capabilities()
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
    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        owner: meerkat_core::SessionId,
    ) -> Result<meerkat_core::BindOutcome, meerkat_core::agent::OpsLifecycleBindError> {
        use meerkat_core::{BindOutcome, agent::OpsLifecycleBindError};
        let Self { inner, source } =
            Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        if inner.capabilities().ops_lifecycle && Arc::strong_count(&inner) != 1 {
            return Err(OpsLifecycleBindError::SharedOwnership);
        }
        if Arc::strong_count(&inner) == 1 {
            let result = inner.bind_ops_lifecycle(registry, owner)?;
            let bound = result.was_bound();
            let wrapped: Arc<dyn AgentToolDispatcher> = Arc::new(Self {
                inner: result.into_dispatcher(),
                source,
            });
            Ok(if bound {
                BindOutcome::Bound(wrapped)
            } else {
                BindOutcome::Skipped(wrapped)
            })
        } else {
            Ok(BindOutcome::Skipped(Arc::new(Self { inner, source })))
        }
    }
}
