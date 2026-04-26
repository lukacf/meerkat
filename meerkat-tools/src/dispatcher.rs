//! Tool dispatcher implementation

#[cfg(not(target_arch = "wasm32"))]
use crate::error::DispatchError;
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::ops::{ToolAccessPolicy, ToolDispatchOutcome};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::types::ToolResult;
use meerkat_core::types::{ToolCallView, ToolDef};
use meerkat_core::{ToolCatalogCapabilities, ToolCatalogEntry};
use std::collections::HashSet;
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use crate::registry::ToolRegistry;
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
            name: call.name.to_string(),
        })
    }
}

/// A high-level tool dispatcher that validates arguments and handles timeouts
#[cfg(not(target_arch = "wasm32"))]
pub struct ToolDispatcher {
    router: Arc<dyn AgentToolDispatcher>,
    default_timeout: Duration,
}

#[cfg(not(target_arch = "wasm32"))]
impl ToolDispatcher {
    /// Create a new tool dispatcher
    pub fn new(router: Arc<dyn AgentToolDispatcher>) -> Self {
        Self {
            router,
            default_timeout: Duration::from_secs(30),
        }
    }

    /// Set the default timeout for tool execution
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    fn live_tool_def(&self, name: &str) -> Result<Arc<ToolDef>, ToolError> {
        if self.router.tool_catalog_capabilities().exact_catalog {
            let Some(entry) = self
                .router
                .tool_catalog()
                .iter()
                .find(|entry| entry.tool.name == name)
                .cloned()
            else {
                return Err(ToolError::NotFound {
                    name: name.to_string(),
                });
            };
            if let Some(reason) = entry.callability.unavailable_reason() {
                return Err(ToolError::unavailable(name, reason.to_string()));
            }
            return Ok(entry.tool);
        }

        self.router
            .tools()
            .iter()
            .find(|tool| tool.name == name)
            .cloned()
            .ok_or_else(|| ToolError::NotFound {
                name: name.to_string(),
            })
    }

    fn validate_args_for_tool(
        tool: &ToolDef,
        name: &str,
        args: &Value,
    ) -> Result<(), ToolValidationError> {
        ToolRegistry::validate_tool_def(tool, name, args)
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
            })??;

        Ok(outcome.result)
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl AgentToolDispatcher for ToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        if self.router.tool_catalog_capabilities().exact_catalog {
            return self
                .router
                .tool_catalog()
                .iter()
                .filter(|entry| entry.currently_callable())
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into();
        }
        self.router.tools()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        let args: Value =
            serde_json::from_str(call.args.get()).map_err(|e| ToolError::InvalidArguments {
                name: call.name.to_string(),
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
        tokio::time::timeout(self.default_timeout, self.router.dispatch(call))
            .await
            .map_err(|_| ToolError::timeout(call.name, self.default_timeout.as_millis() as u64))?
    }

    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        self.router.external_tool_surface_snapshot()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.router.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        if self.router.tool_catalog_capabilities().exact_catalog {
            return self.router.tool_catalog();
        }
        self.tools()
            .iter()
            .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true))
            .collect::<Vec<_>>()
            .into()
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.router.pending_catalog_sources()
    }
}

/// A dispatcher wrapper that filters tools based on a ToolAccessPolicy.
///
/// This is used to restrict which tools a delegated branch can access
/// with an allow/deny list configuration.
pub struct FilteredDispatcher {
    inner: Arc<dyn AgentToolDispatcher>,
    policy: ToolAccessPolicy,
}

impl FilteredDispatcher {
    /// Create a new filtered dispatcher by applying the given policy to the inner dispatcher.
    pub fn new(inner: Arc<dyn AgentToolDispatcher>, policy: &ToolAccessPolicy) -> Self {
        Self {
            inner,
            policy: policy.clone(),
        }
    }

    /// Check if a tool name is allowed by the policy.
    pub fn is_allowed(&self, name: &str) -> bool {
        match &self.policy {
            ToolAccessPolicy::Inherit => true,
            ToolAccessPolicy::AllowList(allow) => allow.iter().any(|n| n.as_str() == name),
            ToolAccessPolicy::DenyList(deny) => !deny.iter().any(|n| n.as_str() == name),
        }
    }

    /// Get the set of allowed tool names.
    pub fn allowed_names(&self) -> HashSet<String> {
        if self.inner.tool_catalog_capabilities().exact_catalog {
            self.inner
                .tool_catalog()
                .iter()
                .map(|entry| entry.tool.name.clone())
                .filter(|name| self.is_allowed(name))
                .collect()
        } else {
            self.inner
                .tools()
                .iter()
                .map(|t| t.name.clone())
                .filter(|name| self.is_allowed(name))
                .collect()
        }
    }

    fn inner_knows_tool(&self, name: &str) -> bool {
        if self.inner.tool_catalog_capabilities().exact_catalog {
            self.inner
                .tool_catalog()
                .iter()
                .any(|entry| entry.tool.name == name)
        } else {
            self.inner.tools().iter().any(|t| t.name == name)
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for FilteredDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        if self.inner.tool_catalog_capabilities().exact_catalog {
            return self
                .inner
                .tool_catalog()
                .iter()
                .filter(|entry| entry.currently_callable())
                .map(|entry| Arc::clone(&entry.tool))
                .filter(|tool| self.is_allowed(&tool.name))
                .collect::<Vec<_>>()
                .into();
        }
        self.inner
            .tools()
            .iter()
            .filter(|t| self.is_allowed(&t.name))
            .cloned()
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        // Wave B (V7): policy-denied calls surface as `AccessDenied`, not
        // `NotFound`. A tool that the inner dispatcher does not know about
        // still returns `NotFound` from the inner dispatcher itself — this
        // wrapper only intervenes when the tool is visible upstream but
        // excluded by the active policy.
        if !self.is_allowed(call.name) {
            if self.inner_knows_tool(call.name) {
                return Err(ToolError::AccessDenied {
                    name: call.name.to_string(),
                });
            }
            return Err(ToolError::NotFound {
                name: call.name.to_string(),
            });
        }
        self.inner.dispatch(call).await
    }

    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        self.inner.external_tool_surface_snapshot()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        if !self.inner.tool_catalog_capabilities().exact_catalog {
            return self
                .tools()
                .iter()
                .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true))
                .collect::<Vec<_>>()
                .into();
        }
        self.inner
            .tool_catalog()
            .iter()
            .filter(|entry| self.is_allowed(entry.tool.name.as_str()))
            .cloned()
            .collect::<Vec<_>>()
            .into()
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// A mock dispatcher with multiple tools for testing filtering
    struct MockDispatcher {
        tool_names: Vec<&'static str>,
    }

    impl MockDispatcher {
        fn new(names: Vec<&'static str>) -> Self {
            Self { tool_names: names }
        }
    }

    struct MutableDispatcher {
        tool_names: Mutex<Vec<&'static str>>,
    }

    impl MutableDispatcher {
        fn new(names: Vec<&'static str>) -> Self {
            Self {
                tool_names: Mutex::new(names),
            }
        }

        fn add_tool(&self, name: &'static str) {
            self.tool_names.lock().unwrap().push(name);
        }
    }

    struct ExactMockDispatcher {
        catalog: Arc<[ToolCatalogEntry]>,
    }

    impl ExactMockDispatcher {
        fn new(entries: &[(&str, bool)]) -> Self {
            let catalog = entries
                .iter()
                .map(|(name, callable)| {
                    ToolCatalogEntry::session_inline(
                        Arc::new(ToolDef {
                            name: (*name).to_string(),
                            description: format!("{name} tool"),
                            input_schema: json!({"type": "object"}),
                            provenance: None,
                        }),
                        *callable,
                    )
                })
                .collect::<Vec<_>>()
                .into();
            Self { catalog }
        }
    }

    fn make_call<'a>(name: &'a str, args_raw: &'a serde_json::value::RawValue) -> ToolCallView<'a> {
        ToolCallView {
            id: "test-1",
            name,
            args: args_raw,
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for MockDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.tool_names
                .iter()
                .map(|name| {
                    Arc::new(ToolDef {
                        name: (*name).to_string(),
                        description: format!("{name} tool"),
                        input_schema: json!({"type": "object"}),
                        provenance: None,
                    })
                })
                .collect::<Vec<_>>()
                .into()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            if self.tool_names.contains(&call.name) {
                Ok(ToolResult::new(
                    call.id.to_string(),
                    json!({"called": call.name}).to_string(),
                    false,
                )
                .into())
            } else {
                Err(ToolError::NotFound {
                    name: call.name.to_string(),
                })
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for MutableDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.tool_names
                .lock()
                .unwrap()
                .iter()
                .map(|name| {
                    Arc::new(ToolDef {
                        name: (*name).to_string(),
                        description: format!("{name} tool"),
                        input_schema: json!({"type": "object"}),
                        provenance: None,
                    })
                })
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
                    name: call.name.to_string(),
                })
            }
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
                    name: call.name.to_string(),
                });
            };
            if let Some(reason) = entry.callability.unavailable_reason() {
                return Err(ToolError::unavailable(call.name, reason.to_string()));
            }
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"called": call.name}).to_string(),
                false,
            )
            .into())
        }
    }

    #[test]
    fn filtered_dispatcher_is_not_exact_catalog_capable() {
        let inner: Arc<dyn AgentToolDispatcher> = Arc::new(MockDispatcher::new(vec!["a", "b"]));
        let filtered =
            FilteredDispatcher::new(inner, &ToolAccessPolicy::AllowList(vec!["a".into()]));

        assert!(
            !filtered.tool_catalog_capabilities().exact_catalog,
            "cached filtered dispatcher must stay on the non-exact path"
        );
    }

    #[tokio::test]
    async fn filtered_dispatcher_preserves_exact_catalog_callability() {
        let inner: Arc<dyn AgentToolDispatcher> = Arc::new(ExactMockDispatcher::new(&[
            ("allowed_hidden", false),
            ("denied_hidden", false),
        ]));
        let filtered = FilteredDispatcher::new(
            inner,
            &ToolAccessPolicy::AllowList(vec!["allowed_hidden".into()]),
        );

        assert!(filtered.tool_catalog_capabilities().exact_catalog);
        assert!(filtered.tools().is_empty());
        let catalog = filtered.tool_catalog();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].tool.name, "allowed_hidden");
        assert!(!catalog[0].currently_callable());

        let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
        let allowed_result = filtered
            .dispatch(make_call("allowed_hidden", &args_raw))
            .await;
        assert!(matches!(allowed_result, Err(ToolError::Unavailable { .. })));

        let denied_result = filtered
            .dispatch(make_call("denied_hidden", &args_raw))
            .await;
        assert!(matches!(denied_result, Err(ToolError::AccessDenied { .. })));
    }

    #[tokio::test]
    async fn dispatcher_uses_live_router_tools_for_listing_and_validation() {
        let router = Arc::new(MutableDispatcher::new(vec!["initial"]));
        let dispatcher = ToolDispatcher::new(router.clone());

        let initial_names: Vec<_> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        assert_eq!(initial_names, vec!["initial".to_string()]);

        router.add_tool("late");

        let live_names: Vec<_> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        assert_eq!(
            live_names,
            vec!["initial".to_string(), "late".to_string()],
            "dispatcher must not freeze the router's tool list at build time"
        );

        let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
        let result = dispatcher.dispatch(make_call("late", &args_raw)).await;
        assert!(
            result.is_ok(),
            "late-added router tool must validate and dispatch through the same live source"
        );
    }

    #[tokio::test]
    async fn filtered_dispatcher_evaluates_policy_against_live_inner_tools() {
        let inner = Arc::new(MutableDispatcher::new(vec!["initial"]));
        let filtered = FilteredDispatcher::new(inner.clone(), &ToolAccessPolicy::Inherit);

        let initial_names: Vec<_> = filtered
            .tools()
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        assert_eq!(initial_names, vec!["initial".to_string()]);

        inner.add_tool("late");

        let live_names: Vec<_> = filtered
            .tools()
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        assert_eq!(
            live_names,
            vec!["initial".to_string(), "late".to_string()],
            "inherited policy must follow the live inner tool set"
        );

        let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
        let result = filtered.dispatch(make_call("late", &args_raw)).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_filtered_dispatcher_inherit_passes_all_tools() {
        let inner = Arc::new(MockDispatcher::new(vec!["shell", "task_list", "wait"]));
        let filtered = FilteredDispatcher::new(inner, &ToolAccessPolicy::Inherit);

        let tool_names: Vec<_> = filtered.tools().iter().map(|t| t.name.clone()).collect();
        assert_eq!(tool_names.len(), 3);
        assert!(filtered.is_allowed("shell"));
        assert!(filtered.is_allowed("task_list"));
        assert!(filtered.is_allowed("wait"));
    }

    #[test]
    fn test_filtered_dispatcher_allow_list_only_includes_specified_tools() {
        let inner = Arc::new(MockDispatcher::new(vec!["shell", "task_list", "wait"]));
        let policy = ToolAccessPolicy::AllowList(vec!["task_list".into()]);
        let filtered = FilteredDispatcher::new(inner, &policy);

        let tool_names: Vec<_> = filtered.tools().iter().map(|t| t.name.clone()).collect();
        assert_eq!(tool_names.len(), 1);
        assert_eq!(tool_names[0], "task_list");
        assert!(!filtered.is_allowed("shell"));
        assert!(filtered.is_allowed("task_list"));
        assert!(!filtered.is_allowed("wait"));
    }

    #[test]
    fn test_filtered_dispatcher_deny_list_excludes_specified_tools() {
        let inner = Arc::new(MockDispatcher::new(vec!["shell", "task_list", "wait"]));
        let policy = ToolAccessPolicy::DenyList(vec!["shell".into()]);
        let filtered = FilteredDispatcher::new(inner, &policy);

        let tool_names: Vec<_> = filtered.tools().iter().map(|t| t.name.clone()).collect();
        assert_eq!(tool_names.len(), 2);
        assert!(!filtered.is_allowed("shell"));
        assert!(filtered.is_allowed("task_list"));
        assert!(filtered.is_allowed("wait"));
    }

    #[tokio::test]
    async fn test_filtered_dispatcher_blocked_tool_returns_access_denied() {
        let inner = Arc::new(MockDispatcher::new(vec!["shell", "task_list"]));
        let policy = ToolAccessPolicy::DenyList(vec!["shell".into()]);
        let filtered = FilteredDispatcher::new(inner, &policy);

        // Allowed tool succeeds.
        let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
        let result = filtered.dispatch(make_call("task_list", &args_raw)).await;
        assert!(result.is_ok());

        // Policy-blocked tool returns `AccessDenied` — distinct from
        // `NotFound` which is reserved for genuinely-missing tools.
        let result = filtered.dispatch(make_call("shell", &args_raw)).await;
        match result {
            Err(ToolError::AccessDenied { name }) => assert_eq!(name, "shell"),
            other => panic!("Expected AccessDenied error, got: {other:?}"),
        }

        // A genuinely-missing tool still surfaces as `NotFound`.
        let result = filtered.dispatch(make_call("ghost", &args_raw)).await;
        match result {
            Err(ToolError::NotFound { name }) => assert_eq!(name, "ghost"),
            other => panic!("Expected NotFound error, got: {other:?}"),
        }
    }

    /// Regression test: Ensure tool access policy is actually enforced.
    #[tokio::test]
    async fn test_regression_tool_access_policy_must_be_enforced() {
        let inner = Arc::new(MockDispatcher::new(vec![
            "shell",
            "dangerous_exec",
            "task_list",
            "wait",
        ]));

        // Deny shell and dangerous_exec - common security restriction
        let policy = ToolAccessPolicy::DenyList(vec!["shell".into(), "dangerous_exec".into()]);
        let filtered = FilteredDispatcher::new(inner, &policy);

        // Only safe tools should be visible
        let visible_tools: Vec<_> = filtered.tools().iter().map(|t| t.name.clone()).collect();
        assert_eq!(visible_tools.len(), 2);
        assert!(visible_tools.contains(&"task_list".to_string()));
        assert!(visible_tools.contains(&"wait".to_string()));

        // Denied tools should NOT be visible or dispatchable
        assert!(
            !visible_tools.contains(&"shell".to_string()),
            "shell should not be visible in tools list"
        );
        assert!(
            !visible_tools.contains(&"dangerous_exec".to_string()),
            "dangerous_exec should not be visible in tools list"
        );

        // Attempting to dispatch denied tools should fail with `AccessDenied`.
        let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
        let shell_result = filtered.dispatch(make_call("shell", &args_raw)).await;
        assert!(
            matches!(shell_result, Err(ToolError::AccessDenied { .. })),
            "shell dispatch should fail with AccessDenied"
        );
    }
}
