//! Adapter from LlmClient to AgentLlmClient.

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::lifecycle::run_primitive::{ProviderParamsOverride, ProviderTag};
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{
    AgentError, AgentEvent, AgentLlmClient, LlmStreamResult, Message, OutputSchema, StopReason,
    ToolDef, Usage,
};
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
    /// Default typed per-request provider-specific knobs. Overridden on
    /// a per-call basis when [`AgentLlmClient::stream_response`] is
    /// invoked with `Some(provider_params)`.
    provider_params: Option<ProviderTag>,
    /// Per-interaction event tap for streaming events to subscribers.
    event_tap: meerkat_core::EventTap,
}

impl LlmClientAdapter {
    pub fn new(client: Arc<dyn LlmClient>, model: String) -> Self {
        Self {
            client,
            model,
            event_tx: None,
            provider_params: None,
            event_tap: meerkat_core::new_event_tap(),
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
            event_tap: meerkat_core::new_event_tap(),
        }
    }

    /// Set default typed provider-specific parameters to apply on every
    /// request. Per-call typed overrides from
    /// [`AgentLlmClient::stream_response`] take precedence when present.
    pub fn with_provider_params(mut self, params: Option<ProviderTag>) -> Self {
        self.provider_params = params;
        self
    }

    /// Set the event tap for interaction-scoped streaming.
    pub fn with_event_tap(mut self, tap: meerkat_core::EventTap) -> Self {
        self.event_tap = tap;
        self
    }

    fn strip_non_object_provider_tool_overrides(tag: ProviderTag) -> ProviderTag {
        match tag {
            ProviderTag::Anthropic(mut tag) => {
                if tag
                    .web_search
                    .as_ref()
                    .is_some_and(|body| !body.as_value().is_object())
                {
                    tag.web_search = None;
                }
                ProviderTag::Anthropic(tag)
            }
            ProviderTag::OpenAi(mut tag) => {
                if tag
                    .web_search
                    .as_ref()
                    .is_some_and(|body| !body.as_value().is_object())
                {
                    tag.web_search = None;
                }
                ProviderTag::OpenAi(tag)
            }
            ProviderTag::Gemini(mut tag) => {
                if tag
                    .google_search
                    .as_ref()
                    .is_some_and(|body| !body.as_value().is_object())
                {
                    tag.google_search = None;
                }
                ProviderTag::Gemini(tag)
            }
            other => other,
        }
    }

    fn apply_generic_provider_overrides(
        &self,
        tag: Option<ProviderTag>,
        params: Option<&ProviderParamsOverride>,
    ) -> Option<ProviderTag> {
        let Some(params) = params else {
            return tag;
        };

        match self.client.provider() {
            "anthropic" if params.thinking_budget_tokens.is_some() => match tag {
                Some(ProviderTag::Anthropic(mut tag)) => {
                    tag.thinking_budget_tokens = params.thinking_budget_tokens;
                    Some(ProviderTag::Anthropic(tag))
                }
                None => Some(ProviderTag::Anthropic(
                    meerkat_core::lifecycle::run_primitive::AnthropicProviderTag {
                        thinking_budget_tokens: params.thinking_budget_tokens,
                        ..Default::default()
                    },
                )),
                other => other,
            },
            "gemini" | "google"
                if params.top_p.is_some() || params.thinking_budget_tokens.is_some() =>
            {
                match tag {
                    Some(ProviderTag::Gemini(mut tag)) => {
                        if let Some(top_p) = params.top_p {
                            tag.top_p = Some(top_p);
                        }
                        if let Some(budget) = params.thinking_budget_tokens {
                            tag.thinking_budget = Some(budget);
                        }
                        Some(ProviderTag::Gemini(tag))
                    }
                    None => Some(ProviderTag::Gemini(
                        meerkat_core::lifecycle::run_primitive::GeminiProviderTag {
                            top_p: params.top_p,
                            thinking_budget: params.thinking_budget_tokens,
                            ..Default::default()
                        },
                    )),
                    other => other,
                }
            }
            _ => tag,
        }
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
fn fallback_raw_value() -> Box<serde_json::value::RawValue> {
    serde_json::value::RawValue::from_string("{}".to_string()).expect("static JSON is valid")
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentLlmClient for LlmClientAdapter {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let effective_params = provider_params
            .and_then(|params| params.provider_tag.clone())
            .or_else(|| self.provider_params.clone());
        let effective_params =
            self.apply_generic_provider_overrides(effective_params, provider_params);
        let effective_params = effective_params.map(Self::strip_non_object_provider_tool_overrides);
        let effective_max_tokens = provider_params
            .and_then(|params| params.max_output_tokens)
            .unwrap_or(max_tokens);
        let effective_temperature = provider_params
            .and_then(|params| params.temperature)
            .or(temperature);

        let projected_messages =
            self.client
                .project_replay_messages(messages)
                .map_err(|error| {
                    AgentError::llm(
                        self.client.provider(),
                        error.failure_reason(),
                        error.to_string(),
                    )
                })?;

        let request = LlmRequest {
            model: self.model.clone(),
            messages: projected_messages,
            tools: tools.to_vec(),
            max_tokens: effective_max_tokens,
            temperature: effective_temperature,
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
                        meerkat_core::tap_try_send(
                            &self.event_tap,
                            &AgentEvent::TextDelta {
                                delta: delta.clone(),
                            },
                        );
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
                        meerkat_core::tap_try_send(
                            &self.event_tap,
                            &AgentEvent::ReasoningDelta {
                                delta: delta.clone(),
                            },
                        );
                        if let Some(ref tx) = self.event_tx {
                            let _ = tx.send(AgentEvent::ReasoningDelta { delta }).await;
                        }
                    }
                    LlmEvent::ReasoningComplete { text, meta } => {
                        if !reasoning_started {
                            assembler.on_reasoning_start();
                            let _ = assembler.on_reasoning_delta(&text);
                        }
                        // Ordering is intentional: snapshot reasoning text before
                        // `on_reasoning_complete(meta)` because completion may clear
                        // the internal reasoning buffer.
                        let reasoning_text = assembler.current_reasoning_text();
                        assembler.on_reasoning_complete(meta);
                        reasoning_started = false;
                        meerkat_core::tap_try_send(
                            &self.event_tap,
                            &AgentEvent::ReasoningComplete {
                                content: reasoning_text.clone(),
                            },
                        );
                        if let Some(ref tx) = self.event_tx {
                            let _ = tx
                                .send(AgentEvent::ReasoningComplete {
                                    content: reasoning_text,
                                })
                                .await;
                        }
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
                    LlmEvent::ServerToolContent {
                        id,
                        name,
                        content,
                        meta,
                    } => {
                        let event_id = id.clone();
                        assembler.on_server_tool_content(id, name.clone(), content.clone(), meta);
                        if let Some(ref tx) = self.event_tx {
                            let _ = tx
                                .send(AgentEvent::ServerToolContent {
                                    id: event_id,
                                    name,
                                    content,
                                })
                                .await;
                        }
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

    fn model(&self) -> &str {
        &self.model
    }

    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        self.client.compile_schema(output_schema)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LlmError, LlmStream};
    use futures::stream;
    use meerkat_core::{
        AssistantBlock, AssistantImageId, BlobId, BlobRef, MediaType, ProviderImageMetadata,
        RevisedPromptDisposition, UserMessage,
    };
    use std::sync::Mutex;

    struct ProjectionClient {
        seen: Arc<Mutex<Option<Vec<Message>>>>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl LlmClient for ProjectionClient {
        fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
            Ok(messages
                .iter()
                .filter(|message| {
                    !matches!(
                        message,
                        Message::BlockAssistant(assistant)
                            if assistant
                                .blocks
                                .iter()
                                .any(|block| matches!(block, AssistantBlock::Image { .. }))
                    )
                })
                .cloned()
                .collect())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            *self.seen.lock().expect("seen lock") = Some(request.messages.clone());
            Box::pin(stream::iter([Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                },
            })]))
        }

        fn provider(&self) -> &'static str {
            "projection-test"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    fn assistant_image_block() -> AssistantBlock {
        AssistantBlock::Image {
            image_id: AssistantImageId::new(meerkat_core::time_compat::new_uuid_v7()),
            blob_ref: BlobRef {
                blob_id: BlobId::from("blob-1"),
                media_type: "image/png".to_string(),
            },
            media_type: MediaType::new("image/png"),
            width: 64,
            height: 64,
            revised_prompt: RevisedPromptDisposition::NotRequested,
            meta: ProviderImageMetadata::NotEmitted,
        }
    }

    #[tokio::test]
    async fn adapter_invokes_provider_replay_projection_before_streaming() {
        let seen = Arc::new(Mutex::new(None));
        let client = ProjectionClient {
            seen: Arc::clone(&seen),
        };
        let adapter = LlmClientAdapter::new(Arc::new(client), "test-model".to_string());

        let raw_messages = vec![
            Message::User(UserMessage::text("continue")),
            Message::BlockAssistant(meerkat_core::BlockAssistantMessage::new(
                vec![assistant_image_block()],
                StopReason::EndTurn,
            )),
        ];

        adapter
            .stream_response(&raw_messages, &[], 1024, None, None)
            .await
            .expect("stream response");

        let projected = seen.lock().expect("seen lock").clone().expect("request");
        assert_eq!(projected.len(), 1);
        assert!(matches!(projected[0], Message::User(_)));
    }
}
