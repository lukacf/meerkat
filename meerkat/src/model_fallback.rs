use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::error::{AgentError, LlmFailureReason};
use meerkat_core::lifecycle::run_primitive::ProviderParamsOverride;
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
}

impl ModelFallbackClient {
    pub fn new(candidates: Vec<ModelFallbackCandidate>) -> Option<Self> {
        (candidates.len() > 1).then_some(Self {
            candidates,
            active: AtomicUsize::new(0),
        })
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

    fn context_downgrade_skip_reason(
        failure: &AgentError,
        _failed: &ModelFallbackCandidate,
        next: &ModelFallbackCandidate,
    ) -> Option<String> {
        let requested = match failure {
            AgentError::Llm {
                reason: LlmFailureReason::ContextExceeded { requested, .. },
                ..
            } => *requested,
            _ => return None,
        };

        let next_window = next.target_profile.context_window()?;
        (next_window < requested).then(|| {
            format!(
                "context overflow requested {requested} tokens; {next_window}-token fallback cannot recover it"
            )
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentLlmClient for ModelFallbackClient {
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

    fn provider(&self) -> Provider {
        self.candidates[self.active_index()].identity.provider
    }

    fn model(&self) -> &str {
        &self.candidates[self.active_index()].identity.model
    }

    fn prepare_model_fallback(&self, failure: &AgentError) -> Option<AgentLlmFallbackSwitch> {
        let current_idx = self.active_index();
        let current = &self.candidates[current_idx];
        let mut skipped_targets = Vec::new();

        for next_idx in current_idx + 1..self.candidates.len() {
            let next = &self.candidates[next_idx];
            if let Some(reason) = Self::context_downgrade_skip_reason(failure, current, next) {
                skipped_targets.push(AgentLlmFallbackSkippedTarget {
                    identity: next.identity.clone(),
                    reason,
                });
                continue;
            }
            return Some(AgentLlmFallbackSwitch {
                previous_identity: current.identity.clone(),
                new_identity: next.identity.clone(),
                request_policy: next.request_policy.clone(),
                target_profile: next.target_profile.clone(),
                skipped_targets,
            });
        }

        None
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
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for ScriptedClient {
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
                provider_params: None,
                provider_tool_defaults: None,
            },
            identity,
            client: Arc::new(ScriptedClient {
                provider,
                model: model.to_string(),
                seen_tools,
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

    #[test]
    fn prepare_model_fallback_moves_to_next_candidate_after_commit() {
        let seen_tools = Arc::new(Mutex::new(Vec::new()));
        let client = ModelFallbackClient::new(vec![
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
        ])
        .expect("chain with backup");

        let switch = client
            .prepare_model_fallback(&retryable_error(Provider::OpenAI))
            .expect("backup switch");

        assert_eq!(switch.previous_identity.model, "primary");
        assert_eq!(switch.new_identity.model, "backup");
        assert_eq!(switch.target_profile.max_output_tokens(), Some(2048));
        assert_eq!(client.provider(), Provider::OpenAI);
        assert_eq!(client.model(), "primary");
        client
            .commit_model_fallback(&switch.previous_identity, &switch.new_identity)
            .expect("exact fallback candidate activation");
        assert_eq!(client.provider(), Provider::Anthropic);
        assert_eq!(client.model(), "backup");
    }

    #[test]
    fn prepare_model_fallback_skips_smaller_context_after_context_overflow() {
        let seen_tools = Arc::new(Mutex::new(Vec::new()));
        let client = ModelFallbackClient::new(vec![
            candidate(
                Provider::OpenAI,
                "large",
                Some(1_000_000),
                None,
                Arc::clone(&seen_tools),
            ),
            candidate(
                Provider::Gemini,
                "small",
                Some(128_000),
                None,
                Arc::clone(&seen_tools),
            ),
            candidate(
                Provider::Anthropic,
                "large-backup",
                Some(1_200_000),
                None,
                Arc::clone(&seen_tools),
            ),
        ])
        .expect("chain with backups");

        let switch = client
            .prepare_model_fallback(&AgentError::llm(
                Provider::OpenAI.as_str(),
                LlmFailureReason::ContextExceeded {
                    max: 1_000_000,
                    requested: 1_100_000,
                },
                "context exceeded",
            ))
            .expect("larger viable backup");

        assert_eq!(switch.new_identity.model, "large-backup");
        assert_eq!(switch.skipped_targets.len(), 1);
        assert_eq!(switch.skipped_targets[0].identity.model, "small");
    }
}
