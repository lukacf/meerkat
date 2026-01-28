//! Tool dispatch with timeouts

use crate::ToolError;
use crate::error::DispatchError;
use crate::registry::ToolRegistry;
use async_trait::async_trait;
use meerkat_core::{AgentToolDispatcher, ToolCall, ToolDef, ToolResult};
use meerkat_mcp_client::McpRouter;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

/// Dispatcher configuration for selecting the shared dispatcher flavor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ToolDispatcherConfig {
    pub kind: ToolDispatcherKind,
}

/// Supported dispatcher kinds.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolDispatcherKind {
    Empty,
    Mcp,
    Composite,
    WithComms,
}

impl Default for ToolDispatcherConfig {
    fn default() -> Self {
        Self {
            kind: ToolDispatcherKind::Composite,
        }
    }
}

/// Empty dispatcher with no tools.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for EmptyToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        Vec::new()
    }

    async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
        Err(ToolError::not_found(name))
    }
}

/// Dispatcher for tool calls with timeout support
pub struct ToolDispatcher {
    registry: ToolRegistry,
    router: Arc<McpRouter>,
    default_timeout: Duration,
}

impl ToolDispatcher {
    /// Create a new dispatcher
    pub fn new(router: Arc<McpRouter>, default_timeout: Duration) -> Self {
        Self {
            registry: ToolRegistry::new(),
            router,
            default_timeout,
        }
    }

    /// Discover tools from MCP servers (caches tools from router)
    ///
    /// This method registers tools from the router's cache. Tools are cached
    /// in the router when servers are added, so this is now synchronous.
    pub fn discover_tools(&mut self) {
        for tool in self.router.list_tools() {
            self.registry.register(tool.clone());
        }
    }

    /// Get tool definitions for LLM requests.
    /// Returns Arc references to avoid cloning ToolDef on each call.
    pub fn tool_defs_arc(&self) -> Vec<Arc<ToolDef>> {
        self.registry.tool_defs()
    }

    /// Get tool definitions for LLM requests (cloned for trait compatibility).
    pub fn tool_defs(&self) -> Vec<ToolDef> {
        self.registry
            .tool_defs()
            .into_iter()
            .map(|arc| (*arc).clone())
            .collect()
    }

    /// Dispatch a single tool call
    pub async fn dispatch_one(&self, call: &ToolCall) -> ToolResult {
        match self.dispatch_one_inner(call).await {
            Ok(result) => result,
            Err(e) => ToolResult::from_tool_call(call, e.to_string(), true),
        }
    }

    async fn dispatch_one_inner(&self, call: &ToolCall) -> Result<ToolResult, DispatchError> {
        // Validate arguments
        self.registry.validate(&call.name, &call.args)?;

        // Dispatch via MCP router with timeout
        let result = tokio::time::timeout(
            self.default_timeout,
            self.router.call_tool(&call.name, &call.args),
        )
        .await
        .map_err(|_| DispatchError::Timeout {
            tool: call.name.clone(),
            timeout_ms: self.default_timeout.as_millis() as u64,
        })??;

        Ok(ToolResult::from_tool_call(call, result, false))
    }

    /// Dispatch multiple tool calls in parallel
    pub async fn dispatch_parallel(&self, calls: &[ToolCall]) -> Vec<ToolResult> {
        let futures = calls.iter().map(|call| self.dispatch_one(call));
        futures::future::join_all(futures).await
    }
}

#[async_trait]
impl AgentToolDispatcher for ToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        self.tool_defs()
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        // Validate arguments against schema
        self.registry
            .validate(name, args)
            .map_err(|e| ToolError::invalid_arguments(name.to_string(), e.to_string()))?;

        // Dispatch via MCP router with timeout
        let result = tokio::time::timeout(self.default_timeout, self.router.call_tool(name, args))
            .await
            .map_err(|_| {
                ToolError::timeout(name.to_string(), self.default_timeout.as_millis() as u64)
            })?
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;

        // Parse the result string as JSON, or wrap in a string value
        #[allow(clippy::unnecessary_lazy_evaluations)]
        let value = serde_json::from_str(&result).unwrap_or_else(|_| Value::String(result));
        Ok(value)
    }
}

/// A tool dispatcher that wraps another dispatcher and filters out specific tools.
///
/// This is useful for sub-agents that should inherit most tools from the parent
/// but not have access to certain tools (e.g., sub-agent tools to prevent infinite nesting).
pub struct FilteredToolDispatcher {
    inner: Arc<dyn AgentToolDispatcher>,
    /// Tools to hide (deny list)
    denied_tools: HashSet<String>,
}

impl FilteredToolDispatcher {
    /// Create a new filtered dispatcher that hides the specified tools.
    pub fn new(inner: Arc<dyn AgentToolDispatcher>, denied_tools: HashSet<String>) -> Self {
        Self {
            inner,
            denied_tools,
        }
    }

    /// Create a filtered dispatcher that denies sub-agent tools.
    ///
    /// This is the standard configuration for sub-agents to prevent infinite nesting.
    pub fn deny_sub_agent_tools(inner: Arc<dyn AgentToolDispatcher>) -> Self {
        let denied = [
            "agent_spawn",
            "agent_fork",
            "agent_status",
            "agent_cancel",
            "agent_list",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        Self::new(inner, denied)
    }
}

#[async_trait]
impl AgentToolDispatcher for FilteredToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        self.inner
            .tools()
            .into_iter()
            .filter(|t| !self.denied_tools.contains(&t.name))
            .collect()
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        if self.denied_tools.contains(name) {
            return Err(ToolError::not_found(name));
        }
        self.inner.dispatch(name, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::AgentToolDispatcher;

    fn create_test_dispatcher() -> ToolDispatcher {
        let router = Arc::new(McpRouter::new());
        ToolDispatcher::new(router, Duration::from_secs(30))
    }

    #[test]
    fn test_tools_returns_empty_vec_for_new_dispatcher() {
        let dispatcher = create_test_dispatcher();
        // Use the AgentToolDispatcher trait method
        let tools: Vec<ToolDef> = AgentToolDispatcher::tools(&dispatcher);
        assert!(tools.is_empty());
    }

    #[test]
    fn test_tools_returns_registered_tools() {
        let router = Arc::new(McpRouter::new());
        let mut dispatcher = ToolDispatcher::new(router, Duration::from_secs(30));

        // Register a tool directly via the registry
        dispatcher.registry.register(ToolDef {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {"type": "string"}
                },
                "required": []
            }),
        });

        // Use the AgentToolDispatcher trait method
        let tools = AgentToolDispatcher::tools(&dispatcher);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "test_tool");
    }

    #[tokio::test]
    async fn test_dispatch_validates_args() {
        let router = Arc::new(McpRouter::new());
        let mut dispatcher = ToolDispatcher::new(router, Duration::from_secs(30));

        dispatcher.registry.register(ToolDef {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "count": {"type": "integer"}
                },
                "required": ["count"]
            }),
        });

        // Invalid args (missing required field) should fail validation
        let result =
            AgentToolDispatcher::dispatch(&dispatcher, "test_tool", &serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("count"));
    }

    #[tokio::test]
    async fn test_dispatch_unknown_tool() {
        let dispatcher = create_test_dispatcher();

        // Unknown tool should fail
        let result =
            AgentToolDispatcher::dispatch(&dispatcher, "unknown_tool", &serde_json::json!({}))
                .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
