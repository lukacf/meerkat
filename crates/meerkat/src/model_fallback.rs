use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::config::ModelFallbackPolicy;
use meerkat_core::error::{AgentError, LlmFailureReason};
use meerkat_core::lifecycle::run_primitive::ProviderParamsOverride;
use meerkat_core::model_fallback::{
    ModelFallbackRequest, ModelFallbackSkipReason, admit_model_fallback, model_fallback_trigger,
};
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{
    AgentLlmClient, AgentLlmFallbackSkippedTarget, AgentLlmFallbackSwitch, LlmStreamResult,
    Provider, SessionLlmIdentity, SessionLlmRequestPolicy, ToolDef,
};

pub struct ModelFallbackCandidate {
    pub identity: SessionLlmIdentity,
    pub request_policy: SessionLlmRequestPolicy,
    pub client: Arc<dyn AgentLlmClient>,
    pub target_profile: meerkat_core::ModelProfileWitness,
}

pub struct ModelFallbackClient {
    candidates: Vec<ModelFallbackCandidate>,
    active: AtomicUsize,
    policy: ModelFallbackPolicy,
    unavailable: Vec<AgentLlmFallbackSkippedTarget>,
    auth_lease: Option<meerkat_core::handles::GeneratedAuthLeaseHandle>,
}

/// The provider-tag knobs a caller explicitly asked for: `params`' tag with
/// build-derived request defaults (`defaults`: prompt-cache policy, native
/// tool bodies), web search and structured output cleared. Web search is
/// judged by tool parity and structured output by schema compilation, so
/// neither is a tag requirement here. `None` when nothing is left.
fn provider_tag_requirements(
    params: &ProviderParamsOverride,
    defaults: Option<&meerkat_core::lifecycle::run_primitive::ProviderTag>,
) -> Option<meerkat_core::lifecycle::run_primitive::ProviderTag> {
    use meerkat_core::lifecycle::run_primitive::ProviderTag;
    let mut remaining = params.clone();
    remaining.clear_web_search();
    let mut tag = remaining.provider_tag?;
    if let Some(defaults) = defaults {
        tag.clear_matching_defaults(defaults);
    }
    let has_requirements = match &mut tag {
        ProviderTag::Anthropic(tag) => {
            tag.structured_output = None;
            *tag != Default::default()
        }
        ProviderTag::OpenAi(tag) => {
            tag.structured_output = None;
            *tag != Default::default()
        }
        ProviderTag::Gemini(tag) => {
            tag.structured_output = None;
            *tag != Default::default()
        }
        ProviderTag::Unknown { .. } => true,
    };
    has_requirements.then_some(tag)
}

/// Whether a projected provider error is the typed tool-choice refusal
/// (`LlmError::ToolChoiceUnsupported`, projected with its details class).
fn is_tool_choice_refusal(error: &meerkat_core::error::LlmProviderError) -> bool {
    error
        .details
        .get("class")
        .and_then(serde_json::Value::as_str)
        == Some(meerkat_llm_core::TOOL_CHOICE_UNSUPPORTED_DETAILS_CLASS)
}

/// Whether `target` keeps every requirement the caller placed on the failed
/// request.
///
/// `request.provider_params` is the effective per-request value: explicit
/// params with the source candidate's build-derived request defaults merged
/// in, and `target` likewise carries the target's own. Those defaults describe
/// how each provider lowers a request, not what the caller asked for (the
/// OpenAI prompt-cache key even names the model), so each side's defaults are
/// cleared before the provider tags are compared.
fn preserves_request_requirements(
    request: &ModelFallbackRequest<'_>,
    source_defaults: Option<&meerkat_core::lifecycle::run_primitive::ProviderTag>,
    target: &ProviderParamsOverride,
    target_defaults: Option<&meerkat_core::lifecycle::run_primitive::ProviderTag>,
) -> bool {
    let Some(source) = request.provider_params else {
        return true;
    };
    if source
        .temperature
        .is_some_and(|value| Some(value) != target.temperature.or(request.temperature))
        || source
            .top_p
            .is_some_and(|value| Some(value) != target.top_p)
        || source.max_output_tokens.unwrap_or(request.max_tokens)
            != target.max_output_tokens.unwrap_or(request.max_tokens)
        || source
            .reasoning
            .is_some_and(|value| Some(value) != target.reasoning)
        || source
            .thinking_budget_tokens
            .is_some_and(|value| Some(value) != target.thinking_budget_tokens)
    {
        return false;
    }
    if request.output_schema.is_none()
        && let Some(schema) = meerkat_core::model_fallback::structured_output(source)
        && Some(schema) != meerkat_core::model_fallback::structured_output(target)
    {
        return false;
    }
    match provider_tag_requirements(source, source_defaults) {
        None => true,
        Some(required) => {
            provider_tag_requirements(target, target_defaults).as_ref() == Some(&required)
        }
    }
}

impl ModelFallbackClient {
    pub fn new(
        candidates: Vec<ModelFallbackCandidate>,
        policy: ModelFallbackPolicy,
        unavailable: Vec<AgentLlmFallbackSkippedTarget>,
        auth_lease: Option<meerkat_core::handles::GeneratedAuthLeaseHandle>,
    ) -> Option<Self> {
        (!candidates.is_empty() && (candidates.len() > 1 || !unavailable.is_empty())).then_some(
            Self {
                candidates,
                active: AtomicUsize::new(0),
                policy,
                unavailable,
                auth_lease,
            },
        )
    }

    fn active_index(&self) -> usize {
        self.active
            .load(Ordering::SeqCst)
            .min(self.candidates.len().saturating_sub(1))
    }

    fn candidate_index(&self, identity: &SessionLlmIdentity) -> Option<usize> {
        self.candidates.iter().position(|candidate| {
            candidate.identity.model == identity.model
                && candidate.identity.provider == identity.provider
                && candidate.identity.self_hosted_server_id == identity.self_hosted_server_id
                && candidate.identity.provider_params == identity.provider_params
                && candidate.identity.auth_binding == identity.auth_binding
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentLlmClient for ModelFallbackClient {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.candidates[self.active_index()]
            .client
            .controller_model_selection()
    }

    fn pin_controller(self: Arc<Self>) -> Option<meerkat_core::ControllerModelClient> {
        // Read the existing selector exactly once and retain the selected
        // child. Later fallback changes cannot retarget the admitted pin.
        Arc::clone(&self.candidates[self.active_index()].client).pin_controller()
    }

    async fn stream_response_authorized(
        &self,
        messages: &[meerkat_core::Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<LlmStreamResult, AgentError> {
        self.candidates[self.active_index()]
            .client
            .stream_response_authorized(
                messages,
                tools,
                max_tokens,
                temperature,
                provider_params,
                authorization,
            )
            .await
    }

    fn prepare_request_attempt_authorized(
        self: Arc<Self>,
        messages: Arc<Vec<meerkat_core::Message>>,
        tools: Arc<[Arc<ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<ProviderParamsOverride>,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<Arc<dyn meerkat_core::AgentLlmRequestAttempt>, AgentError> {
        // Authorization refusal never advances the failure fallback policy.
        // The admitted controller/alternative owner selects any other route.
        let client = Arc::clone(&self.candidates[self.active_index()].client);
        client.prepare_request_attempt_authorized(
            messages,
            tools,
            max_tokens,
            temperature,
            provider_params,
            authorization,
        )
    }

    fn prepare_request_attempt(
        self: Arc<Self>,
        messages: Arc<Vec<meerkat_core::Message>>,
        tools: Arc<[Arc<ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<ProviderParamsOverride>,
    ) -> Result<Arc<dyn meerkat_core::AgentLlmRequestAttempt>, AgentError> {
        let client = Arc::clone(&self.candidates[self.active_index()].client);
        client.prepare_request_attempt(messages, tools, max_tokens, temperature, provider_params)
    }

    async fn stream_response(
        &self,
        messages: &[meerkat_core::Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let candidate = &self.candidates[self.active_index()];
        candidate
            .client
            .stream_response(messages, tools, max_tokens, temperature, provider_params)
            .await
    }

    fn request_pressure(
        &self,
        messages: &[meerkat_core::Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, AgentError> {
        self.candidates[self.active_index()]
            .client
            .request_pressure(messages, tools, max_tokens, temperature, provider_params)
    }

    fn target_cache_lowering_capabilities(
        &self,
        issuer: &meerkat_core::TargetCacheLoweringIssuer,
        messages: &[meerkat_core::Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<Vec<meerkat_core::TargetCacheLoweringCapability>, AgentError> {
        self.candidates[self.active_index()]
            .client
            .target_cache_lowering_capabilities(
                issuer,
                messages,
                tools,
                max_tokens,
                temperature,
                provider_params,
            )
    }

    fn request_attempt_authority(&self) -> meerkat_core::RequestAttemptAuthority {
        self.candidates[self.active_index()]
            .client
            .request_attempt_authority()
    }

    fn provider(&self) -> Provider {
        self.candidates[self.active_index()].identity.provider
    }

    fn model(&self) -> &str {
        &self.candidates[self.active_index()].identity.model
    }

    fn fork_noncommitting_live_bridge(&self) -> Result<Arc<dyn AgentLlmClient>, AgentError> {
        // The bridge is bound to the member's exact currently active model.
        // It receives no model-routing authority, so carrying inactive
        // fallback candidates would only create a second mutable selection
        // plane beside the ordinary member.
        self.candidates[self.active_index()]
            .client
            .fork_noncommitting_live_bridge()
    }

    fn prepare_model_fallback(
        &self,
        failure: &AgentError,
        request: &ModelFallbackRequest<'_>,
    ) -> Result<AgentLlmFallbackSwitch, Vec<AgentLlmFallbackSkippedTarget>> {
        if request.attempt < self.policy.trigger_after_attempts
            || !model_fallback_trigger(failure)
                .is_some_and(|trigger| self.policy.triggers.contains(&trigger))
        {
            return Err(Vec::new());
        }
        let current_idx = self.active_index();
        let current = &self.candidates[current_idx];
        let mut skipped_targets = self.unavailable.clone();

        for next_idx in current_idx + 1..self.candidates.len() {
            let next = &self.candidates[next_idx];
            match meerkat_core::model_fallback::fallback_credential_authorized(
                self.auth_lease.as_ref(),
                next.request_policy.credential_identity.as_ref(),
            ) {
                Ok(true) => {}
                result => {
                    let reason = match result {
                        Ok(false) => ModelFallbackSkipReason::AuthUnavailable,
                        _ => ModelFallbackSkipReason::AdmissionUnavailable,
                    };
                    skipped_targets.push(AgentLlmFallbackSkippedTarget::new(
                        next.identity.clone(),
                        reason,
                    ));
                    continue;
                }
            }
            // Forecast the actual request before target lowering: an obviously
            // unsafe migration must not even consult the target adapter.
            if let Err(skipped) = admit_model_fallback(
                &current.identity,
                &next.identity,
                &next.target_profile,
                &self.policy,
                request,
                None,
            ) {
                skipped_targets.push(*skipped);
                continue;
            }
            let params = meerkat_core::ProviderParamsCarrier {
                params: next
                    .request_policy
                    .provider_params
                    .clone()
                    .unwrap_or_default(),
                tool_defaults: next.request_policy.provider_tool_defaults.clone(),
            }
            .effective_params();
            let Ok(mut params) = params else {
                skipped_targets.push(AgentLlmFallbackSkippedTarget::new(
                    next.identity.clone(),
                    ModelFallbackSkipReason::RequestUnsupported,
                ));
                continue;
            };
            if !preserves_request_requirements(
                request,
                current.request_policy.provider_tool_defaults.as_ref(),
                &params,
                next.request_policy.provider_tool_defaults.as_ref(),
            ) {
                skipped_targets.push(AgentLlmFallbackSkippedTarget::new(
                    next.identity.clone(),
                    ModelFallbackSkipReason::RequestUnsupported,
                ));
                continue;
            }
            if let Some(schema) = request.output_schema.or_else(|| {
                request
                    .provider_params
                    .and_then(meerkat_core::model_fallback::structured_output)
            }) {
                let Ok(compiled) = next.client.compile_schema(schema) else {
                    skipped_targets.push(AgentLlmFallbackSkippedTarget::new(
                        next.identity.clone(),
                        ModelFallbackSkipReason::ToolParity,
                    ));
                    continue;
                };
                let Ok(schema_value) = meerkat_core::MeerkatSchema::new(compiled.schema) else {
                    skipped_targets.push(AgentLlmFallbackSkippedTarget::new(
                        next.identity.clone(),
                        ModelFallbackSkipReason::ToolParity,
                    ));
                    continue;
                };
                let mut schema = schema.clone();
                schema.schema = schema_value;
                params.clear_web_search();
                if !matches!(
                    params.set_structured_output(next.identity.provider, schema),
                    Ok(meerkat_core::lifecycle::run_primitive::StructuredOutputInjection::Injected)
                ) {
                    skipped_targets.push(AgentLlmFallbackSkippedTarget::new(
                        next.identity.clone(),
                        ModelFallbackSkipReason::ToolParity,
                    ));
                    continue;
                }
            }
            if self.policy.require_tool_parity
                && request
                    .provider_params
                    .is_some_and(meerkat_core::model_fallback::has_native_search)
                && !meerkat_core::model_fallback::has_native_search(&params)
            {
                skipped_targets.push(AgentLlmFallbackSkippedTarget::new(
                    next.identity.clone(),
                    ModelFallbackSkipReason::ToolParity,
                ));
                continue;
            }
            // Probe the target with the failed request's own tool choice, so
            // the target adapter's lowering (the one owner of what each
            // provider and model can honour) refuses a forced choice it
            // cannot serve before any provider call. Stripped again below.
            params.tool_choice = request
                .provider_params
                .and_then(|source| source.tool_choice.clone());
            let pressure = match next.client.request_pressure(
                request.messages,
                request.tools,
                request.max_tokens,
                request.temperature,
                Some(&params),
            ) {
                Ok(Some(pressure)) => pressure,
                result => {
                    let reason = match result {
                        Err(AgentError::Llm {
                            reason: LlmFailureReason::AuthError,
                            ..
                        }) => ModelFallbackSkipReason::AuthUnavailable,
                        Err(AgentError::Llm {
                            reason: LlmFailureReason::ProviderError(error),
                            ..
                        }) if is_tool_choice_refusal(&error) => {
                            ModelFallbackSkipReason::ToolChoiceUnsupported
                        }
                        Ok(None) => ModelFallbackSkipReason::AdmissionUnavailable,
                        _ => ModelFallbackSkipReason::RequestUnsupported,
                    };
                    skipped_targets.push(AgentLlmFallbackSkippedTarget::new(
                        next.identity.clone(),
                        reason,
                    ));
                    continue;
                }
            };
            if let Err(skipped) = admit_model_fallback(
                &current.identity,
                &next.identity,
                &next.target_profile,
                &self.policy,
                &ModelFallbackRequest {
                    provider_params: Some(&params),
                    ..*request
                },
                Some(pressure),
            ) {
                skipped_targets.push(*skipped);
                continue;
            }
            // A tool choice is request-local (the agent loop sets it per call
            // from the turn plan); the sticky policy the switch hands on must
            // never carry one into later requests.
            params.tool_choice = None;
            let mut request_policy = next.request_policy.clone();
            request_policy.provider_params = (!params.is_empty()).then_some(params);
            request_policy.provider_tool_defaults = None;
            return Ok(AgentLlmFallbackSwitch {
                policy: self.policy.clone(),
                previous_identity: current.identity.clone(),
                new_identity: next.identity.clone(),
                request_policy,
                target_profile: next.target_profile.clone(),
                skipped_targets,
            });
        }

        Err(skipped_targets)
    }

    fn commit_model_fallback(
        &self,
        previous_identity: &SessionLlmIdentity,
        target_identity: &SessionLlmIdentity,
    ) -> Result<(), AgentError> {
        let current_idx = self.active_index();
        if self.candidates[current_idx].identity != *previous_identity {
            return Err(AgentError::ConfigError(format!(
                "fallback client expected active identity '{}:{}' but found '{}:{}'",
                previous_identity.provider.as_str(),
                previous_identity.model,
                self.candidates[current_idx].identity.provider.as_str(),
                self.candidates[current_idx].identity.model
            )));
        }
        let target_idx = self.candidate_index(target_identity).ok_or_else(|| {
            AgentError::ConfigError(format!(
                "fallback target '{}:{}' is not an exact prebuilt candidate",
                target_identity.provider.as_str(),
                target_identity.model
            ))
        })?;
        self.active
            .compare_exchange(current_idx, target_idx, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|observed| {
                let observed = observed.min(self.candidates.len().saturating_sub(1));
                AgentError::ConfigError(format!(
                    "fallback client active candidate changed concurrently to '{}:{}'",
                    self.candidates[observed].identity.provider.as_str(),
                    self.candidates[observed].identity.model
                ))
            })?;
        Ok(())
    }

    fn active_model_fallback_identity(&self) -> Option<SessionLlmIdentity> {
        Some(self.candidates[self.active_index()].identity.clone())
    }

    fn compile_model_fallback_schema(
        &self,
        target_identity: &SessionLlmIdentity,
        output_schema: &meerkat_core::OutputSchema,
    ) -> Result<CompiledSchema, AgentError> {
        let target_idx = self.candidate_index(target_identity).ok_or_else(|| {
            AgentError::ConfigError(format!(
                "fallback target '{}:{}' is not an exact prebuilt candidate",
                target_identity.provider.as_str(),
                target_identity.model
            ))
        })?;
        self.candidates[target_idx]
            .client
            .compile_schema(output_schema)
            .map_err(|error| {
                AgentError::ConfigError(format!(
                    "fallback target '{}:{}' rejected structured output schema: {error}",
                    target_identity.provider.as_str(),
                    target_identity.model
                ))
            })
    }

    fn begin_stream_output_observation(&self) {
        self.candidates[self.active_index()]
            .client
            .begin_stream_output_observation();
    }

    fn stream_output_observed(&self) -> bool {
        self.candidates[self.active_index()]
            .client
            .stream_output_observed()
    }

    fn stream_activity_count(&self) -> Option<u64> {
        // Without this forwarder the stall watchdog silently disengages on
        // every factory-built agent (fallback wraps the adapter by default).
        self.candidates[self.active_index()]
            .client
            .stream_activity_count()
    }

    fn compile_schema(
        &self,
        output_schema: &meerkat_core::OutputSchema,
    ) -> Result<CompiledSchema, SchemaError> {
        self.candidates[self.active_index()]
            .client
            .compile_schema(output_schema)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::error::{LlmProviderError, LlmProviderErrorKind};
    use meerkat_core::{AssistantBlock, ModelCatalog, StopReason, Usage};
    use tokio::sync::Mutex;

    struct ScriptedClient {
        provider: Provider,
        model: String,
        seen_tools: Arc<Mutex<Vec<Vec<String>>>>,
        pressure_available: bool,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for ScriptedClient {
        fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
            let binding = meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse("fixture-realm").unwrap(),
                binding: meerkat_core::BindingId::parse("fixture-binding").unwrap(),
                profile: None,
                origin: meerkat_core::BindingOrigin::Configured,
            };
            Some(meerkat_core::ControllerModelSelection::new(
                SessionLlmIdentity {
                    model: self.model.clone(),
                    provider: self.provider,
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: Some(binding.clone()),
                },
                meerkat_core::AuthCredentialIdentity::Binding(binding),
                "fixture-profile".to_owned(),
                self.provider.as_str().to_owned(),
            ))
        }

        fn pin_controller(self: Arc<Self>) -> Option<meerkat_core::ControllerModelClient> {
            Some(meerkat_core::ControllerModelClient::new(
                self.controller_model_selection()?,
                self,
            ))
        }

        fn request_pressure(
            &self,
            _messages: &[meerkat_core::Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&ProviderParamsOverride>,
        ) -> Result<Option<meerkat_core::ProviderRequestPressure>, AgentError> {
            Ok(self.pressure_available.then(|| {
                meerkat_core::ProviderRequestPressure::new(
                    123,
                    meerkat_models::approximate_request_byte_cap(self.provider),
                )
            }))
        }

        async fn stream_response(
            &self,
            _messages: &[meerkat_core::Message],
            tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&ProviderParamsOverride>,
        ) -> Result<LlmStreamResult, AgentError> {
            self.seen_tools.lock().await.push(
                tools
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect::<Vec<_>>(),
            );
            Ok(LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "ok".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                Usage::default(),
            ))
        }

        fn provider(&self) -> Provider {
            self.provider
        }

        fn model(&self) -> &str {
            &self.model
        }
    }

    fn candidate(
        provider: Provider,
        model: &str,
        context_window: Option<u32>,
        max_output_tokens: Option<u32>,
        seen_tools: Arc<Mutex<Vec<Vec<String>>>>,
    ) -> ModelFallbackCandidate {
        let mut config = meerkat_core::Config::default();
        config.models.custom.insert(
            model.to_string(),
            meerkat_core::config::CustomModelConfig {
                provider,
                display_name: None,
                context_window,
                max_input_tokens: None,
                max_output_tokens,
                vision: Some(false),
                web_search: Some(false),
                call_timeout_secs: None,
            },
        );
        let empty_catalog = ModelCatalog {
            entries: &[],
            capabilities: &[],
            provider_defaults: &[],
            image_generation_models: &[],
            providers: &[],
            default_models: &[],
            image_generation_defaults: &[],
            global_default_model: "",
            provider_priority: &[],
        };
        let target_profile = meerkat_core::ModelRegistry::from_config(&config, empty_catalog)
            .expect("fallback test registry")
            .profile_witness_for_provider(provider, model)
            .expect("registered fallback test profile");
        let identity = SessionLlmIdentity {
            model: model.to_string(),
            provider,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        ModelFallbackCandidate {
            request_policy: SessionLlmRequestPolicy {
                model: model.to_string(),
                credential_identity: None,
                provider_params: None,
                provider_tool_defaults: None,
                provider_native_tools: meerkat_core::ProviderNativeToolPolicy::Inherit,
            },
            identity,
            client: Arc::new(ScriptedClient {
                provider,
                model: model.to_string(),
                seen_tools,
                pressure_available: true,
            }),
            target_profile,
        }
    }

    fn retryable_error(provider: Provider) -> AgentError {
        AgentError::llm(
            provider.as_str(),
            LlmFailureReason::ProviderError(LlmProviderError::retryable(
                LlmProviderErrorKind::ServerOverloaded,
                serde_json::json!({"message": "busy"}),
            )),
            "busy",
        )
    }

    fn request(messages: &[meerkat_core::Message]) -> ModelFallbackRequest<'_> {
        ModelFallbackRequest {
            messages,
            tools: &[],
            max_tokens: 1024,
            temperature: None,
            provider_params: None,
            output_schema: None,
            attempt: 3,
        }
    }

    #[tokio::test]
    async fn controller_pin_keeps_original_runnable_child_after_fallback_commit() {
        let primary_seen = Arc::default();
        let target_seen = Arc::default();
        let primary = candidate(
            Provider::OpenAI,
            "primary",
            Some(1_000_000),
            Some(8192),
            Arc::clone(&primary_seen),
        );
        let target = candidate(
            Provider::OpenAI,
            "target",
            Some(1_000_000),
            Some(8192),
            Arc::clone(&target_seen),
        );
        let previous = primary.identity.clone();
        let next = target.identity.clone();
        let client = Arc::new(
            ModelFallbackClient::new(vec![primary, target], Default::default(), Vec::new(), None)
                .unwrap(),
        );
        let pinned = Arc::clone(&client).pin_controller().unwrap();
        assert_eq!(pinned.selection().model(), "primary");
        client.commit_model_fallback(&previous, &next).unwrap();
        assert_eq!(
            client.controller_model_selection().unwrap().model(),
            "target"
        );
        assert_eq!(pinned.client().model(), "primary");
        pinned
            .client()
            .stream_response(&[], &[], 64, None, None)
            .await
            .unwrap();
        assert_eq!(primary_seen.lock().await.len(), 1);
        assert!(target_seen.lock().await.is_empty());
    }

    #[test]
    fn work_context_requires_pin_to_match_admitted_selection_and_actual_client() {
        struct Admitted(meerkat_core::ControllerModelSelection);
        impl meerkat_core::authorization::WorkAuthorization for Admitted {
            fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
                Some(self.0.clone())
            }
            fn prepare(
                &self,
                _: &meerkat_core::authorization::PreparedAuthorizationBinding,
            ) -> Result<
                Arc<dyn meerkat_core::authorization::PreparedOperationAuthorization>,
                meerkat_core::OperationAuthorizationError,
            > {
                Err(meerkat_core::authorization::OperationRefused::new(
                    meerkat_core::authorization::OperationRefusalKind::Denied,
                )
                .into())
            }
        }
        let primary = candidate(
            Provider::OpenAI,
            "primary",
            Some(1_000_000),
            Some(8192),
            Arc::default(),
        );
        let target = candidate(
            Provider::OpenAI,
            "target",
            Some(1_000_000),
            Some(8192),
            Arc::default(),
        );
        let pin = Arc::clone(&primary.client).pin_controller().unwrap();
        let work = meerkat_core::authorization::WorkAuthorizationContext::new(
            Arc::new(Admitted(pin.selection().clone())),
            meerkat_core::exact_operation::OperationExecutionScope::Domain,
        );
        assert!(work.controller_client().is_none());
        let attached = work.clone().with_controller_client(pin.clone()).unwrap();
        assert!(Arc::ptr_eq(
            attached.controller_client().unwrap().client(),
            pin.client()
        ));
        assert!(!attached.same_context(&work));
        let wrong = Arc::clone(&target.client).pin_controller().unwrap();
        assert!(work.clone().with_controller_client(wrong.clone()).is_err());
        // Even a manually paired DTO and a different actual client cannot
        // satisfy the supported attachment path.
        let forged = meerkat_core::ControllerModelClient::new(
            pin.selection().clone(),
            Arc::clone(wrong.client()),
        );
        assert!(work.with_controller_client(forged).is_err());
    }

    #[test]
    fn model_fallback_missing_request_pressure_fails_closed() {
        let primary = candidate(
            Provider::OpenAI,
            "primary",
            Some(1_000_000),
            Some(8192),
            Arc::default(),
        );
        let mut target = candidate(
            Provider::OpenAI,
            "target",
            Some(1_000_000),
            Some(8192),
            Arc::default(),
        );
        target.client = Arc::new(ScriptedClient {
            provider: Provider::OpenAI,
            model: "target".into(),
            seen_tools: Arc::default(),
            pressure_available: false,
        });
        let client =
            ModelFallbackClient::new(vec![primary, target], Default::default(), Vec::new(), None)
                .unwrap();
        let skipped = client
            .prepare_model_fallback(&retryable_error(Provider::OpenAI), &request(&[]))
            .unwrap_err();
        assert_eq!(
            skipped[0].reason,
            ModelFallbackSkipReason::AdmissionUnavailable
        );
        assert_eq!(client.model(), "primary");
    }

    fn cross_provider_policy() -> ModelFallbackPolicy {
        ModelFallbackPolicy {
            cross_provider: true,
            ..Default::default()
        }
    }

    #[test]
    fn model_fallback_preserves_tools_modalities_and_admitted_extraction_params() {
        let client = ModelFallbackClient::new(
            vec![
                candidate(
                    Provider::OpenAI,
                    "primary",
                    Some(1_000_000),
                    Some(8192),
                    Arc::default(),
                ),
                candidate(
                    Provider::Anthropic,
                    "target",
                    Some(200_000),
                    Some(8192),
                    Arc::default(),
                ),
            ],
            cross_provider_policy(),
            Vec::new(),
            None,
        )
        .unwrap();
        let failure = retryable_error(Provider::OpenAI);
        let tools = [Arc::new(ToolDef::new(
            meerkat_core::VIEW_IMAGE_TOOL_NAME,
            "read an image",
            serde_json::json!({"type":"object"}),
        ))];
        let rejected = client
            .prepare_model_fallback(
                &failure,
                &ModelFallbackRequest {
                    tools: &tools,
                    ..request(&[])
                },
            )
            .unwrap_err();
        assert_eq!(rejected[0].reason, ModelFallbackSkipReason::ToolParity);
        let content = vec![meerkat_core::ContentBlock::Image {
            media_type: "image/png".into(),
            data: "aW1hZ2U=".into(),
        }];
        let messages = [meerkat_core::Message::SystemNotice(
            meerkat_core::SystemNoticeMessage::with_block(
                meerkat_core::SystemNoticeKind::Generic,
                None,
                meerkat_core::SystemNoticeBlock::ExternalEvent {
                    source: "fixture".into(),
                    event_type: "image".into(),
                    summary: None,
                    body: None,
                    payload: None,
                    content,
                },
            ),
        )];
        let rejected = client
            .prepare_model_fallback(&failure, &request(&messages))
            .unwrap_err();
        assert_eq!(rejected[0].reason, ModelFallbackSkipReason::ModalityParity);
        let schema = meerkat_core::OutputSchema::new(serde_json::json!({
            "type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],
        }))
        .unwrap();
        let switch = client
            .prepare_model_fallback(
                &failure,
                &ModelFallbackRequest {
                    output_schema: Some(&schema),
                    ..request(&[])
                },
            )
            .unwrap();
        let admitted = switch.request_policy.provider_params.as_ref().unwrap();
        assert_eq!(
            meerkat_core::model_fallback::structured_output(admitted),
            Some(&schema)
        );
        assert!(!meerkat_core::model_fallback::has_native_search(admitted));
        assert!(switch.request_policy.provider_tool_defaults.is_none());
    }

    /// The switch's sticky request policy never carries a tool choice, even
    /// when the failed request and the target's in-process policy did: the
    /// choice is request-local and set per call by the agent loop.
    #[test]
    fn model_fallback_switch_policy_never_carries_a_tool_choice() {
        let mut target = candidate(
            Provider::Anthropic,
            "target",
            Some(200_000),
            Some(8192),
            Arc::default(),
        );
        target.request_policy.provider_params = Some(ProviderParamsOverride {
            temperature: Some(0.3),
            tool_choice: Some(meerkat_core::ToolChoice::Required),
            ..Default::default()
        });
        let client = ModelFallbackClient::new(
            vec![
                candidate(
                    Provider::OpenAI,
                    "primary",
                    Some(1_000_000),
                    Some(8192),
                    Arc::default(),
                ),
                target,
            ],
            cross_provider_policy(),
            Vec::new(),
            None,
        )
        .unwrap();
        let forced = ProviderParamsOverride {
            tool_choice: Some(meerkat_core::ToolChoice::Tool {
                name: "deny_probe".into(),
            }),
            ..Default::default()
        };
        let switch = client
            .prepare_model_fallback(
                &retryable_error(Provider::OpenAI),
                &ModelFallbackRequest {
                    provider_params: Some(&forced),
                    ..request(&[])
                },
            )
            .unwrap();
        let sticky = switch
            .request_policy
            .provider_params
            .as_ref()
            .expect("the target's own params survive");
        assert_eq!(sticky.temperature, Some(0.3));
        assert_eq!(
            sticky.tool_choice, None,
            "no tool choice sticks into fallback"
        );
    }

    /// A fallback target backed by the real Anthropic adapter, so admission
    /// consults the adapter's own tool-choice lowering.
    #[cfg(feature = "anthropic")]
    fn anthropic_adapter_candidate(model: &str) -> ModelFallbackCandidate {
        let mut target = candidate(
            Provider::Anthropic,
            model,
            Some(200_000),
            Some(8192),
            Arc::default(),
        );
        target.client = Arc::new(
            meerkat_llm_core::LlmClientAdapter::try_for_provider_identity(
                Arc::new(meerkat_anthropic::AnthropicClient::new("test-key".to_string()).unwrap()),
                model.to_string(),
                Provider::Anthropic,
            )
            .unwrap(),
        );
        target
    }

    /// Admission asks the target adapter whether it can honour the failed
    /// request's forced tool choice. A target that refuses it (Claude Opus
    /// 5.5) is skipped with the typed reason before any provider call; the
    /// same target is admitted for an unforced request, and a target that
    /// accepts the choice is admitted without carrying it into its policy.
    #[cfg(feature = "anthropic")]
    #[test]
    fn model_fallback_skips_targets_that_cannot_honour_a_forced_tool_choice() {
        let primary = || {
            candidate(
                Provider::OpenAI,
                "primary",
                Some(1_000_000),
                Some(8192),
                Arc::default(),
            )
        };
        let tools = [Arc::new(ToolDef::new(
            "deny_probe",
            "probe",
            serde_json::json!({"type": "object", "properties": {}}),
        ))];
        let forced = ProviderParamsOverride {
            tool_choice: Some(meerkat_core::ToolChoice::Tool {
                name: "deny_probe".into(),
            }),
            ..Default::default()
        };
        let forced_request = ModelFallbackRequest {
            tools: &tools,
            provider_params: Some(&forced),
            ..request(&[])
        };
        let failure = retryable_error(Provider::OpenAI);

        let refusing = ModelFallbackClient::new(
            vec![primary(), anthropic_adapter_candidate("claude-opus-5-5")],
            cross_provider_policy(),
            Vec::new(),
            None,
        )
        .unwrap();
        let skipped = refusing
            .prepare_model_fallback(&failure, &forced_request)
            .unwrap_err();
        assert_eq!(skipped.len(), 1);
        assert_eq!(
            skipped[0].reason,
            ModelFallbackSkipReason::ToolChoiceUnsupported
        );
        // The same target is admitted when the request forces nothing.
        refusing
            .prepare_model_fallback(
                &failure,
                &ModelFallbackRequest {
                    tools: &tools,
                    ..request(&[])
                },
            )
            .expect("an unforced request may fall back to Opus 5.5");

        let accepting = ModelFallbackClient::new(
            vec![primary(), anthropic_adapter_candidate("claude-sonnet-4-6")],
            cross_provider_policy(),
            Vec::new(),
            None,
        )
        .unwrap();
        let switch = accepting
            .prepare_model_fallback(&failure, &forced_request)
            .expect("a target that honours the forced choice is admitted");
        assert!(
            switch
                .request_policy
                .provider_params
                .as_ref()
                .is_none_or(|params| params.tool_choice.is_none()),
            "the admitted policy never carries the request's tool choice"
        );
    }

    #[test]
    fn model_fallback_default_trigger_threshold_and_provider_boundary() {
        let clients = || {
            vec![
                candidate(
                    Provider::OpenAI,
                    "primary",
                    Some(1_000_000),
                    Some(8192),
                    Arc::default(),
                ),
                candidate(
                    Provider::Anthropic,
                    "target",
                    Some(200_000),
                    Some(8192),
                    Arc::default(),
                ),
            ]
        };
        let client =
            ModelFallbackClient::new(clients(), Default::default(), Vec::new(), None).unwrap();
        let capacity = retryable_error(Provider::OpenAI);
        assert!(
            client
                .prepare_model_fallback(
                    &capacity,
                    &ModelFallbackRequest {
                        attempt: 2,
                        ..request(&[])
                    }
                )
                .unwrap_err()
                .is_empty()
        );
        let rejected = client
            .prepare_model_fallback(&capacity, &request(&[]))
            .unwrap_err();
        assert_eq!(
            rejected[0].reason,
            ModelFallbackSkipReason::ProviderBoundary
        );
        for kind in [
            LlmProviderErrorKind::ConnectionReset,
            LlmProviderErrorKind::IncompleteResponse,
        ] {
            let error = AgentError::llm(
                "openai",
                LlmFailureReason::ProviderError(LlmProviderError::retryable(
                    kind,
                    serde_json::json!({"message":"test"}),
                )),
                "test",
            );
            assert!(
                client
                    .prepare_model_fallback(&error, &request(&[]))
                    .unwrap_err()
                    .is_empty()
            );
        }
        let enabled =
            ModelFallbackClient::new(clients(), cross_provider_policy(), Vec::new(), None).unwrap();
        assert!(
            enabled
                .prepare_model_fallback(&capacity, &request(&[]))
                .is_ok()
        );
    }

    #[test]
    fn model_fallback_does_not_drop_output_or_reasoning_requirements() {
        let client = ModelFallbackClient::new(
            vec![
                candidate(
                    Provider::OpenAI,
                    "primary",
                    Some(1_000_000),
                    Some(8192),
                    Arc::default(),
                ),
                candidate(
                    Provider::Anthropic,
                    "target",
                    Some(200_000),
                    Some(2048),
                    Arc::default(),
                ),
            ],
            cross_provider_policy(),
            Vec::new(),
            None,
        )
        .unwrap();
        let rejected = client
            .prepare_model_fallback(
                &retryable_error(Provider::OpenAI),
                &ModelFallbackRequest {
                    max_tokens: 4096,
                    ..request(&[])
                },
            )
            .unwrap_err();
        assert_eq!(rejected[0].reason, ModelFallbackSkipReason::OutputBudget);
        let params = ProviderParamsOverride {
            thinking_budget_tokens: Some(1024),
            ..Default::default()
        };
        let rejected = client
            .prepare_model_fallback(
                &retryable_error(Provider::OpenAI),
                &ModelFallbackRequest {
                    provider_params: Some(&params),
                    ..request(&[])
                },
            )
            .unwrap_err();
        assert_eq!(
            rejected[0].reason,
            ModelFallbackSkipReason::RequestUnsupported
        );
    }

    /// The source candidate's build-derived request defaults (the GPT-6
    /// prompt-cache policy and its model-named cache key) are merged into the
    /// effective request params, but they are not caller requirements: a
    /// cross-provider target stays admissible, same-provider targets with
    /// their own defaults stay admissible, and an explicit caller knob still
    /// blocks a target that cannot carry it.
    #[test]
    fn model_fallback_ignores_source_request_defaults_but_keeps_caller_requirements() {
        use meerkat_core::lifecycle::run_primitive::{
            OpenAiPromptCacheOptions, OpenAiProviderTag, ProviderParamsCarrier, ProviderTag,
        };
        use meerkat_core::model_profile::capabilities::{
            OpenAiPromptCacheMode, OpenAiPromptCacheTtl,
        };
        let cache_defaults = |model: &str| {
            ProviderTag::OpenAi(OpenAiProviderTag {
                prompt_cache_enabled: Some(true),
                prompt_cache_key: Some(format!("meerkat:profile:openai:{model}")),
                prompt_cache_options: Some(OpenAiPromptCacheOptions {
                    mode: Some(OpenAiPromptCacheMode::Implicit),
                    ttl: Some(OpenAiPromptCacheTtl::ThirtyMinutes),
                }),
                ..Default::default()
            })
        };
        let with_defaults = |mut candidate: ModelFallbackCandidate| {
            candidate.request_policy.provider_tool_defaults =
                Some(cache_defaults(&candidate.identity.model));
            candidate
        };
        let effective = |explicit: ProviderParamsOverride| {
            ProviderParamsCarrier {
                params: explicit,
                tool_defaults: Some(cache_defaults("primary")),
            }
            .effective_params()
            .expect("same-family defaults merge")
        };
        let primary = || {
            with_defaults(candidate(
                Provider::OpenAI,
                "primary",
                Some(200_000),
                Some(4096),
                Arc::default(),
            ))
        };

        let cross = ModelFallbackClient::new(
            vec![
                primary(),
                candidate(
                    Provider::Anthropic,
                    "target",
                    Some(200_000),
                    Some(4096),
                    Arc::default(),
                ),
            ],
            cross_provider_policy(),
            Vec::new(),
            None,
        )
        .unwrap();
        let defaults_only = effective(ProviderParamsOverride::default());
        let switch = cross
            .prepare_model_fallback(
                &retryable_error(Provider::OpenAI),
                &ModelFallbackRequest {
                    provider_params: Some(&defaults_only),
                    ..request(&[])
                },
            )
            .expect("source cache defaults do not block a cross-provider target");
        assert_eq!(switch.new_identity.provider, Provider::Anthropic);

        let seeded = effective(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                seed: Some(7),
                ..Default::default()
            })),
            ..Default::default()
        });
        let rejected = cross
            .prepare_model_fallback(
                &retryable_error(Provider::OpenAI),
                &ModelFallbackRequest {
                    provider_params: Some(&seeded),
                    ..request(&[])
                },
            )
            .unwrap_err();
        assert_eq!(
            rejected[0].reason,
            ModelFallbackSkipReason::RequestUnsupported,
            "an explicit caller knob the target cannot carry still blocks"
        );

        let same = ModelFallbackClient::new(
            vec![
                primary(),
                with_defaults(candidate(
                    Provider::OpenAI,
                    "target",
                    Some(200_000),
                    Some(4096),
                    Arc::default(),
                )),
            ],
            Default::default(),
            Vec::new(),
            None,
        )
        .unwrap();
        let switch = same
            .prepare_model_fallback(
                &retryable_error(Provider::OpenAI),
                &ModelFallbackRequest {
                    provider_params: Some(&defaults_only),
                    ..request(&[])
                },
            )
            .expect("a same-provider target brings its own cache defaults");
        assert_eq!(switch.new_identity.model, "target");
    }

    #[test]
    fn prepare_model_fallback_moves_to_next_candidate_after_commit() {
        let seen_tools = Arc::new(Mutex::new(Vec::new()));
        let client = ModelFallbackClient::new(
            vec![
                candidate(
                    Provider::OpenAI,
                    "primary",
                    Some(200_000),
                    Some(4096),
                    Arc::clone(&seen_tools),
                ),
                candidate(
                    Provider::Anthropic,
                    "backup",
                    Some(200_000),
                    Some(2048),
                    Arc::clone(&seen_tools),
                ),
            ],
            cross_provider_policy(),
            Vec::new(),
            None,
        )
        .expect("chain with backup");

        let switch = client
            .prepare_model_fallback(&retryable_error(Provider::OpenAI), &request(&[]))
            .expect("backup switch");

        assert_eq!(switch.previous_identity.model, "primary");
        assert_eq!(switch.new_identity.model, "backup");
        assert_eq!(switch.target_profile.max_output_tokens(), Some(2048));
        assert_eq!(client.provider(), Provider::OpenAI);
        assert_eq!(client.model(), "primary");
        assert_eq!(
            client
                .request_pressure(&[], &[], 1, None, None)
                .expect("primary pressure")
                .expect("primary witness")
                .max_bytes,
            meerkat_models::approximate_request_byte_cap(Provider::OpenAI)
        );
        client
            .commit_model_fallback(&switch.previous_identity, &switch.new_identity)
            .expect("exact fallback candidate activation");
        assert_eq!(client.provider(), Provider::Anthropic);
        assert_eq!(client.model(), "backup");
        assert_eq!(
            client
                .request_pressure(&[], &[], 1, None, None)
                .expect("fallback pressure")
                .expect("fallback witness")
                .max_bytes,
            meerkat_models::approximate_request_byte_cap(Provider::Anthropic),
            "request pressure must follow the active fallback candidate"
        );
    }

    #[test]
    fn prepare_model_fallback_skips_actual_large_context_on_capacity() {
        let seen_tools = Arc::new(Mutex::new(Vec::new()));
        let client = ModelFallbackClient::new(
            vec![
                candidate(
                    Provider::OpenAI,
                    "large",
                    Some(1_000_000),
                    Some(4096),
                    Arc::clone(&seen_tools),
                ),
                candidate(
                    Provider::Gemini,
                    "small",
                    Some(128_000),
                    Some(4096),
                    Arc::clone(&seen_tools),
                ),
                candidate(
                    Provider::Anthropic,
                    "large-backup",
                    Some(1_200_000),
                    Some(4096),
                    Arc::clone(&seen_tools),
                ),
            ],
            cross_provider_policy(),
            Vec::new(),
            None,
        )
        .expect("chain with backups");

        let messages = [meerkat_core::Message::User(
            meerkat_core::UserMessage::text("word".repeat(650_000)),
        )];
        let switch = client
            .prepare_model_fallback(&retryable_error(Provider::OpenAI), &request(&messages))
            .expect("larger viable backup");

        assert_eq!(switch.new_identity.model, "large-backup");
        assert_eq!(switch.skipped_targets.len(), 1);
        assert_eq!(switch.skipped_targets[0].identity.model, "small");
        assert_eq!(
            switch.skipped_targets[0].reason,
            ModelFallbackSkipReason::ContextFit
        );
        assert!(
            switch.skipped_targets[0]
                .context
                .as_ref()
                .unwrap()
                .effective_input_tokens()
                >= 650_000
        );
    }

    #[test]
    fn observation_infrastructure_never_selects_fallback_but_capacity_still_can() {
        let client = ModelFallbackClient::new(
            vec![
                candidate(
                    Provider::OpenAI,
                    "primary",
                    Some(200_000),
                    Some(4096),
                    Arc::default(),
                ),
                candidate(
                    Provider::OpenAI,
                    "backup",
                    Some(200_000),
                    Some(4096),
                    Arc::default(),
                ),
            ],
            Default::default(),
            Vec::new(),
            None,
        )
        .expect("real two-candidate fallback owner");
        let original = client.active_model_fallback_identity().unwrap();
        for retryability in [
            meerkat_core::error::LlmProviderErrorRetryability::NonRetryable,
            meerkat_core::error::LlmProviderErrorRetryability::Retryable,
        ] {
            let error = AgentError::llm(
                "openai",
                LlmFailureReason::ProviderError(LlmProviderError::new(
                    LlmProviderErrorKind::OperationObservationUnavailable,
                    retryability,
                    serde_json::json!({"message": "capacity"}),
                )),
                "safe observation failure",
            );
            let rejected = client
                .prepare_model_fallback(&error, &request(&[]))
                .expect_err("infrastructure failure must not select any candidate");
            assert!(
                rejected.is_empty(),
                "no candidate probing is required for this class"
            );
            assert_eq!(
                client.active_model_fallback_identity().as_ref(),
                Some(&original)
            );
        }

        // Same configuration, threshold and candidates can admit and commit an
        // ordinary capacity fallback. The negative is not a broken fixture.
        let switch = client
            .prepare_model_fallback(&retryable_error(Provider::OpenAI), &request(&[]))
            .expect("ordinary capacity fallback remains supported");
        assert_eq!(switch.previous_identity.model, "primary");
        assert_eq!(switch.new_identity.model, "backup");
        client
            .commit_model_fallback(&switch.previous_identity, &switch.new_identity)
            .expect("positive-control owner transition");
        assert_eq!(
            client.active_model_fallback_identity().unwrap().model,
            "backup"
        );
    }
}
