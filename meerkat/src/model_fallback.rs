use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::error::{AgentError, LlmFailureReason};
use meerkat_core::lifecycle::run_primitive::ProviderParamsOverride;
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{
    AgentLlmClient, AgentLlmFallbackSkippedTarget, AgentLlmFallbackSwitch, LlmStreamResult,
    Provider, SessionLlmIdentity, SessionLlmRequestPolicy, ToolDef, ToolFilter,
};

pub struct ModelFallbackCandidate {
    pub identity: SessionLlmIdentity,
    pub request_policy: SessionLlmRequestPolicy,
    pub client: Arc<dyn AgentLlmClient>,
    pub capability_base_filter: ToolFilter,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
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

        let next_window = next.context_window?;
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
                capability_base_filter: next.capability_base_filter.clone(),
                context_window: next.context_window,
                max_output_tokens: next.max_output_tokens,
                skipped_targets,
            });
        }

        None
    }

    fn commit_model_fallback(&self, identity: &SessionLlmIdentity) {
        if let Some(idx) = self.candidates.iter().position(|candidate| {
            candidate.identity.model == identity.model
                && candidate.identity.provider == identity.provider
                && candidate.identity.auth_binding == identity.auth_binding
        }) {
            self.active.store(idx, Ordering::SeqCst);
        }
    }

    fn active_capability_base_filter(&self) -> ToolFilter {
        self.candidates[self.active_index()]
            .capability_base_filter
            .clone()
    }

    fn active_max_output_tokens(&self) -> Option<u32> {
        self.candidates[self.active_index()].max_output_tokens
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
    use meerkat_core::{AssistantBlock, StopReason, Usage};
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
        capability_base_filter: ToolFilter,
        context_window: Option<u32>,
        max_output_tokens: Option<u32>,
        seen_tools: Arc<Mutex<Vec<Vec<String>>>>,
    ) -> ModelFallbackCandidate {
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
            capability_base_filter,
            context_window,
            max_output_tokens,
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
                ToolFilter::All,
                Some(200_000),
                Some(4096),
                Arc::clone(&seen_tools),
            ),
            candidate(
                Provider::Anthropic,
                "backup",
                ToolFilter::Deny(["view_image".to_string()].into_iter().collect()),
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
        assert_eq!(switch.max_output_tokens, Some(2048));
        assert_eq!(client.provider(), Provider::OpenAI);
        assert_eq!(client.model(), "primary");
        client.commit_model_fallback(&switch.new_identity);
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
                ToolFilter::All,
                Some(1_000_000),
                None,
                Arc::clone(&seen_tools),
            ),
            candidate(
                Provider::SelfHosted,
                "small",
                ToolFilter::All,
                Some(128_000),
                None,
                Arc::clone(&seen_tools),
            ),
            candidate(
                Provider::Anthropic,
                "large-backup",
                ToolFilter::All,
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
