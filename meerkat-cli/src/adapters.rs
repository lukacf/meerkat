//! Adapters to bridge existing crate types to agent traits

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_client::{LlmClient, LlmEvent, LlmRequest};
use meerkat_core::{
    AgentError, Message, Session, SessionId, StopReason, ToolCall, ToolDef, Usage,
    agent::{AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult},
    error::ToolError,
    event::AgentEvent,
};
use meerkat_store::SessionStore;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Dynamic dispatch adapter for runtime LLM provider selection
pub struct DynLlmClientAdapter {
    client: Arc<dyn LlmClient>,
    model: String,
    /// Optional channel to emit streaming events (text deltas)
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    /// Provider-specific parameters to pass with every request
    provider_params: Option<Value>,
}

impl DynLlmClientAdapter {
    pub fn new(client: Arc<dyn LlmClient>, model: String) -> Self {
        Self {
            client,
            model,
            event_tx: None,
            provider_params: None,
        }
    }

    /// Create an adapter with streaming event support.
    /// Text deltas will be sent to the provided channel as they arrive.
    pub fn with_event_channel(
        client: Arc<dyn LlmClient>,
        model: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Self {
        Self {
            client,
            model,
            event_tx: Some(event_tx),
            provider_params: None,
        }
    }

    /// Set provider-specific parameters to pass with every request
    pub fn with_provider_params(mut self, params: Option<Value>) -> Self {
        self.provider_params = params;
        self
    }
}

#[async_trait]
impl AgentLlmClient for DynLlmClientAdapter {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&Value>,
    ) -> Result<LlmStreamResult, AgentError> {
        // Use provider_params from call-site if provided, otherwise use adapter-level defaults
        let effective_params = provider_params
            .cloned()
            .or_else(|| self.provider_params.clone());

        // Build request
        let request = LlmRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            max_tokens,
            temperature,
            stop_sequences: None,
            provider_params: effective_params,
        };

        // Get stream
        let mut stream = self.client.stream(&request);

        // Accumulate response
        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tool_call_buffers: HashMap<String, ToolCallBuffer> = HashMap::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = Usage::default();

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => match event {
                    LlmEvent::TextDelta { delta } => {
                        content.push_str(&delta);
                        // Emit streaming event if channel is configured
                        if let Some(ref tx) = self.event_tx {
                            let _ = tx
                                .send(AgentEvent::TextDelta {
                                    delta: delta.clone(),
                                })
                                .await;
                        }
                    }
                    LlmEvent::ToolCallDelta {
                        id,
                        name,
                        args_delta,
                    } => {
                        let buffer = tool_call_buffers
                            .entry(id.clone())
                            .or_insert_with(|| ToolCallBuffer::new(id));

                        if let Some(n) = name {
                            buffer.name = Some(n);
                        }
                        buffer.args_json.push_str(&args_delta);
                    }
                    LlmEvent::ToolCallComplete { id, name, args, .. } => {
                        tool_calls.push(ToolCall::new(id, name, args));
                    }
                    LlmEvent::UsageUpdate { usage: u } => {
                        usage = u;
                    }
                    LlmEvent::Done { stop_reason: sr } => {
                        stop_reason = sr;
                    }
                },
                Err(e) => {
                    return Err(AgentError::LlmError(e.to_string()));
                }
            }
        }

        // Complete any buffered tool calls that weren't explicitly completed
        for (_, buffer) in tool_call_buffers {
            if let Some(tc) = buffer.try_complete() {
                // Only add if not already in tool_calls
                if !tool_calls.iter().any(|t| t.id == tc.id) {
                    tool_calls.push(tc);
                }
            }
        }

        Ok(LlmStreamResult {
            content,
            tool_calls,
            stop_reason,
            usage,
        })
    }

    fn provider(&self) -> &'static str {
        self.client.provider()
    }
}

/// Helper for accumulating tool call deltas
#[derive(Debug, Default)]
struct ToolCallBuffer {
    id: String,
    name: Option<String>,
    args_json: String,
}

impl ToolCallBuffer {
    fn new(id: String) -> Self {
        Self {
            id,
            name: None,
            args_json: String::new(),
        }
    }

    fn try_complete(&self) -> Option<ToolCall> {
        let name = self.name.as_ref()?;
        let args: Value = serde_json::from_str(&self.args_json).ok()?;
        Some(ToolCall::new(self.id.clone(), name.clone(), args))
    }
}

/// Adapter that wraps a SessionStore to implement AgentSessionStore
pub struct SessionStoreAdapter<S: SessionStore> {
    store: Arc<S>,
}

impl<S: SessionStore> SessionStoreAdapter<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<S: SessionStore + 'static> AgentSessionStore for SessionStoreAdapter<S> {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.store
            .save(session)
            .await
            .map_err(|e| AgentError::StoreError(e.to_string()))
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        // Parse the string ID to SessionId
        let session_id = SessionId::parse(id)
            .map_err(|e| AgentError::StoreError(format!("Invalid session ID: {}", e)))?;

        self.store
            .load(&session_id)
            .await
            .map_err(|e| AgentError::StoreError(e.to_string()))
    }
}

/// Empty tool dispatcher for when no tools are configured
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

use meerkat_mcp_client::McpRouter;
use meerkat_tools::builtin::CompositeDispatcher;
use tokio::sync::RwLock;

/// Adapter that wraps an McpRouter to implement AgentToolDispatcher
pub struct McpRouterAdapter {
    router: RwLock<Option<McpRouter>>,
    cached_tools: RwLock<Vec<ToolDef>>,
}

impl McpRouterAdapter {
    pub fn new(router: McpRouter) -> Self {
        Self {
            router: RwLock::new(Some(router)),
            cached_tools: RwLock::new(Vec::new()),
        }
    }

    /// Refresh the cached tool list from the router
    ///
    /// Note: list_tools() is now synchronous since tools are cached in the router.
    pub async fn refresh_tools(&self) -> Result<(), String> {
        let router = self.router.read().await;
        if let Some(router) = router.as_ref() {
            let tools = router.list_tools().to_vec();
            let mut cached = self.cached_tools.write().await;
            *cached = tools;
        }
        Ok(())
    }

    /// Gracefully shutdown the MCP router
    ///
    /// Takes the router out of the adapter and shuts it down.
    /// After this call, tool calls will fail.
    pub async fn shutdown(&self) {
        let mut router = self.router.write().await;
        if let Some(router) = router.take() {
            router.shutdown().await;
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for McpRouterAdapter {
    fn tools(&self) -> Vec<ToolDef> {
        // Return the cached tools (blocking read in sync context)
        // Note: This requires refresh_tools() to be called before first use
        // We use try_read to avoid deadlocks, falling back to empty vec
        match self.cached_tools.try_read() {
            Ok(tools) => tools.clone(),
            Err(_) => Vec::new(),
        }
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        let guard = self.router.read().await;
        match &*guard {
            Some(router) => {
                let result = router
                    .call_tool(name, args)
                    .await
                    .map_err(|e| ToolError::execution_failed(e.to_string()))?;
                // Parse the result string as JSON, or wrap in a string value
                #[allow(clippy::unnecessary_lazy_evaluations)]
                let value = serde_json::from_str(&result).unwrap_or_else(|_| Value::String(result));
                Ok(value)
            }
            None => Err(ToolError::execution_failed("MCP router has been shut down")),
        }
    }
}

/// Combined tool dispatcher that can be empty, MCP-backed, or composite (with builtins)
pub enum CliToolDispatcher {
    Empty(EmptyToolDispatcher),
    Mcp(Box<McpRouterAdapter>),
    Composite(std::sync::Arc<CompositeDispatcher>),
}

impl CliToolDispatcher {
    /// Gracefully shutdown MCP connections (no-op for Empty and Composite)
    pub async fn shutdown(&self) {
        match self {
            CliToolDispatcher::Empty(_) => {}
            CliToolDispatcher::Mcp(adapter) => adapter.shutdown().await,
            CliToolDispatcher::Composite(_) => {
                // CompositeDispatcher doesn't have external connections to shut down
            }
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for CliToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        match self {
            CliToolDispatcher::Empty(d) => d.tools(),
            CliToolDispatcher::Mcp(d) => d.tools(),
            CliToolDispatcher::Composite(d) => d.tools(),
        }
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        match self {
            CliToolDispatcher::Empty(d) => d.dispatch(name, args).await,
            CliToolDispatcher::Mcp(d) => d.dispatch(name, args).await,
            CliToolDispatcher::Composite(d) => d.dispatch(name, args).await,
        }
    }
}
