//! Adapter from LlmClient to AgentLlmClient.

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::{
    AgentError, AgentEvent, AgentLlmClient, LlmStreamResult, Message, StopReason, ToolCall,
    ToolDef, Usage,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::types::{LlmClient, LlmEvent, LlmRequest, ToolCallBuffer};

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

#[async_trait]
impl AgentLlmClient for LlmClientAdapter {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
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
                        if let Some(ref tx) = self.event_tx {
                            let _ = tx.send(AgentEvent::TextDelta { delta }).await;
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
                        buffer.push_args(&args_delta);
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
                    LlmEvent::Done { stop_reason: sr } => {
                        stop_reason = sr;
                    }
                },
                Err(e) => {
                    return Err(AgentError::LlmError(e.to_string()));
                }
            }
        }

        for (_, buffer) in tool_call_buffers {
            if let Some(tc) = buffer.try_complete() {
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
