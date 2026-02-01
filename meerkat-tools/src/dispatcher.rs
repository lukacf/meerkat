//! Tool dispatcher implementation

use crate::error::DispatchError;
use crate::registry::ToolRegistry;
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::{ToolError, ToolValidationError};
use meerkat_core::ops::ToolAccessPolicy;
use meerkat_core::types::{ToolCall, ToolDef};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

/// An empty tool dispatcher that has no tools and always returns NotFound
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for EmptyToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
        Err(ToolError::NotFound {
            name: name.to_string(),
        })
    }
}

/// A high-level tool dispatcher that validates arguments and handles timeouts
pub struct ToolDispatcher {
    registry: ToolRegistry,
    router: Arc<dyn AgentToolDispatcher>,
    default_timeout: Duration,
}

impl ToolDispatcher {
    /// Create a new tool dispatcher
    pub fn new(registry: ToolRegistry, router: Arc<dyn AgentToolDispatcher>) -> Self {
        Self {
            registry,
            router,
            default_timeout: Duration::from_secs(30),
        }
    }

    /// Set the default timeout for tool execution
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Dispatch a tool call
    pub async fn dispatch_call(&self, call: &ToolCall) -> Result<Value, DispatchError> {
        // 1. Validate arguments against schema
        self.registry.validate(&call.name, &call.args)?;

        // 2. Dispatch to router with timeout
        let result = tokio::time::timeout(
            self.default_timeout,
            self.router.dispatch(&call.name, &call.args),
        )
        .await
        .map_err(|_| DispatchError::Timeout {
            timeout_ms: self.default_timeout.as_millis() as u64,
        })??;

        Ok(result)
    }
}

#[async_trait]
impl AgentToolDispatcher for ToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from(self.registry.list())
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        // Validate arguments against schema
        self.registry.validate(name, args).map_err(|e| match e {
            ToolValidationError::NotFound { name } => ToolError::NotFound { name },
            ToolValidationError::InvalidArguments { name, reason } => {
                ToolError::InvalidArguments { name, reason }
            }
        })?;

        // Dispatch with timeout to prevent hanging tool calls
        tokio::time::timeout(self.default_timeout, self.router.dispatch(name, args))
            .await
            .map_err(|_| ToolError::timeout(name, self.default_timeout.as_millis() as u64))?
    }
}

/// A dispatcher wrapper that filters tools based on a ToolAccessPolicy.
///
/// This is used to restrict which tools a sub-agent can access when spawned
/// with an allow/deny list configuration.
pub struct FilteredDispatcher {
    inner: Arc<dyn AgentToolDispatcher>,
    allowed_names: HashSet<String>,
}

impl FilteredDispatcher {
    /// Create a new filtered dispatcher by applying the given policy to the inner dispatcher.
    pub fn new(inner: Arc<dyn AgentToolDispatcher>, policy: &ToolAccessPolicy) -> Self {
        let all_names: HashSet<String> = inner.tools().iter().map(|t| t.name.clone()).collect();

        let allowed_names = match policy {
            ToolAccessPolicy::Inherit => all_names,
            ToolAccessPolicy::AllowList(allow) => {
                let allow_set: HashSet<&str> = allow.iter().map(|s| s.as_str()).collect();
                all_names
                    .into_iter()
                    .filter(|n| allow_set.contains(n.as_str()))
                    .collect()
            }
            ToolAccessPolicy::DenyList(deny) => {
                let deny_set: HashSet<&str> = deny.iter().map(|s| s.as_str()).collect();
                all_names
                    .into_iter()
                    .filter(|n| !deny_set.contains(n.as_str()))
                    .collect()
            }
        };

        Self {
            inner,
            allowed_names,
        }
    }

    /// Check if a tool name is allowed by the policy.
    pub fn is_allowed(&self, name: &str) -> bool {
        self.allowed_names.contains(name)
    }

    /// Get the set of allowed tool names.
    pub fn allowed_names(&self) -> &HashSet<String> {
        &self.allowed_names
    }
}

#[async_trait]
impl AgentToolDispatcher for FilteredDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner
            .tools()
            .iter()
            .filter(|t| self.allowed_names.contains(&t.name))
            .cloned()
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        if !self.allowed_names.contains(name) {
            return Err(ToolError::NotFound {
                name: name.to_string(),
            });
        }
        self.inner.dispatch(name, args).await
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A mock dispatcher with multiple tools for testing filtering
    struct MockDispatcher {
        tool_names: Vec<&'static str>,
    }

    impl MockDispatcher {
        fn new(names: Vec<&'static str>) -> Self {
            Self { tool_names: names }
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
                        description: format!("{} tool", name),
                        input_schema: json!({"type": "object"}),
                    })
                })
                .collect::<Vec<_>>()
                .into()
        }

        async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
            if self.tool_names.contains(&name) {
                Ok(json!({"called": name}))
            } else {
                Err(ToolError::NotFound {
                    name: name.to_string(),
                })
            }
        }
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
        let policy = ToolAccessPolicy::AllowList(vec!["task_list".to_string()]);
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
        let policy = ToolAccessPolicy::DenyList(vec!["shell".to_string()]);
        let filtered = FilteredDispatcher::new(inner, &policy);

        let tool_names: Vec<_> = filtered.tools().iter().map(|t| t.name.clone()).collect();
        assert_eq!(tool_names.len(), 2);
        assert!(!filtered.is_allowed("shell"));
        assert!(filtered.is_allowed("task_list"));
        assert!(filtered.is_allowed("wait"));
    }

    #[tokio::test]
    async fn test_filtered_dispatcher_dispatch_blocked_tool_returns_not_found() {
        let inner = Arc::new(MockDispatcher::new(vec!["shell", "task_list"]));
        let policy = ToolAccessPolicy::DenyList(vec!["shell".to_string()]);
        let filtered = FilteredDispatcher::new(inner, &policy);

        // Allowed tool succeeds
        let result = filtered.dispatch("task_list", &json!({})).await;
        assert!(result.is_ok());

        // Blocked tool returns NotFound
        let result = filtered.dispatch("shell", &json!({})).await;
        match result {
            Err(ToolError::NotFound { name }) => assert_eq!(name, "shell"),
            other => panic!("Expected NotFound error, got: {:?}", other),
        }
    }

    /// Regression test: Ensure tool access policy is actually enforced
    /// Previously, _tool_access was parsed but never applied, so sub-agents
    /// could access all tools regardless of allow/deny configuration.
    #[tokio::test]
    async fn test_regression_tool_access_policy_must_be_enforced() {
        let inner = Arc::new(MockDispatcher::new(vec![
            "shell",
            "agent_spawn",
            "task_list",
            "wait",
        ]));

        // Deny shell and agent_spawn - common security restriction
        let policy =
            ToolAccessPolicy::DenyList(vec!["shell".to_string(), "agent_spawn".to_string()]);
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
            !visible_tools.contains(&"agent_spawn".to_string()),
            "agent_spawn should not be visible in tools list"
        );

        // Attempting to dispatch denied tools should fail
        let shell_result = filtered.dispatch("shell", &json!({})).await;
        assert!(
            matches!(shell_result, Err(ToolError::NotFound { .. })),
            "shell dispatch should fail with NotFound"
        );
    }
}
