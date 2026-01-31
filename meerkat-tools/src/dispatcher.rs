//! Tool dispatcher implementation

use crate::error::DispatchError;
use crate::registry::ToolRegistry;
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::{ToolError, ToolValidationError};
use meerkat_core::types::{ToolCall, ToolDef};
use serde_json::Value;
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
            .map_err(|_| ToolError::ExecutionFailed {
                message: format!(
                    "Tool '{}' timed out after {}ms",
                    name,
                    self.default_timeout.as_millis()
                ),
            })?
    }
}
