//! Adapters to bridge existing crate types to agent traits

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_client::{LlmClient, LlmEvent, LlmRequest};
use meerkat_core::{
    agent::{AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult},
    AgentError, Message, Session, SessionId, StopReason, ToolCall, ToolDef, Usage,
};
use meerkat_store::SessionStore;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Adapter that wraps an LlmClient to implement AgentLlmClient
pub struct LlmClientAdapter<C: LlmClient> {
    client: Arc<C>,
    model: String,
}

impl<C: LlmClient> LlmClientAdapter<C> {
    pub fn new(client: Arc<C>, model: String) -> Self {
        Self { client, model }
    }
}

/// Dynamic dispatch version of LlmClientAdapter for runtime provider selection
pub struct DynLlmClientAdapter {
    client: Arc<dyn LlmClient>,
    model: String,
}

impl DynLlmClientAdapter {
    pub fn new(client: Arc<dyn LlmClient>, model: String) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl<C: LlmClient + 'static> AgentLlmClient for LlmClientAdapter<C> {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
    ) -> Result<LlmStreamResult, AgentError> {
        // Build request
        let request = LlmRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            max_tokens,
            temperature: None,
            stop_sequences: None,
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
                    LlmEvent::ToolCallComplete { id, name, args } => {
                        tool_calls.push(ToolCall { id, name, args });
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

#[async_trait]
impl AgentLlmClient for DynLlmClientAdapter {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
    ) -> Result<LlmStreamResult, AgentError> {
        // Build request
        let request = LlmRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            max_tokens,
            temperature: None,
            stop_sequences: None,
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
                    LlmEvent::ToolCallComplete { id, name, args } => {
                        tool_calls.push(ToolCall { id, name, args });
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
        Some(ToolCall {
            id: self.id.clone(),
            name: name.clone(),
            args,
        })
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

    async fn dispatch(&self, name: &str, _args: &Value) -> Result<String, String> {
        Err(format!("Unknown tool: {}", name))
    }
}

use meerkat_mcp_client::McpRouter;
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
    pub async fn refresh_tools(&self) -> Result<(), String> {
        let router = self.router.read().await;
        if let Some(router) = router.as_ref() {
            let tools = router.list_tools().await.map_err(|e| e.to_string())?;
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

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        let guard = self.router.read().await;
        match &*guard {
            Some(router) => router.call_tool(name, args).await.map_err(|e| e.to_string()),
            None => Err("MCP router has been shut down".to_string()),
        }
    }
}

/// Combined tool dispatcher that can be either empty or MCP-backed
pub enum CliToolDispatcher {
    Empty(EmptyToolDispatcher),
    Mcp(McpRouterAdapter),
}

impl CliToolDispatcher {
    /// Gracefully shutdown MCP connections (no-op for Empty)
    pub async fn shutdown(&self) {
        match self {
            CliToolDispatcher::Empty(_) => {}
            CliToolDispatcher::Mcp(adapter) => adapter.shutdown().await,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for CliToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        match self {
            CliToolDispatcher::Empty(d) => d.tools(),
            CliToolDispatcher::Mcp(d) => d.tools(),
        }
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        match self {
            CliToolDispatcher::Empty(d) => d.dispatch(name, args).await,
            CliToolDispatcher::Mcp(d) => d.dispatch(name, args).await,
        }
    }
}
