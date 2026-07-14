//! Tool dispatcher implementation

#[cfg(not(target_arch = "wasm32"))]
use crate::error::DispatchError;
use async_trait::async_trait;
#[cfg(test)]
use meerkat_core::ToolCallability;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::ToolUnavailableReason;
use meerkat_core::error::ToolError;
use meerkat_core::ops::ToolDispatchOutcome;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::types::ToolResult;
use meerkat_core::types::{ToolCallView, ToolDef};
use meerkat_core::{AgentToolDispatcher, ToolDispatchContext};
use meerkat_core::{ToolCatalogCapabilities, ToolCatalogEntry};
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
        let args: Value = serde_json::from_str(call.args.get())
            .map_err(|e| ToolValidationError::invalid_arguments(call.name, e.to_string()))?;
        let tool = self.live_tool_def(call.name)?;
        // 1. Validate arguments against schema
        Self::validate_args_for_tool(tool.as_ref(), call.name, &args)?;

        // 2. Dispatch to router with timeout
        let outcome = tokio::time::timeout(self.default_timeout, self.router.dispatch(call))
            .await
            .map_err(|_| DispatchError::Timeout {
                timeout_ms: self.default_timeout.as_millis() as u64,
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
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    struct MutableExactDispatcher {
        tool_names: Mutex<Vec<&'static str>>,
    }

    struct UnroutableVisibleDispatcher {
        tool_name: &'static str,
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
