//! Adapters to bridge existing crate types to agent traits

#![allow(dead_code)]

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_core::{
    AgentError, Message, Session, SessionId, StopReason, ToolCall, ToolDef, Usage,
    agent::{AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult},
    error::{invalid_session_id, store_error},
    event::AgentEvent,
};
use meerkat_store::SessionStore;
use meerkat_tools::ToolError;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Dynamic dispatch adapter for runtime LLM provider selection
pub struct DynLlmClientAdapter {
    client: Arc<dyn LlmClient>,
    model: Cow<'static, str>,
    /// Optional channel to emit streaming events (text deltas)
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    /// Provider-specific parameters to pass with every request
    provider_params: Option<Value>,
}

impl DynLlmClientAdapter {
    pub fn new(client: Arc<dyn LlmClient>, model: impl Into<Cow<'static, str>>) -> Self {
        Self {
            client,
            model: model.into(),
            event_tx: None,
            provider_params: None,
        }
    }

    /// Create an adapter with streaming event support.
    /// Text deltas will be sent to the provided channel as they arrive.
    pub fn with_event_channel(
        client: Arc<dyn LlmClient>,
        model: impl Into<Cow<'static, str>>,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Self {
        Self {
            client,
            model: model.into(),
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
        tools: &[Arc<ToolDef>],
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
            model: self.model.to_string(),
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
        let mut tool_call_buffers: HashMap<Arc<str>, ToolCallBuffer> = HashMap::new();
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
                        let id: Arc<str> = id.into();
                        let buffer = tool_call_buffers
                            .entry(Arc::clone(&id))
                            .or_insert_with(|| ToolCallBuffer::new(Arc::clone(&id)));

                        if let Some(n) = name {
                            buffer.name = Some(n.into());
                        }
                        buffer.args_json.push_str(&args_delta);
                    }
                    LlmEvent::ToolCallComplete {
                        id,
                        name,
                        args,
                        thought_signature,
                    } => {
                        let tool_call = match thought_signature {
                            Some(sig) => ToolCall::with_thought_signature(id, name, args, sig),
                            None => ToolCall::new(id, name, args),
                        };
                        tool_calls.push(tool_call);
                    }
                    LlmEvent::UsageUpdate { usage: u } => {
                        usage = u;
                    }
                    LlmEvent::Done { outcome } => match outcome {
                        LlmDoneOutcome::Success { stop_reason: sr } => {
                            stop_reason = sr;
                        }
                        LlmDoneOutcome::Error { error } => {
                            return Err(AgentError::llm(
                                self.client.provider(),
                                error.failure_reason(),
                                error.to_string(),
                            ));
                        }
                    },
                },
                Err(e) => {
                    return Err(AgentError::llm(
                        self.client.provider(),
                        e.failure_reason(),
                        e.to_string(),
                    ));
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

        Ok(LlmStreamResult::new(
            content,
            tool_calls,
            stop_reason,
            usage,
        ))
    }

    fn provider(&self) -> &'static str {
        self.client.provider()
    }
}

/// Helper for accumulating tool call deltas
#[derive(Debug, Default)]
struct ToolCallBuffer {
    id: Arc<str>,
    name: Option<Cow<'static, str>>,
    args_json: String,
}

impl ToolCallBuffer {
    fn new(id: Arc<str>) -> Self {
        Self {
            id,
            name: None,
            args_json: String::new(),
        }
    }

    fn try_complete(&self) -> Option<ToolCall> {
        let name = self.name.as_ref()?;
        let args: Value = serde_json::from_str(&self.args_json).ok()?;
        Some(ToolCall::new(self.id.to_string(), name.to_string(), args))
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
        self.store.save(session).await.map_err(store_error)
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        // Parse the string ID to SessionId
        let session_id = SessionId::parse(id).map_err(invalid_session_id)?;

        self.store.load(&session_id).await.map_err(store_error)
    }
}

/// Empty tool dispatcher for when no tools are configured
pub struct EmptyToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for EmptyToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
        Err(ToolError::not_found(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use futures::Stream;
    use meerkat_client::LlmError;
    use meerkat_core::UserMessage;
    use std::pin::Pin;

    struct MockClient;

    #[async_trait]
    impl LlmClient for MockClient {
        fn stream<'a>(
            &'a self,
            _request: &'a LlmRequest,
        ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: "call_1".to_string(),
                    name: "get_weather".to_string(),
                    args: serde_json::json!({"city": "Tokyo"}),
                    thought_signature: Some("sig_123".to_string()),
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: StopReason::ToolUse,
                    },
                }),
            ]))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_tool_call_complete_preserves_thought_signature() {
        let client = Arc::new(MockClient);
        let adapter = DynLlmClientAdapter::new(client, "mock-model");
        let result = adapter
            .stream_response(
                &[Message::User(UserMessage {
                    content: "test".to_string(),
                })],
                &[],
                128,
                None,
                None,
            )
            .await
            .expect("stream_response should succeed");

        assert_eq!(result.tool_calls().len(), 1);
        assert_eq!(
            result.tool_calls()[0].thought_signature.as_deref(),
            Some("sig_123")
        );
    }
}

use meerkat_mcp::McpRouter;
use meerkat_tools::builtin::CompositeDispatcher;
use tokio::sync::RwLock;

/// Adapter that wraps an McpRouter to implement AgentToolDispatcher
pub struct McpRouterAdapter {
    router: RwLock<Option<McpRouter>>,
    cached_tools: RwLock<Arc<[Arc<ToolDef>]>>,
}

impl McpRouterAdapter {
    pub fn new(router: McpRouter) -> Self {
        Self {
            router: RwLock::new(Some(router)),
            cached_tools: RwLock::new(Arc::from([])),
        }
    }

    /// Refresh the cached tool list from the router
    ///
    /// Note: list_tools() is now synchronous since tools are cached in the router.
    pub async fn refresh_tools(&self) -> Result<(), String> {
        let router = self.router.read().await;
        if let Some(router) = router.as_ref() {
            let tools: Arc<[Arc<ToolDef>]> = router.list_tools().to_vec().into();
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
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        // Return the cached tools (blocking read in sync context)
        // Note: This requires refresh_tools() to be called before first use
        // We use try_read to avoid deadlocks, falling back to empty vec
        match self.cached_tools.try_read() {
            Ok(tools) => Arc::clone(&tools),
            Err(_) => Arc::from([]),
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
    /// Dispatcher wrapped with comms tools (uses Arc to match wrap_with_comms output)
    WithComms(Arc<dyn AgentToolDispatcher>),
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
            CliToolDispatcher::WithComms(_) => {
                // Comms connections are managed by CommsRuntime
            }
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for CliToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        match self {
            CliToolDispatcher::Empty(d) => d.tools(),
            CliToolDispatcher::Mcp(d) => d.tools(),
            CliToolDispatcher::Composite(d) => d.tools(),
            CliToolDispatcher::WithComms(d) => d.tools(),
        }
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        match self {
            CliToolDispatcher::Empty(d) => d.dispatch(name, args).await,
            CliToolDispatcher::Mcp(d) => d.dispatch(name, args).await,
            CliToolDispatcher::Composite(d) => d.dispatch(name, args).await,
            CliToolDispatcher::WithComms(d) => d.dispatch(name, args).await,
        }
    }
}
