//! Adapter from LlmClient to AgentLlmClient.

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::lifecycle::run_primitive::{ProviderParamsOverride, ProviderTag};
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{
    AgentError, AgentEvent, AgentLlmClient, AgentLlmRequestAttempt, AssistantMessageId,
    LlmStreamResult, Message, OutputSchema, Provider, RequestAttemptAuthority, StopReason, ToolDef,
    Usage,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::mpsc;

use crate::block_assembler::BlockAssembler;
use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, PreparedLlmRequest};

/// Shared adapter for streaming LLM clients.
#[derive(Clone)]
pub struct LlmClientAdapter {
    client: Arc<dyn LlmClient>,
    model: String,
    /// Canonical provider identity selected by the owning factory/session.
    ///
    /// Raw clients are replaceable transport mechanics (including deterministic
    /// test doubles); they must not fork the durable provider identity that
    /// owns request policy and capability projection.
    provider: Provider,
    /// Optional channel to emit streaming text deltas.
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    /// Default typed per-request provider-specific knobs. Overridden on
    /// a per-call basis when [`AgentLlmClient::stream_response`] is
    /// invoked with `Some(provider_params)`.
    provider_params: Option<ProviderTag>,
    /// Per-interaction event tap for streaming events to subscribers.
    event_tap: meerkat_core::EventTap,
    /// True after this adapter emitted user-visible streaming output for the
    /// current call. The agent retry loop reads this to avoid cross-model
    /// fallback after partial output has escaped.
    stream_output_observed: Arc<AtomicBool>,
    /// Monotonic count of raw provider stream events (visible or not),
    /// bumped on every yielded item. The agent loop's stream-inactivity
    /// watchdog compares snapshots of this count to detect a hung stream.
    stream_activity: Arc<AtomicU64>,
}

impl LlmClientAdapter {
    pub fn new(client: Arc<dyn LlmClient>, model: String) -> Self {
        let provider = client.provider();
        Self::new_bound(client, model, provider, None)
    }

    /// Bind a raw transport client to the canonical provider identity selected
    /// by the owning factory/session.
    pub fn try_for_provider_identity(
        client: Arc<dyn LlmClient>,
        model: String,
        provider: Provider,
    ) -> Result<Self, LlmError> {
        Self::validate_provider_binding(client.provider(), provider, &model)?;
        Ok(Self::new_bound(client, model, provider, None))
    }

    fn validate_provider_binding(
        client_provider: Provider,
        provider: Provider,
        model: &str,
    ) -> Result<(), LlmError> {
        if matches!(client_provider, Provider::Other) || client_provider == provider {
            return Ok(());
        }
        Err(LlmError::InvalidRequest {
            message: format!(
                "raw LLM client provider '{}' cannot back canonical identity '{}:{model}'",
                client_provider.as_str(),
                provider.as_str(),
            ),
        })
    }

    /// Project host-declared usage from a provider-agnostic raw client onto
    /// the canonical identity that owns this adapter.
    ///
    /// `Provider::Other` is the explicit wildcard accepted by
    /// [`Self::validate_provider_binding`]. A wildcard client cannot author a
    /// concrete provider identity, so its host-declared accounting inherits
    /// the canonical provider selected by the owning factory/session. Concrete
    /// provider evidence, provider-authored conventions, and model mismatches
    /// remain untouched so the core identity validator can reject them.
    fn project_wildcard_host_declared_usage(
        &self,
        usage: meerkat_core::TurnUsage,
    ) -> meerkat_core::TurnUsage {
        let accounting = usage.accounting();
        let should_project = self.client.provider() == Provider::Other
            && self.provider != Provider::Other
            && accounting.provider == Provider::Other
            && accounting.model == self.model
            && accounting.convention
                == meerkat_core::PresentedTokenConvention::HostDeclaredInclusiveInputTotal;
        if !should_project {
            return usage;
        }

        let raw_usage = usage.as_usage().clone();
        let mut accounting = accounting.clone();
        accounting.provider = self.provider;
        accounting.model.clone_from(&self.model);
        meerkat_core::TurnUsage::new(raw_usage, accounting)
    }

    fn new_bound(
        client: Arc<dyn LlmClient>,
        model: String,
        provider: Provider,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Self {
        Self {
            client,
            model,
            provider,
            event_tx,
            provider_params: None,
            event_tap: meerkat_core::new_event_tap(),
            stream_output_observed: Arc::new(AtomicBool::new(false)),
            stream_activity: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Create an adapter with streaming event support.
    pub fn with_event_channel(
        client: Arc<dyn LlmClient>,
        model: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Self {
        let provider = client.provider();
        Self::new_bound(client, model, provider, Some(event_tx))
    }

    /// Create an identity-bound adapter with streaming event support.
    pub fn try_with_event_channel_for_provider_identity(
        client: Arc<dyn LlmClient>,
        model: String,
        provider: Provider,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Self, LlmError> {
        Self::validate_provider_binding(client.provider(), provider, &model)?;
        Ok(Self::new_bound(client, model, provider, Some(event_tx)))
    }

    /// Publish one live event to the interaction tap (best effort) and then
    /// to the run's event channel, in that order, like the agent loop does.
    async fn publish(&self, event: AgentEvent) {
        meerkat_core::tap_try_send(&self.event_tap, &event);
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event).await;
        }
    }

    fn mark_visible_stream_output(&self, text: &str) {
        if !text.trim().is_empty() {
            self.stream_output_observed.store(true, Ordering::SeqCst);
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

        match self.provider {
            Provider::Anthropic if params.thinking_budget_tokens.is_some() => match tag {
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
            Provider::Gemini
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
            // Explicit no-op arms: these providers (and the unmatched-guard
            // Anthropic/Gemini cases) carry no generic override here.
            Provider::Anthropic
            | Provider::Gemini
            | Provider::OpenAI
            | Provider::SelfHosted
            | Provider::Other => tag,
        }
    }

    /// Build the exact raw-provider request used by both pressure observation
    /// and streaming. Keeping this projection in one helper prevents the
    /// compaction witness from drifting from the eventual provider call.
    fn build_request(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<PreparedLlmRequest, AgentError> {
        let effective_params = provider_params
            .and_then(|params| params.provider_tag.clone())
            .or_else(|| self.provider_params.clone());
        let effective_params =
            self.apply_generic_provider_overrides(effective_params, provider_params);
        let effective_params = effective_params.map(Self::strip_non_object_provider_tool_overrides);
        // The per-call host override intentionally wins. HomeCore uses this
        // escape hatch to raise Fable 5's output allowance while older config
        // surfaces are being migrated.
        let effective_max_tokens = provider_params
            .and_then(|params| params.max_output_tokens)
            .unwrap_or(max_tokens);
        let effective_temperature = provider_params
            .and_then(|params| params.temperature)
            .or(temperature);
        let projection = self
            .client
            .project_replay_request(messages)
            .map_err(|error| error.into_agent_error(self.provider.as_str()))?;

        Ok(PreparedLlmRequest::from_projection(
            LlmRequest {
                model: self.model.clone(),
                messages: Vec::new(),
                tools: tools.to_vec(),
                max_tokens: effective_max_tokens,
                temperature: effective_temperature,
                stop_sequences: None,
                provider_params: effective_params,
                tool_choice: provider_params
                    .and_then(|params| params.tool_choice.clone())
                    .unwrap_or_default(),
            },
            projection,
        ))
    }

    /// Stream one prepared request, publishing its live events.
    ///
    /// `assistant_message_id` is the core-minted identity of the assistant
    /// message this provider turn commits; every message-scoped live event
    /// (`TextDelta`, `ReasoningDelta`, `ReasoningComplete`,
    /// `ServerToolContent`) carries it on both the event channel and the tap.
    /// `None` marks output that is not a transcript assistant message (the
    /// legacy [`AgentLlmClient::stream_response`] path, used by the compaction
    /// summary call). The id is event metadata only and never reaches the
    /// provider request.
    async fn stream_prepared_response(
        &self,
        request: &PreparedLlmRequest,
        canonical_messages: &[Message],
        assistant_message_id: Option<AssistantMessageId>,
    ) -> Result<LlmStreamResult, AgentError> {
        let cache_breakpoint_claims = self
            .client
            .prepared_cache_breakpoints(request, canonical_messages)
            .map_err(|error| error.into_agent_error(self.provider.as_str()))?;
        let mut stream = self.client.stream_prepared(request);
        let mut assembler = BlockAssembler::new();
        let mut reasoning_started = false;
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = Usage::default();

        while let Some(result) = stream.next().await {
            self.stream_activity.fetch_add(1, Ordering::SeqCst);
            match result {
                Ok(event) => match event {
                    LlmEvent::AssistantOutput { blocks } => {
                        assembler = BlockAssembler::from_final_blocks(blocks);
                        reasoning_started = false;
                    }
                    LlmEvent::TextDelta { delta, meta } => {
                        assembler.on_text_delta(&delta, meta);
                        self.mark_visible_stream_output(&delta);
                        self.publish(AgentEvent::TextDelta {
                            delta,
                            assistant_message_id,
                        })
                        .await;
                    }
                    LlmEvent::ReasoningDelta { delta } => {
                        if !reasoning_started {
                            reasoning_started = true;
                            assembler.on_reasoning_start();
                        }
                        self.mark_visible_stream_output(&delta);
                        if let Err(error) = assembler.on_reasoning_delta(&delta) {
                            tracing::warn!(?error, "orphaned reasoning delta");
                        }
                        self.publish(AgentEvent::ReasoningDelta {
                            delta,
                            assistant_message_id,
                        })
                        .await;
                    }
                    LlmEvent::ReasoningComplete { text, meta } => {
                        self.mark_visible_stream_output(&text);
                        if !reasoning_started {
                            assembler.on_reasoning_start();
                            let _ = assembler.on_reasoning_delta(&text);
                        }
                        let reasoning_text = assembler.current_reasoning_text();
                        assembler.on_reasoning_complete(meta);
                        reasoning_started = false;
                        self.mark_visible_stream_output(&reasoning_text);
                        self.publish(AgentEvent::ReasoningComplete {
                            content: reasoning_text,
                            assistant_message_id,
                        })
                        .await;
                    }
                    LlmEvent::ToolCallDelta {
                        id,
                        name,
                        args_delta,
                    } => {
                        if let Err(error) =
                            assembler.on_tool_call_delta(&id, name.as_deref(), &args_delta)
                        {
                            if matches!(
                                error,
                                crate::block_assembler::StreamAssemblyError::OrphanedToolDelta(_)
                            ) {
                                let _ = assembler.on_tool_call_start(id.clone());
                                if let Err(error) =
                                    assembler.on_tool_call_delta(&id, name.as_deref(), &args_delta)
                                {
                                    tracing::warn!(?error, "orphaned tool delta");
                                }
                            } else {
                                tracing::warn!(?error, "tool delta error");
                            }
                        }
                    }
                    LlmEvent::ToolCallComplete {
                        id,
                        name,
                        args,
                        meta,
                    } => {
                        let args_raw = match serde_json::to_string(&args)
                            .ok()
                            .and_then(|json| serde_json::value::RawValue::from_string(json).ok())
                        {
                            Some(raw) => raw,
                            None => fallback_raw_value(),
                        };
                        let _ = assembler.on_tool_call_complete(id, name, args_raw, meta);
                    }
                    LlmEvent::ServerToolContent {
                        id,
                        kind,
                        content,
                        meta,
                    } => {
                        let event_id = id.clone();
                        assembler.on_server_tool_content(id, kind.clone(), content.clone(), meta);
                        self.publish(AgentEvent::ServerToolContent {
                            id: event_id,
                            kind,
                            content,
                            assistant_message_id,
                        })
                        .await;
                    }
                    LlmEvent::UsageUpdate { usage: update } => {
                        usage = self
                            .project_wildcard_host_declared_usage(update)
                            .into_inner();
                    }
                    LlmEvent::OperationObservationFailed {
                        operation_id,
                        phase,
                    } => {
                        self.publish(AgentEvent::OperationObservationFailed {
                            operation_id,
                            phase,
                        })
                        .await;
                    }
                    LlmEvent::WireLiveness => {}
                    LlmEvent::Done { outcome } => match outcome {
                        LlmDoneOutcome::Success {
                            stop_reason: completed_reason,
                        } => {
                            stop_reason = completed_reason;
                        }
                        LlmDoneOutcome::Error { error } => {
                            return Err(error.into_agent_error(self.provider.as_str()));
                        }
                    },
                },
                Err(error) => {
                    return Err(error.into_agent_error(self.provider.as_str()));
                }
            }
        }
        if reasoning_started {
            let reasoning_text = assembler.current_reasoning_text();
            assembler.on_reasoning_complete(None);
            self.mark_visible_stream_output(&reasoning_text);
            self.publish(AgentEvent::ReasoningComplete {
                content: reasoning_text,
                assistant_message_id,
            })
            .await;
        }
        Ok(
            LlmStreamResult::new(assembler.finalize(), stop_reason, usage)
                .with_cache_breakpoint_claims(cache_breakpoint_claims),
        )
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
fn fallback_raw_value() -> Box<serde_json::value::RawValue> {
    serde_json::value::RawValue::from_string("{}".to_string()).expect("static JSON is valid")
}

struct LlmClientAdapterAttempt {
    adapter: Arc<LlmClientAdapter>,
    request: PreparedLlmRequest,
    canonical_messages: Arc<Vec<Message>>,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentLlmRequestAttempt for LlmClientAdapterAttempt {
    fn request_pressure(
        &self,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, AgentError> {
        self.adapter
            .client
            .prepared_request_pressure(&self.request)
            .map_err(|error| error.into_agent_error(self.adapter.provider.as_str()))
    }

    async fn stream_response(
        &self,
        assistant_message_id: AssistantMessageId,
    ) -> Result<LlmStreamResult, AgentError> {
        self.adapter
            .stream_prepared_response(
                &self.request,
                &self.canonical_messages,
                Some(assistant_message_id),
            )
            .await
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentLlmClient for LlmClientAdapter {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        let selected = self
            .controller_model_selection()
            .ok_or(meerkat_core::auth::AuthError::StaleCredential)?;
        self.client.prepare_controller_credential().await?;
        if self.controller_model_selection().as_ref() != Some(&selected) {
            return Err(meerkat_core::auth::AuthError::StaleCredential);
        }
        Ok(())
    }

    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        let selection = self.client.controller_model_selection()?;
        (selection.model() == self.model && selection.provider() == self.provider)
            .then_some(selection)
    }

    fn controller_model_facts(
        &self,
    ) -> Result<meerkat_core::ControllerModelFacts, meerkat_core::ControllerFactsUnavailable> {
        let selection = self
            .controller_model_selection()
            .ok_or(meerkat_core::ControllerFactsUnavailable)?;
        let route = self.client.plain_model_route(&self.model)?;
        Ok(meerkat_core::ControllerModelFacts::new(
            selection,
            Arc::from(route.endpoint()),
            Arc::from(route.wire_model()),
        ))
    }

    fn pin_controller(self: Arc<Self>) -> Option<meerkat_core::ControllerModelClient> {
        let selection = self.controller_model_selection()?;
        Some(meerkat_core::ControllerModelClient::new(selection, self))
    }

    async fn stream_response_authorized(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<LlmStreamResult, AgentError> {
        let request = self
            .build_request(messages, tools, max_tokens, temperature, provider_params)?
            .with_authorization(authorization);
        self.stream_prepared_response(&request, messages, None)
            .await
    }

    fn prepare_request_attempt_authorized(
        self: Arc<Self>,
        messages: Arc<Vec<Message>>,
        tools: Arc<[Arc<ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<ProviderParamsOverride>,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<Arc<dyn AgentLlmRequestAttempt>, AgentError> {
        let request = self
            .build_request(
                messages.as_slice(),
                tools.as_ref(),
                max_tokens,
                temperature,
                provider_params.as_ref(),
            )?
            .with_authorization(authorization);
        Ok(Arc::new(LlmClientAdapterAttempt {
            adapter: self,
            request,
            canonical_messages: messages,
        }))
    }

    fn prepare_request_attempt(
        self: Arc<Self>,
        messages: Arc<Vec<Message>>,
        tools: Arc<[Arc<ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<ProviderParamsOverride>,
    ) -> Result<Arc<dyn AgentLlmRequestAttempt>, AgentError> {
        let request = self.build_request(
            messages.as_slice(),
            tools.as_ref(),
            max_tokens,
            temperature,
            provider_params.as_ref(),
        )?;
        Ok(Arc::new(LlmClientAdapterAttempt {
            adapter: self,
            request,
            canonical_messages: messages,
        }))
    }

    fn request_attempt_authority(&self) -> RequestAttemptAuthority {
        RequestAttemptAuthority::Unified
    }

    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let request =
            self.build_request(messages, tools, max_tokens, temperature, provider_params)?;
        // The legacy split path has no assistant message identity: it serves
        // non-transcript calls such as the compaction summary.
        self.stream_prepared_response(&request, messages, None)
            .await
    }

    fn request_pressure(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, AgentError> {
        let request =
            self.build_request(messages, tools, max_tokens, temperature, provider_params)?;
        self.client
            .prepared_request_pressure(&request)
            .map_err(|error| error.into_agent_error(self.provider.as_str()))
    }

    fn target_cache_lowering_capabilities(
        &self,
        issuer: &meerkat_core::TargetCacheLoweringIssuer,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<Vec<meerkat_core::TargetCacheLoweringCapability>, AgentError> {
        let request =
            self.build_request(messages, tools, max_tokens, temperature, provider_params)?;
        self.client
            .prepared_cache_breakpoints(&request, messages)
            .map_err(|error| error.into_agent_error(self.provider.as_str()))?
            .into_iter()
            .map(|evidence| {
                issuer.mint(evidence).map_err(|error| {
                    AgentError::InternalError(format!(
                        "target cache lowering produced invalid evidence: {error}"
                    ))
                })
            })
            .collect()
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.provider
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn fork_noncommitting_live_bridge(&self) -> Result<Arc<dyn AgentLlmClient>, AgentError> {
        let mut fork = Self::new_bound(
            Arc::clone(&self.client),
            self.model.clone(),
            self.provider,
            None,
        );
        fork.provider_params = self.provider_params.clone();
        Ok(Arc::new(fork))
    }

    fn begin_stream_output_observation(&self) {
        self.stream_output_observed.store(false, Ordering::SeqCst);
    }

    fn stream_output_observed(&self) -> bool {
        self.stream_output_observed.load(Ordering::SeqCst)
    }

    fn stream_activity_count(&self) -> Option<u64> {
        Some(self.stream_activity.load(Ordering::SeqCst))
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

    struct ScriptedClient {
        events: Vec<Result<LlmEvent, LlmError>>,
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
            let mut seen = match self.seen.lock() {
                Ok(seen) => seen,
                Err(err) => {
                    return Box::pin(stream::iter([Err(LlmError::Unknown {
                        message: format!("seen lock poisoned: {err}"),
                    })]));
                }
            };
            *seen = Some(request.messages.clone());
            Box::pin(stream::iter([
                Ok(LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl LlmClient for ScriptedClient {
        fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, _request: &'a LlmRequest) -> LlmStream<'a> {
            Box::pin(stream::iter(self.events.clone()))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
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

    #[test]
    fn identity_bound_adapter_projects_canonical_provider() -> Result<(), String> {
        let adapter = LlmClientAdapter::try_for_provider_identity(
            Arc::new(ScriptedClient { events: Vec::new() }),
            "claude-sonnet-4-5".to_string(),
            Provider::Anthropic,
        )
        .map_err(|error| error.to_string())?;

        assert_eq!(adapter.provider(), Provider::Anthropic);
        assert_eq!(adapter.model(), "claude-sonnet-4-5");
        Ok(())
    }

    #[test]
    fn openai_generic_top_p_preserves_existing_omission() -> Result<(), String> {
        for model in ["gpt-6-astra", "gpt-5.5"] {
            let adapter = LlmClientAdapter::try_for_provider_identity(
                Arc::new(crate::TestClient::for_provider(Provider::OpenAI)),
                model.to_string(),
                Provider::OpenAI,
            )
            .map_err(|error| error.to_string())?;
            let params = ProviderParamsOverride {
                top_p: Some(0.8),
                ..Default::default()
            };
            let prepared = adapter
                .build_request(&[], &[], 1024, None, Some(&params))
                .map_err(|error| error.to_string())?;
            assert!(prepared.request().provider_params.is_none());
            assert_eq!(params.top_p, Some(0.8));
        }
        Ok(())
    }

    #[test]
    fn identity_bound_adapter_rejects_conflicting_fixed_provider() -> Result<(), String> {
        let error = match LlmClientAdapter::try_for_provider_identity(
            Arc::new(crate::TestClient::for_provider(Provider::OpenAI)),
            "claude-sonnet-4-5".to_string(),
            Provider::Anthropic,
        ) {
            Err(error) => error,
            Ok(_) => return Err("fixed OpenAI client must not bind as Anthropic".to_string()),
        };

        assert!(error.to_string().contains("cannot back canonical identity"));
        Ok(())
    }

    #[tokio::test]
    async fn identity_bound_adapter_projects_wildcard_host_declared_usage() -> Result<(), String> {
        let adapter = LlmClientAdapter::try_for_provider_identity(
            Arc::new(crate::TestClient::default()),
            "gpt-5.5".to_string(),
            Provider::OpenAI,
        )
        .map_err(|error| error.to_string())?;

        let result = adapter
            .stream_response(
                &[Message::User(UserMessage::text("hello"))],
                &[],
                1024,
                None,
                None,
            )
            .await
            .map_err(|error| error.to_string())?;
        let accounting = result
            .usage()
            .provider_accounting
            .as_ref()
            .ok_or_else(|| "projected usage must retain accounting".to_string())?;

        assert_eq!(accounting.provider, Provider::OpenAI);
        assert_eq!(accounting.model, "gpt-5.5");
        assert_eq!(accounting.presented_tokens, 0);
        assert_eq!(
            accounting.convention,
            meerkat_core::PresentedTokenConvention::HostDeclaredInclusiveInputTotal
        );
        Ok(())
    }

    #[tokio::test]
    async fn identity_bound_adapter_does_not_rewrite_concrete_provider_usage() -> Result<(), String>
    {
        let adapter = LlmClientAdapter::try_for_provider_identity(
            Arc::new(ScriptedClient {
                events: vec![
                    Ok(LlmEvent::UsageUpdate {
                        usage: meerkat_core::TurnUsage::host_declared(
                            Provider::Anthropic,
                            "gpt-5.5",
                            Usage::default(),
                        ),
                    }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: StopReason::EndTurn,
                        },
                    }),
                ],
            }),
            "gpt-5.5".to_string(),
            Provider::OpenAI,
        )
        .map_err(|error| error.to_string())?;

        let result = adapter
            .stream_response(
                &[Message::User(UserMessage::text("hello"))],
                &[],
                1024,
                None,
                None,
            )
            .await
            .map_err(|error| error.to_string())?;
        let accounting = result
            .usage()
            .provider_accounting
            .as_ref()
            .ok_or_else(|| "usage must retain accounting".to_string())?;

        assert_eq!(accounting.provider, Provider::Anthropic);
        assert_eq!(accounting.model, "gpt-5.5");
        Ok(())
    }

    #[tokio::test]
    async fn identity_bound_adapter_does_not_rewrite_wildcard_model_mismatch() -> Result<(), String>
    {
        let adapter = LlmClientAdapter::try_for_provider_identity(
            Arc::new(ScriptedClient {
                events: vec![
                    Ok(LlmEvent::UsageUpdate {
                        usage: meerkat_core::TurnUsage::host_declared(
                            Provider::Other,
                            "different-model",
                            Usage::default(),
                        ),
                    }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: StopReason::EndTurn,
                        },
                    }),
                ],
            }),
            "gpt-5.5".to_string(),
            Provider::OpenAI,
        )
        .map_err(|error| error.to_string())?;

        let result = adapter
            .stream_response(
                &[Message::User(UserMessage::text("hello"))],
                &[],
                1024,
                None,
                None,
            )
            .await
            .map_err(|error| error.to_string())?;
        let accounting = result
            .usage()
            .provider_accounting
            .as_ref()
            .ok_or_else(|| "usage must retain accounting".to_string())?;

        assert_eq!(accounting.provider, Provider::Other);
        assert_eq!(accounting.model, "different-model");
        Ok(())
    }

    #[tokio::test]
    async fn identity_bound_adapter_does_not_rewrite_provider_authored_wildcard_usage()
    -> Result<(), String> {
        let adapter = LlmClientAdapter::try_for_provider_identity(
            Arc::new(ScriptedClient {
                events: vec![
                    Ok(LlmEvent::UsageUpdate {
                        usage: meerkat_core::TurnUsage::new(
                            Usage {
                                input_tokens: 5,
                                ..Usage::default()
                            },
                            meerkat_core::ProviderTokenAccounting {
                                provider: Provider::Other,
                                model: "gpt-5.5".to_string(),
                                presented_tokens: 5,
                                convention: meerkat_core::PresentedTokenConvention::OpenAiInputIncludesCachedSubset,
                                aggregation: meerkat_core::TokenAggregationProvenance::ProviderInclusiveInputTotal,
                            },
                        ),
                    }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: StopReason::EndTurn,
                        },
                    }),
                ],
            }),
            "gpt-5.5".to_string(),
            Provider::OpenAI,
        )
        .map_err(|error| error.to_string())?;

        let result = adapter
            .stream_response(
                &[Message::User(UserMessage::text("hello"))],
                &[],
                1024,
                None,
                None,
            )
            .await
            .map_err(|error| error.to_string())?;
        let accounting = result
            .usage()
            .provider_accounting
            .as_ref()
            .ok_or_else(|| "usage must retain accounting".to_string())?;

        assert_eq!(accounting.provider, Provider::Other);
        assert_eq!(
            accounting.convention,
            meerkat_core::PresentedTokenConvention::OpenAiInputIncludesCachedSubset
        );
        Ok(())
    }

    #[tokio::test]
    async fn adapter_invokes_provider_replay_projection_before_streaming() -> Result<(), String> {
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
            .map_err(|err| format!("stream response failed: {err}"))?;

        let projected = seen
            .lock()
            .map_err(|err| format!("seen lock poisoned: {err}"))?
            .clone()
            .ok_or_else(|| "request was not captured".to_string())?;
        assert_eq!(projected.len(), 1);
        assert!(matches!(projected[0], Message::User(_)));
        Ok(())
    }

    #[tokio::test]
    async fn adapter_commits_reasoning_delta_when_done_lacks_reasoning_complete()
    -> Result<(), String> {
        let adapter = LlmClientAdapter::new(
            Arc::new(ScriptedClient {
                events: vec![
                    Ok(LlmEvent::ReasoningDelta {
                        delta: "thinking before silence".to_string(),
                    }),
                    Ok(LlmEvent::UsageUpdate {
                        usage: meerkat_core::TurnUsage::host_declared(
                            Provider::Other,
                            "scripted-model",
                            Usage {
                                input_tokens: 3,
                                output_tokens: 5,
                                ..Usage::default()
                            },
                        ),
                    }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: StopReason::EndTurn,
                        },
                    }),
                ],
            }),
            "scripted-model".to_string(),
        );

        let result = adapter
            .stream_response(
                &[Message::User(UserMessage::text("ack silently"))],
                &[],
                1024,
                None,
                None,
            )
            .await
            .map_err(|err| {
                format!("pending reasoning should be finalized on successful done: {err}")
            })?;

        assert_eq!(result.usage().output_tokens, 5);
        assert!(matches!(
            result.blocks(),
            [AssistantBlock::Reasoning { text, meta }]
                if text == "thinking before silence" && meta.is_none()
        ));
        assert!(
            meerkat_core::assistant_blocks_have_visible_or_actionable_output(result.blocks()),
            "non-empty reasoning delta should survive to the core commit predicate"
        );
        Ok(())
    }

    #[tokio::test]
    async fn adapter_counts_every_raw_stream_event_as_activity() -> Result<(), String> {
        let adapter = LlmClientAdapter::new(
            Arc::new(ScriptedClient {
                events: vec![
                    // Non-visible events must still count as stream liveness.
                    Ok(LlmEvent::UsageUpdate {
                        usage: meerkat_core::TurnUsage::host_declared(
                            Provider::Other,
                            "scripted-model",
                            Usage::default(),
                        ),
                    }),
                    Ok(LlmEvent::TextDelta {
                        delta: "hello".to_string(),
                        meta: None,
                    }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: StopReason::EndTurn,
                        },
                    }),
                ],
            }),
            "scripted-model".to_string(),
        );

        let before = adapter
            .stream_activity_count()
            .ok_or_else(|| "adapter must report stream liveness".to_string())?;
        adapter
            .stream_response(
                &[Message::User(UserMessage::text("count events"))],
                &[],
                1024,
                None,
                None,
            )
            .await
            .map_err(|err| format!("stream response failed: {err}"))?;
        let after = adapter
            .stream_activity_count()
            .ok_or_else(|| "adapter must report stream liveness".to_string())?;

        assert_eq!(after - before, 3, "each raw event bumps the counter once");
        Ok(())
    }

    /// Wire liveness is a watchdog signal only: it must bump the activity
    /// counter (the agent loop re-arms the stall window from it) while
    /// producing no blocks and no visible stream output.
    #[tokio::test]
    async fn adapter_counts_wire_liveness_as_activity_without_output() -> Result<(), String> {
        let adapter = LlmClientAdapter::new(
            Arc::new(ScriptedClient {
                events: vec![
                    Ok(LlmEvent::WireLiveness),
                    Ok(LlmEvent::WireLiveness),
                    Ok(LlmEvent::WireLiveness),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: StopReason::EndTurn,
                        },
                    }),
                ],
            }),
            "scripted-model".to_string(),
        );

        let before = adapter
            .stream_activity_count()
            .ok_or_else(|| "adapter must report stream liveness".to_string())?;
        adapter.begin_stream_output_observation();
        let result = adapter
            .stream_response(
                &[Message::User(UserMessage::text("keepalives only"))],
                &[],
                1024,
                None,
                None,
            )
            .await
            .map_err(|err| format!("stream response failed: {err}"))?;
        let after = adapter
            .stream_activity_count()
            .ok_or_else(|| "adapter must report stream liveness".to_string())?;

        assert_eq!(
            after - before,
            4,
            "every WireLiveness item re-arms the watchdog counter"
        );
        assert!(result.blocks().is_empty(), "liveness is not output");
        assert!(
            !adapter.stream_output_observed(),
            "liveness must not count as visible stream output"
        );
        Ok(())
    }

    fn scripted_message_id() -> Result<AssistantMessageId, String> {
        // Only core mints ids; other crates obtain one the way every consumer
        // does, from its serialized form.
        serde_json::from_value(serde_json::json!("0190f5c2-4a1e-7c3d-8e2f-000000000001"))
            .map_err(|error| error.to_string())
    }

    fn message_scoped_script() -> Vec<Result<LlmEvent, LlmError>> {
        vec![
            Ok(LlmEvent::ReasoningDelta {
                delta: "think".to_string(),
            }),
            Ok(LlmEvent::ReasoningComplete {
                text: "think".to_string(),
                meta: None,
            }),
            Ok(LlmEvent::TextDelta {
                delta: "before ".to_string(),
                meta: None,
            }),
            Ok(LlmEvent::ServerToolContent {
                id: Some("srv-1".to_string()),
                kind: meerkat_core::ServerToolKind::WebSearch,
                content: serde_json::json!({"query": "rust"}),
                meta: None,
            }),
            Ok(LlmEvent::TextDelta {
                delta: "after".to_string(),
                meta: None,
            }),
            // Reasoning left open at the end is flushed as a trailing
            // ReasoningComplete after the stream ends.
            Ok(LlmEvent::ReasoningDelta {
                delta: "trailing".to_string(),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                },
            }),
        ]
    }

    fn event_kinds_and_ids(
        events: &[AgentEvent],
    ) -> Vec<(&'static str, Option<AssistantMessageId>)> {
        events
            .iter()
            .map(|event| {
                (
                    meerkat_core::agent_event_type(event),
                    event.assistant_message_id(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn request_attempt_stamps_every_live_event_on_channel_and_tap() -> Result<(), String> {
        let (tx, mut rx) = mpsc::channel(64);
        let (tap_tx, mut tap_rx) = mpsc::channel(64);
        let tap = meerkat_core::new_event_tap();
        *tap.lock() = Some(meerkat_core::EventTapState {
            tx: tap_tx,
            truncated: AtomicBool::new(false),
        });
        let adapter = Arc::new(
            LlmClientAdapter::with_event_channel(
                Arc::new(ScriptedClient {
                    events: message_scoped_script(),
                }),
                "scripted-model".to_string(),
                tx,
            )
            .with_event_tap(tap),
        );
        let id = scripted_message_id()?;

        let attempt = Arc::clone(&adapter)
            .prepare_request_attempt(
                Arc::new(vec![Message::User(UserMessage::text("hello"))]),
                Arc::from([]),
                1024,
                None,
                None,
            )
            .map_err(|error| error.to_string())?;
        attempt
            .stream_response(id)
            .await
            .map_err(|error| error.to_string())?;

        let expected = vec![
            ("reasoning_delta", Some(id)),
            ("reasoning_complete", Some(id)),
            ("text_delta", Some(id)),
            ("server_tool_content", Some(id)),
            ("text_delta", Some(id)),
            ("reasoning_delta", Some(id)),
            ("reasoning_complete", Some(id)),
        ];
        let channel = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        let tapped = std::iter::from_fn(|| tap_rx.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(event_kinds_and_ids(&channel), expected);
        assert_eq!(
            event_kinds_and_ids(&tapped),
            expected,
            "interaction taps see the same stamped live events, server tool content included"
        );
        Ok(())
    }

    #[tokio::test]
    async fn legacy_stream_response_publishes_no_message_identity() -> Result<(), String> {
        let (tx, mut rx) = mpsc::channel(64);
        let adapter = LlmClientAdapter::with_event_channel(
            Arc::new(ScriptedClient {
                events: message_scoped_script(),
            }),
            "scripted-model".to_string(),
            tx,
        );

        adapter
            .stream_response(
                &[Message::User(UserMessage::text("summarize"))],
                &[],
                1024,
                None,
                None,
            )
            .await
            .map_err(|error| error.to_string())?;

        let channel = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(channel.len(), 7);
        assert!(
            channel
                .iter()
                .all(|event| event.assistant_message_id().is_none()),
            "output outside the request-attempt path is not a transcript assistant message"
        );
        Ok(())
    }

    #[tokio::test]
    async fn adapter_marks_visible_stream_output_before_done_error() -> Result<(), String> {
        let adapter = LlmClientAdapter::new(
            Arc::new(ScriptedClient {
                events: vec![
                    Ok(LlmEvent::TextDelta {
                        delta: "partial answer".to_string(),
                        meta: None,
                    }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Error {
                            error: LlmError::ServerOverloaded,
                        },
                    }),
                ],
            }),
            "scripted-model".to_string(),
        );

        adapter.begin_stream_output_observation();
        let err = match adapter
            .stream_response(
                &[Message::User(UserMessage::text("stream then fail"))],
                &[],
                1024,
                None,
                None,
            )
            .await
        {
            Ok(_) => return Err("late done error should surface".to_string()),
            Err(err) => err,
        };

        assert!(matches!(err, AgentError::Llm { .. }));
        assert!(
            adapter.stream_output_observed(),
            "partial text delta should suppress cross-model fallback on the failed call"
        );
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn operation_observation_diagnostic_is_published_without_changing_success_or_error() {
        for fail in [false, true] {
            let operation_id = meerkat_core::OperationId::new();
            let (tx, mut rx) = mpsc::channel(16);
            let mut events = vec![Ok(LlmEvent::OperationObservationFailed {
                operation_id: operation_id.clone(),
                phase: meerkat_core::authorization::OperationObservationPhase::Outcome,
            })];
            if !fail {
                events.push(Ok(LlmEvent::TextDelta {
                    delta: "actual response".into(),
                    meta: None,
                }));
            }
            events.push(Ok(LlmEvent::Done {
                outcome: if fail {
                    LlmDoneOutcome::Error {
                        error: LlmError::ConnectionReset,
                    }
                } else {
                    LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    }
                },
            }));
            let adapter = LlmClientAdapter::with_event_channel(
                Arc::new(ScriptedClient { events }),
                "scripted-model".into(),
                tx,
            );
            adapter.begin_stream_output_observation();
            let result = adapter
                .stream_response(
                    &[Message::User(UserMessage::text("observe"))],
                    &[],
                    1024,
                    None,
                    None,
                )
                .await;
            if fail {
                assert!(matches!(result, Err(AgentError::Llm { .. })));
                assert!(
                    !adapter.stream_output_observed(),
                    "diagnostic is not model output"
                );
            } else {
                let result = result.expect("success remains success");
                assert_eq!(result.stop_reason(), StopReason::EndTurn);
                assert!(
                    matches!(result.blocks(), [AssistantBlock::Text { text, .. }] if text == "actual response")
                );
            }
            let published = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
            assert_eq!(published.iter().filter(|event| matches!(event,
                AgentEvent::OperationObservationFailed { operation_id: actual, phase: meerkat_core::authorization::OperationObservationPhase::Outcome }
                if actual == &operation_id)).count(), 1);
            assert!(!published.iter().any(|event| matches!(
                event,
                AgentEvent::Retrying { .. } | AgentEvent::RunFailed { .. }
            )));
        }
    }
}
