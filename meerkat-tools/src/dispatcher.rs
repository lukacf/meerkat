//! Tool dispatcher implementation

#[cfg(not(target_arch = "wasm32"))]
use crate::error::DispatchError;
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
#[cfg(test)]
use meerkat_core::ToolCallability;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::ToolUnavailableReason;
use meerkat_core::error::ToolError;
use meerkat_core::ops::ToolDispatchOutcome;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::types::ToolResult;
use meerkat_core::types::{ToolCallView, ToolDef};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::{ToolCatalogCapabilities, ToolCatalogEntry, ToolDispatchContext};
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashSet;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::RwLock;

#[cfg(not(target_arch = "wasm32"))]
use crate::registry::{ToolIdentityEntry, ToolIdentityRegistry};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::error::ToolValidationError;
#[cfg(not(target_arch = "wasm32"))]
use serde_json::Value;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

/// An empty tool dispatcher that has no tools and always returns NotFound
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyToolDispatcher;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for EmptyToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::NotFound {
            name: call.name.into(),
        })
    }
}

/// A high-level tool dispatcher that validates arguments and handles timeouts
#[cfg(not(target_arch = "wasm32"))]
pub struct ToolDispatcher {
    router: Arc<dyn AgentToolDispatcher>,
    identity_registry: RwLock<ToolIdentityRegistry>,
    default_timeout: Duration,
}

#[cfg(not(target_arch = "wasm32"))]
impl ToolDispatcher {
    /// Create a new tool dispatcher
    pub fn new(router: Arc<dyn AgentToolDispatcher>) -> Self {
        let initial_catalog = Self::catalog_snapshot_for(router.as_ref());
        Self {
            router,
            identity_registry: RwLock::new(ToolIdentityRegistry::from_catalog(&initial_catalog)),
            default_timeout: crate::timeout::ToolTimeoutPolicy::default().default_timeout(),
        }
    }

    /// Set the default timeout for tool execution
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    fn catalog_snapshot_for(router: &dyn AgentToolDispatcher) -> Arc<[ToolCatalogEntry]> {
        if router.tool_catalog_capabilities().exact_catalog {
            router.tool_catalog()
        } else {
            router
                .tools()
                .iter()
                .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true))
                .collect::<Vec<_>>()
                .into()
        }
    }

    fn live_catalog_snapshot(&self) -> Arc<[ToolCatalogEntry]> {
        Self::catalog_snapshot_for(self.router.as_ref())
    }

    fn refresh_identity_registry(&self, catalog: &[ToolCatalogEntry]) {
        let mut registry = match self.identity_registry.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        for entry in catalog {
            registry.register(Arc::clone(&entry.tool));
        }
        // Reconcile against the live catalog: any previously-admitted tool the
        // live catalog no longer claims is retired (vanished from the source
        // set), so it is never republished as `NotCurrentlyCallable` catalog
        // truth and dispatch resolves it as `NotFound`. Tools that are listed
        // but momentarily uncallable remain in the live catalog carrying their
        // own callability and are unaffected here.
        registry.reconcile_to_live(catalog.iter().map(|entry| entry.tool.name.as_str()));
    }

    fn registry_entries(&self) -> Vec<ToolIdentityEntry> {
        match self.identity_registry.read() {
            Ok(registry) => registry.iter().cloned().collect(),
            Err(poisoned) => poisoned.into_inner().iter().cloned().collect(),
        }
    }

    fn admitted_catalog_snapshot(&self) -> Arc<[ToolCatalogEntry]> {
        let live_catalog = self.live_catalog_snapshot();
        self.refresh_identity_registry(live_catalog.as_ref());

        let mut emitted = HashSet::new();
        let mut catalog = Vec::new();
        // Live registry entries are, by reconciliation, a subset of the live
        // catalog: emit them (in admission order) carrying their real
        // callability. A retired (vanished) tool is not in `registry_entries()`
        // anymore, so it is never republished as an unavailable catalog entry.
        for registry_entry in self.registry_entries() {
            let name = registry_entry.identity.name.as_str();
            if let Some(live_entry) = live_catalog
                .iter()
                .find(|entry| entry.tool.name == name)
                .cloned()
                && emitted.insert(name.to_string())
            {
                catalog.push(live_entry);
            }
        }

        for entry in live_catalog.iter() {
            if emitted.insert(entry.tool.name.to_string()) {
                catalog.push(entry.clone());
            }
        }

        catalog.into()
    }

    fn route_not_found_as_unavailable(name: &str, err: ToolError) -> ToolError {
        match err {
            ToolError::NotFound { name: err_name } if err_name == name => {
                ToolError::unavailable(name, ToolUnavailableReason::NotCurrentlyCallable)
            }
            other => other,
        }
    }

    fn live_tool_def(&self, name: &str) -> Result<Arc<ToolDef>, ToolError> {
        let live_catalog = self.live_catalog_snapshot();
        self.refresh_identity_registry(live_catalog.as_ref());

        if let Some(entry) = live_catalog
            .iter()
            .find(|entry| entry.tool.name == name)
            .cloned()
        {
            if let Some(reason) = entry.callability.unavailable_reason() {
                return Err(ToolError::unavailable(name, reason));
            }
            return Ok(entry.tool);
        }

        // The live catalog no longer claims this tool. After reconciliation a
        // registered identity is only retained as `Live` while it remains in the
        // live catalog, so a name absent here has vanished from the source set:
        // resolve as NotFound rather than fabricating NotCurrentlyCallable for a
        // tool that no longer exists.
        Err(ToolError::NotFound { name: name.into() })
    }

    fn validate_args_for_tool(
        tool: &ToolDef,
        name: &str,
        args: &Value,
    ) -> Result<(), ToolValidationError> {
        crate::registry::validate_tool_def(tool, name, args)
    }

    /// Dispatch a tool call
    pub async fn dispatch_call(&self, call: ToolCallView<'_>) -> Result<ToolResult, DispatchError> {
        self.dispatch_call_with_resolution_elapsed(call, None).await
    }

    async fn dispatch_call_with_resolution_elapsed(
        &self,
        call: ToolCallView<'_>,
        resolution_elapsed_override: Option<Duration>,
    ) -> Result<ToolResult, DispatchError> {
        let args: Value = serde_json::from_str(call.args.get())
            .map_err(|e| ToolValidationError::invalid_arguments(call.name, e.to_string()))?;
        let tool = self.live_tool_def(call.name)?;
        Self::validate_args_for_tool(tool.as_ref(), call.name, &args)?;

        let dispatch_context = ToolDispatchContext::default();
        let resolution_context = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::Dispatcher,
                    self.default_timeout,
                ),
            ])
            .map_err(meerkat_core::ToolExecutionResolutionError::from)
            .map_err(ToolError::from)?,
        );
        let resolution_started = std::time::Instant::now();
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &self.router,
            call,
            &dispatch_context,
            &resolution_context,
        )
        .map_err(ToolError::from)?;
        self.router
            .validate_resolved_execution_plan(call, &resolution_context, &plan)
            .map_err(ToolError::from)?;
        let effective_timeout = plan
            .deadlines()
            .effective_timeout()
            .unwrap_or(self.default_timeout);
        let timeout = effective_timeout.saturating_sub(
            resolution_elapsed_override.unwrap_or_else(|| resolution_started.elapsed()),
        );
        if timeout.is_zero() {
            return Err(DispatchError::Timeout {
                timeout_ms: effective_timeout.as_millis() as u64,
            });
        }
        let outcome = tokio::time::timeout(
            timeout,
            meerkat_core::dispatch_tool_execution_plan_fenced(
                &self.router,
                call,
                &dispatch_context,
                &plan,
            ),
        )
        .await
        .map_err(|_| DispatchError::Timeout {
            timeout_ms: effective_timeout.as_millis() as u64,
        })?
        .map_err(|err| Self::route_not_found_as_unavailable(call.name, err))?;

        Ok(outcome.result)
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl AgentToolDispatcher for ToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tool_catalog()
            .iter()
            .filter(|entry| entry.currently_callable())
            .map(|entry| Arc::clone(&entry.tool))
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatch_with_context(call, &ToolDispatchContext::default())
            .await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let args: Value =
            serde_json::from_str(call.args.get()).map_err(|e| ToolError::InvalidArguments {
                name: call.name.into(),
                reason: e.to_string(),
            })?;
        let tool = self.live_tool_def(call.name)?;
        // Validate arguments against schema
        Self::validate_args_for_tool(tool.as_ref(), call.name, &args).map_err(|e| match e {
            ToolValidationError::NotFound { name } => ToolError::NotFound { name },
            ToolValidationError::InvalidArguments { name, reason } => {
                ToolError::InvalidArguments { name, reason }
            }
        })?;

        // Dispatch with timeout to prevent hanging tool calls
        tokio::time::timeout(
            self.default_timeout,
            self.router.dispatch_with_context(call, context),
        )
        .await
        .map_err(|_| ToolError::timeout(call.name, self.default_timeout.as_millis() as u64))?
        .map_err(|err| Self::route_not_found_as_unavailable(call.name, err))
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let args: Value =
            serde_json::from_str(call.args.get()).map_err(|e| ToolError::InvalidArguments {
                name: call.name.into(),
                reason: e.to_string(),
            })?;
        let tool = self.live_tool_def(call.name)?;
        Self::validate_args_for_tool(tool.as_ref(), call.name, &args).map_err(|e| match e {
            ToolValidationError::NotFound { name } => ToolError::NotFound { name },
            ToolValidationError::InvalidArguments { name, reason } => {
                ToolError::InvalidArguments { name, reason }
            }
        })?;

        tokio::time::timeout(
            self.default_timeout,
            self.router
                .dispatch_resolved_with_context(call, context, plan),
        )
        .await
        .map_err(|_| ToolError::timeout(call.name, self.default_timeout.as_millis() as u64))?
        .map_err(|err| Self::route_not_found_as_unavailable(call.name, err))
    }

    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        self.router.external_tool_surface_snapshot()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: self
                .router
                .tool_catalog_capabilities()
                .may_require_catalog_control_plane,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.admitted_catalog_snapshot()
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.router.pending_catalog_sources()
    }

    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<
        meerkat_core::EphemeralToolBindingFingerprint,
        meerkat_core::ToolExecutionResolutionError,
    > {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == tool_name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })?;
        Ok(
            meerkat_core::ephemeral_tool_catalog_binding_fingerprint(entry)
                .with_live_authority(0, 0)
                .with_dependency(&self.router.execution_binding_fingerprint(tool_name)?),
        )
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let resolution_context =
            resolution_context.with_deadline(meerkat_core::ToolDeadlineContributor::finite(
                meerkat_core::ToolDeadlineOwner::Dispatcher,
                self.default_timeout,
            ))?;
        let plan =
            self.router
                .resolve_execution_plan(call, dispatch_context, &resolution_context)?;
        resolution_context.validate_resolved_plan(&plan)?;
        Ok(plan)
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::{
        ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
        ToolExecutionResolutionContext,
    };
    use serde_json::json;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct MutableExactDispatcher {
        tool_names: Mutex<Vec<&'static str>>,
    }

    struct UnroutableVisibleDispatcher {
        tool_name: &'static str,
    }

    struct EpochDispatcher {
        tool: ToolDef,
        epoch: AtomicU64,
    }

    #[async_trait]
    impl AgentToolDispatcher for EpochDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([Arc::new(self.tool.clone())])
        }

        fn execution_binding_epoch(&self, _tool_name: &str) -> u64 {
            self.epoch.load(Ordering::Acquire)
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(call.id.to_string(), "ok".to_string(), false).into())
        }
    }

    impl MutableExactDispatcher {
        fn new(names: Vec<&'static str>) -> Self {
            Self {
                tool_names: Mutex::new(names),
            }
        }

        fn add_tool(&self, name: &'static str) {
            self.tool_names.lock().unwrap().push(name);
        }

        fn remove_tool(&self, name: &str) {
            self.tool_names.lock().unwrap().retain(|tool| *tool != name);
        }
    }

    struct ExactMockDispatcher {
        catalog: Arc<[ToolCatalogEntry]>,
    }

    struct ContextAwareDispatcher;

    struct DeadlineErasingDispatcher;

    struct ResolvedOnlyDispatcher {
        saw_dispatcher_deadline: AtomicU64,
        dispatch_count: AtomicU64,
    }

    impl ExactMockDispatcher {
        fn with_unavailable_reason(name: &str, reason: ToolUnavailableReason) -> Self {
            let catalog = vec![ToolCatalogEntry::session_inline_with_callability(
                Arc::new(ToolDef {
                    name: name.into(),
                    description: format!("{name} tool"),
                    input_schema: json!({"type": "object"}),
                    provenance: None,
                }),
                ToolCallability::unavailable(reason),
            )]
            .into();
            Self { catalog }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for ContextAwareDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([tool_def("inspect_context")])
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"saw_context_image": false}).to_string(),
                false,
            )
            .into())
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &ToolDispatchContext,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            let saw_context_image = context
                .current_turn()
                .and_then(|turn| turn.image_ref(0))
                .and_then(|image_ref| context.current_turn_image(image_ref))
                .is_some();
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"saw_context_image": saw_context_image}).to_string(),
                false,
            )
            .into())
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for DeadlineErasingDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([tool_def("erase_deadline")])
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(call.id.to_string(), "ok".to_string(), false).into())
        }

        fn resolve_execution_plan(
            &self,
            _call: ToolCallView<'_>,
            _dispatch_context: &ToolDispatchContext,
            _resolution_context: &ToolExecutionResolutionContext,
        ) -> Result<
            meerkat_core::ResolvedToolExecutionPlan,
            meerkat_core::ToolExecutionResolutionError,
        > {
            let erased = ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap();
            meerkat_core::ToolExecutionContract::default()
                .resolve_default(erased)
                .map_err(Into::into)
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for ResolvedOnlyDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([tool_def("resolved_only")])
        }

        fn resolve_execution_plan(
            &self,
            call: ToolCallView<'_>,
            context: &ToolDispatchContext,
            resolution: &ToolExecutionResolutionContext,
        ) -> Result<
            meerkat_core::ResolvedToolExecutionPlan,
            meerkat_core::ToolExecutionResolutionError,
        > {
            if resolution
                .deadlines()
                .contributors()
                .iter()
                .any(|contributor| {
                    contributor.owner() == ToolDeadlineOwner::Dispatcher
                        && contributor.timeout() == Some(Duration::from_millis(17))
                })
            {
                self.saw_dispatcher_deadline.store(1, Ordering::Release);
            }
            let _ = (call, context);
            meerkat_core::ToolExecutionContract::default()
                .resolve_default(resolution.deadlines().clone())
                .map_err(Into::into)
        }

        async fn dispatch(
            &self,
            _call: ToolCallView<'_>,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            Err(ToolError::execution_failed(
                "plain dispatch must not be used by dispatch_call",
            ))
        }

        async fn dispatch_resolved_with_context(
            &self,
            call: ToolCallView<'_>,
            _context: &ToolDispatchContext,
            _plan: &meerkat_core::ResolvedToolExecutionPlan,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            self.dispatch_count.fetch_add(1, Ordering::AcqRel);
            Ok(ToolResult::new(call.id.to_string(), "resolved".to_string(), false).into())
        }
    }

    fn tool_def(name: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef {
            name: name.into(),
            description: format!("{name} tool"),
            input_schema: json!({"type": "object"}),
            provenance: None,
        })
    }

    fn make_call<'a>(name: &'a str, args_raw: &'a serde_json::value::RawValue) -> ToolCallView<'a> {
        ToolCallView {
            id: "test-1",
            name,
            args: args_raw,
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for MutableExactDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.tool_catalog()
                .iter()
                .filter(|entry| entry.currently_callable())
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into()
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            self.tool_names
                .lock()
                .unwrap()
                .iter()
                .map(|name| ToolCatalogEntry::session_inline(tool_def(name), true))
                .collect::<Vec<_>>()
                .into()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            if self.tool_names.lock().unwrap().contains(&call.name) {
                Ok(ToolResult::new(
                    call.id.to_string(),
                    json!({"called": call.name}).to_string(),
                    false,
                )
                .into())
            } else {
                Err(ToolError::NotFound {
                    name: call.name.into(),
                })
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for UnroutableVisibleDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([tool_def(self.tool_name)])
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            Arc::from([ToolCatalogEntry::session_inline(
                tool_def(self.tool_name),
                true,
            )])
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Err(ToolError::NotFound {
                name: call.name.into(),
            })
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for ExactMockDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.catalog
                .iter()
                .filter(|entry| entry.currently_callable())
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into()
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            let Some(entry) = self
                .catalog
                .iter()
                .find(|entry| entry.tool.name == call.name)
            else {
                return Err(ToolError::NotFound {
                    name: call.name.into(),
                });
            };
            if let Some(reason) = entry.callability.unavailable_reason() {
                return Err(ToolError::unavailable(call.name, reason));
            }
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"called": call.name}).to_string(),
                false,
            )
            .into())
        }
    }

    fn image_dispatch_context() -> ToolDispatchContext {
        ToolDispatchContext::from_current_turn_input(&meerkat_core::ContentInput::Blocks(vec![
            meerkat_core::ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "abc".into(),
            },
        ]))
    }

    #[tokio::test]
    async fn tool_dispatcher_preserves_dispatch_context() {
        let router: Arc<dyn AgentToolDispatcher> = Arc::new(ContextAwareDispatcher);
        let dispatcher = ToolDispatcher::new(router);
        let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
        let context = image_dispatch_context();

        let outcome = dispatcher
            .dispatch_with_context(make_call("inspect_context", &args_raw), &context)
            .await
            .expect("tool dispatcher should dispatch");
        let payload: serde_json::Value =
            serde_json::from_str(&outcome.result.text_content()).expect("tool result JSON");
        assert_eq!(payload["saw_context_image"], true);
    }

    #[tokio::test]
    async fn public_dispatch_call_uses_fenced_resolved_path_and_dispatcher_deadline() {
        let router = Arc::new(ResolvedOnlyDispatcher {
            saw_dispatcher_deadline: AtomicU64::new(0),
            dispatch_count: AtomicU64::new(0),
        });
        let dispatcher =
            ToolDispatcher::new(router.clone()).with_timeout(Duration::from_millis(17));
        let args_raw = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();

        let result = dispatcher
            .dispatch_call(make_call("resolved_only", &args_raw))
            .await
            .expect("public dispatch uses the resolved path");

        assert_eq!(result.text_content(), "resolved");
        assert_eq!(router.saw_dispatcher_deadline.load(Ordering::Acquire), 1);
        assert_eq!(router.dispatch_count.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn public_dispatch_call_never_polls_inner_after_resolution_exhausts_budget() {
        let router = Arc::new(ResolvedOnlyDispatcher {
            saw_dispatcher_deadline: AtomicU64::new(0),
            dispatch_count: AtomicU64::new(0),
        });
        let dispatcher = ToolDispatcher::new(router.clone()).with_timeout(Duration::from_nanos(1));
        let args_raw = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();

        let error = dispatcher
            .dispatch_call_with_resolution_elapsed(
                make_call("resolved_only", &args_raw),
                Some(Duration::from_nanos(1)),
            )
            .await
            .expect_err("an exhausted budget must fail before polling the ready dispatcher");

        assert!(matches!(error, DispatchError::Timeout { timeout_ms: 0 }));
        assert_eq!(
            router.dispatch_count.load(Ordering::Acquire),
            0,
            "tokio::time::timeout(0, ready_future) must never get a chance to poll inner"
        );
    }

    #[test]
    fn tool_dispatcher_appends_its_enforced_timeout_to_execution_plan() {
        let router: Arc<dyn AgentToolDispatcher> = Arc::new(ContextAwareDispatcher);
        let dispatcher = ToolDispatcher::new(router);
        let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
        let context = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap(),
        );

        let plan = dispatcher
            .resolve_execution_plan(
                make_call("inspect_context", &args_raw),
                &ToolDispatchContext::default(),
                &context,
            )
            .expect("tool dispatcher should forward plan resolution");

        assert_eq!(
            plan.deadlines().effective_timeout(),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            plan.deadlines()
                .winner()
                .map(ToolDeadlineContributor::owner),
            Some(ToolDeadlineOwner::Dispatcher)
        );
    }

    #[test]
    fn tool_dispatcher_binding_fingerprint_composes_inner_epoch() {
        let inner = Arc::new(EpochDispatcher {
            tool: ToolDef::new("live", "live tool", serde_json::json!({"type": "object"})),
            epoch: AtomicU64::new(0),
        });
        let dispatcher = ToolDispatcher::new(inner.clone());
        let before = dispatcher
            .execution_binding_fingerprint("live")
            .expect("initial live binding");

        inner.epoch.fetch_add(1, Ordering::AcqRel);

        let after = dispatcher
            .execution_binding_fingerprint("live")
            .expect("updated live binding");
        assert_ne!(before, after);
    }

    #[test]
    fn tool_dispatcher_rejects_child_plan_that_erases_dispatcher_deadline() {
        let router: Arc<dyn AgentToolDispatcher> = Arc::new(DeadlineErasingDispatcher);
        let dispatcher = ToolDispatcher::new(router);
        let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
        let context = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap(),
        );

        let error = dispatcher
            .resolve_execution_plan(
                make_call("erase_deadline", &args_raw),
                &ToolDispatchContext::default(),
                &context,
            )
            .expect_err("a child resolver must not erase the dispatcher deadline");

        assert!(matches!(
            error,
            meerkat_core::ToolExecutionResolutionError::DeadlineExtension(_)
        ));
    }

    #[tokio::test]
    async fn dispatcher_preserves_typed_unavailable_reason() {
        let router = Arc::new(ExactMockDispatcher::with_unavailable_reason(
            "peers",
            ToolUnavailableReason::NoPeersConfigured,
        ));
        let dispatcher = ToolDispatcher::new(router);

        let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
        let err = dispatcher
            .dispatch(make_call("peers", &args_raw))
            .await
            .unwrap_err();

        let reason = match &err {
            ToolError::Unavailable { reason, .. } => Some(*reason),
            _ => None,
        };
        assert_eq!(reason, Some(ToolUnavailableReason::NoPeersConfigured));
        assert!(err.to_string().contains("no peers configured"));
    }

    #[tokio::test]
    async fn dispatcher_admits_late_exact_catalog_identity_after_construction() {
        let router = Arc::new(MutableExactDispatcher::new(vec!["initial"]));
        let dispatcher = ToolDispatcher::new(router.clone());

        let initial_names: Vec<_> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(initial_names, vec!["initial".to_string()]);

        router.add_tool("late");

        let live_names: Vec<_> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(
            live_names,
            vec!["initial".to_string(), "late".to_string()],
            "dispatcher must admit tool identities added after construction"
        );

        let catalog_names: Vec<_> = dispatcher
            .tool_catalog()
            .iter()
            .map(|entry| entry.tool.name.to_string())
            .collect();
        assert_eq!(
            catalog_names,
            vec!["initial".to_string(), "late".to_string()]
        );

        let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
        let result = dispatcher.dispatch(make_call("late", &args_raw)).await;
        assert!(result.is_ok());

        router.remove_tool("initial");
        let live_names: Vec<_> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(live_names, vec!["late".to_string()]);

        // A tool removed from the router's source set has VANISHED: it is
        // retired in the identity registry and must NOT be republished as an
        // unavailable catalog entry (that would advertise dead truth). The
        // admitted catalog now contains only the still-live tool.
        let catalog = dispatcher.tool_catalog();
        let catalog_names: Vec<_> = catalog
            .iter()
            .map(|entry| entry.tool.name.to_string())
            .collect();
        assert_eq!(
            catalog_names,
            vec!["late".to_string()],
            "a vanished tool must not be emitted as an unavailable catalog entry"
        );
        assert!(
            catalog
                .iter()
                .find(|entry| entry.tool.name == "late")
                .expect("late remains admitted")
                .currently_callable()
        );

        // Dispatch of a vanished tool resolves as NotFound, not a fabricated
        // NotCurrentlyCallable.
        let result = dispatcher.dispatch(make_call("initial", &args_raw)).await;
        assert!(
            matches!(result, Err(ToolError::NotFound { .. })),
            "a tool removed from the source set must resolve as NotFound, got {result:?}"
        );
    }

    #[tokio::test]
    async fn dispatcher_fails_closed_when_visible_identity_is_not_routable() {
        let router = Arc::new(UnroutableVisibleDispatcher {
            tool_name: "visible",
        });
        let dispatcher = ToolDispatcher::new(router);

        assert_eq!(
            dispatcher
                .tools()
                .iter()
                .map(|tool| tool.name.to_string())
                .collect::<Vec<_>>(),
            vec!["visible".to_string()]
        );

        let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
        let err = dispatcher
            .dispatch(make_call("visible", &args_raw))
            .await
            .unwrap_err();

        let reason = match &err {
            ToolError::Unavailable { reason, .. } => Some(*reason),
            _ => None,
        };
        assert_eq!(reason, Some(ToolUnavailableReason::NotCurrentlyCallable));
    }

    /// Gate (#79): the dispatcher enforcement default and the shell config
    /// default both flow from the single [`ToolTimeoutPolicy`] owner. There is
    /// no divergent hardcoded literal: the dispatcher's `Duration` default and
    /// the shell config's `u64`-seconds default are the same value expressed in
    /// two units, both sourced from the one policy.
    #[test]
    fn timeout_default_is_single_sourced_across_call_sites() {
        use crate::builtin::shell::ShellConfig;
        use crate::timeout::ToolTimeoutPolicy;

        let policy = ToolTimeoutPolicy::default();

        // Dispatcher default reads from the policy.
        let dispatcher = ToolDispatcher::new(Arc::new(EmptyToolDispatcher));
        assert_eq!(
            dispatcher.default_timeout,
            policy.default_timeout(),
            "dispatcher default must be sourced from ToolTimeoutPolicy"
        );

        // Shell config default reads from the same policy.
        let shell = ShellConfig::default();
        assert_eq!(
            shell.default_timeout_secs,
            policy.default_timeout_secs(),
            "shell config default must be sourced from ToolTimeoutPolicy"
        );

        // The two call sites therefore agree (no divergent defaults).
        assert_eq!(
            Duration::from_secs(shell.default_timeout_secs),
            dispatcher.default_timeout,
            "dispatcher and shell defaults must not diverge"
        );

        // Builder config default also flows from the policy.
        let builder_default = crate::builder::ToolDispatcherConfig::default();
        assert_eq!(
            builder_default.default_timeout,
            policy.default_timeout(),
            "builder config default must be sourced from ToolTimeoutPolicy"
        );
    }
}
