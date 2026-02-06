//! Adapters to bridge existing crate types to agent traits

#![allow(dead_code)]

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_client::{BlockAssembler, LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_core::{
    AgentError, Message, OutputSchema, Session, SessionId, StopReason, ToolCallView, ToolDef, ToolResult, Usage,
    agent::{AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult},
    error::{invalid_session_id, store_error},
    event::AgentEvent,
    schema::{CompiledSchema, SchemaError},
};
use meerkat_store::SessionStore;
use meerkat_tools::ToolError;
use serde_json::Value;
use std::borrow::Cow;
use std::sync::Arc;
use tokio::sync::mpsc;

#[allow(clippy::unwrap_used, clippy::expect_used)]
fn fallback_raw_value() -> Box<serde_json::value::RawValue> {
    serde_json::value::RawValue::from_string("{}".to_string()).expect("static JSON is valid")
}

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

        // Accumulate response blocks
        let mut assembler = BlockAssembler::new();
        let mut reasoning_started = false;
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = Usage::default();

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => match event {
                    LlmEvent::TextDelta { delta, meta } => {
                        assembler.on_text_delta(&delta, meta);
                        // Emit streaming event if channel is configured
                        if let Some(ref tx) = self.event_tx {
                            let _ = tx
                                .send(AgentEvent::TextDelta {
                                    delta: delta.clone(),
                                })
                                .await;
                        }
                    }
                    LlmEvent::ReasoningDelta { delta } => {
                        if !reasoning_started {
                            reasoning_started = true;
                            assembler.on_reasoning_start();
                        }
                        if let Err(e) = assembler.on_reasoning_delta(&delta) {
                            tracing::warn!(?e, "orphaned reasoning delta");
                        }
                    }
                    LlmEvent::ReasoningComplete { text, meta } => {
                        if !reasoning_started {
                            assembler.on_reasoning_start();
                            let _ = assembler.on_reasoning_delta(&text);
                        }
                        assembler.on_reasoning_complete(meta);
                        reasoning_started = false;
                    }
                    LlmEvent::ToolCallDelta {
                        id,
                        name,
                        args_delta,
                    } => {
                        if let Err(e) =
                            assembler.on_tool_call_delta(&id, name.as_deref(), &args_delta)
                        {
                            if matches!(
                                e,
                                meerkat_client::StreamAssemblyError::OrphanedToolDelta(_)
                            ) {
                                let _ = assembler.on_tool_call_start(id.clone());
                                if let Err(e) =
                                    assembler.on_tool_call_delta(&id, name.as_deref(), &args_delta)
                                {
                                    tracing::warn!(?e, "orphaned tool delta");
                                }
                            } else {
                                tracing::warn!(?e, "tool delta error");
                            }
                        }
                    }
                    LlmEvent::ToolCallComplete {
                        id,
                        name,
                        args,
                        meta,
                    } => {
                        let effective_meta = meta;
                        let args_raw = match serde_json::to_string(&args)
                            .ok()
                            .and_then(|s| serde_json::value::RawValue::from_string(s).ok())
                        {
                            Some(raw) => raw,
                            None => fallback_raw_value(),
                        };
                        let _ = assembler.on_tool_call_complete(id, name, args_raw, effective_meta);
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
        Ok(LlmStreamResult::new(
            assembler.finalize(),
            stop_reason,
            usage,
        ))
    }

    fn provider(&self) -> &'static str {
        self.client.provider()
    }

    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        self.client.compile_schema(output_schema)
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

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        Err(ToolError::not_found(call.name))
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

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let guard = self.router.read().await;
        match &*guard {
            Some(router) => {
                let args: Value = serde_json::from_str(call.args.get())
                    .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
                let result = router
                    .call_tool(call.name, &args)
                    .await
                    .map_err(|e| ToolError::execution_failed(e.to_string()))?;
                Ok(ToolResult {
                    tool_use_id: call.id.to_string(),
                    content: result,
                    is_error: false,
                })
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

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        match self {
            CliToolDispatcher::Empty(d) => d.dispatch(call).await,
            CliToolDispatcher::Mcp(d) => d.dispatch(call).await,
            CliToolDispatcher::Composite(d) => d.dispatch(call).await,
            CliToolDispatcher::WithComms(d) => d.dispatch(call).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::Stream;
    use futures::stream;
    use meerkat_client::LlmError;
    use meerkat_core::{AssistantBlock, ProviderMeta, UserMessage};
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
                    meta: Some(Box::new(ProviderMeta::Gemini {
                        thought_signature: "sig_123".to_string(),
                    })),
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
    async fn test_tool_call_complete_preserves_thought_signature()
    -> Result<(), Box<dyn std::error::Error>> {
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
            .await?;

        let mut sig = None;
        for block in result.blocks() {
            if let AssistantBlock::ToolUse { meta, .. } = block {
                if let Some(ProviderMeta::Gemini { thought_signature }) = meta.as_deref() {
                    sig = Some(thought_signature.as_str());
                }
            }
        }
        assert_eq!(sig, Some("sig_123"));
        Ok(())
    }
}
