//! Adapter from LlmClient to AgentLlmClient.

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::{
    AgentError, AgentEvent, AgentLlmClient, LlmStreamResult, Message, StopReason, ToolDef, Usage,
};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::block_assembler::BlockAssembler;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};

/// Shared adapter for streaming LLM clients.
#[derive(Clone)]
pub struct LlmClientAdapter {
    client: Arc<dyn LlmClient>,
    model: String,
    /// Optional channel to emit streaming text deltas.
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    /// Provider-specific parameters to pass with every request.
    provider_params: Option<Value>,
}

impl LlmClientAdapter {
    pub fn new(client: Arc<dyn LlmClient>, model: String) -> Self {
        Self {
            client,
            model,
            event_tx: None,
            provider_params: None,
        }
    }

    /// Create an adapter with streaming event support.
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

    /// Set provider-specific parameters to pass with every request.
    pub fn with_provider_params(mut self, params: Option<Value>) -> Self {
        self.provider_params = params;
        self
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
fn fallback_raw_value() -> Box<serde_json::value::RawValue> {
    serde_json::value::RawValue::from_string("{}".to_string()).expect("static JSON is valid")
}

#[async_trait]
impl AgentLlmClient for LlmClientAdapter {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&Value>,
    ) -> Result<LlmStreamResult, AgentError> {
        let effective_params = provider_params
            .cloned()
            .or_else(|| self.provider_params.clone());

        let request = LlmRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            max_tokens,
            temperature,
            stop_sequences: None,
            provider_params: effective_params,
        };

        let mut stream = self.client.stream(&request);

        let mut assembler = BlockAssembler::new();
        let mut reasoning_started = false;
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = Usage::default();

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => match event {
                    LlmEvent::TextDelta { delta, meta } => {
                        assembler.on_text_delta(&delta, meta);
                        if let Some(ref tx) = self.event_tx {
                            let _ = tx.send(AgentEvent::TextDelta { delta }).await;
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
                                crate::block_assembler::StreamAssemblyError::OrphanedToolDelta(_)
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
}
