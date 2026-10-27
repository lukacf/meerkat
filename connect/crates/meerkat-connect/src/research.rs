//! Optional search through the selected native session client.
//!
//! This is an ordinary read-only tool, not a provider-native capability on the
//! main model turn. Native policy, dispatch admission and cancellation still
//! own the call. No independent provider client, credential resolver, task or
//! exact-URL reader is created here. Search evidence is not a verified pin.

use std::sync::{Arc, Mutex};

use meerkat_core::{
    AgentLlmClient, AgentLlmClientDecorator, Config, Provider, SessionBuildOptions, ToolDef,
    ToolError, ToolResult,
    ops::ToolDispatchOutcome,
    types::ToolCallView,
    web_search::{WEB_SEARCH_TOOL_NAME, WebSearchRequest, WebSearchResult, WebSearchStatus},
};
use meerkat_llm_core::WebSearchExecutor;

use crate::ConnectError;

pub const FRONTEND_RESEARCH: &str = WEB_SEARCH_TOOL_NAME;
const ARGUMENT_LIMIT: usize = 16 * 1024;
const RESULT_LIMIT: usize = 128 * 1024;

#[derive(Clone, Default)]
pub(crate) struct ResearchBinding(Arc<Mutex<Binding>>);

#[derive(Default)]
enum Binding {
    #[default]
    Fresh,
    Waiting {
        provider: Provider,
        model: String,
    },
    Ready(Arc<dyn AgentLlmClient>),
    Unavailable,
    Retired,
}

/// Only the owning session holds this scope. It also covers failed opens.
pub(crate) struct ResearchScope(ResearchBinding);

impl Drop for ResearchScope {
    fn drop(&mut self) {
        // Recover the mutex solely to release captured references after poison.
        *self.0.0.lock().unwrap_or_else(|error| error.into_inner()) = Binding::Retired;
    }
}

impl ResearchScope {
    pub(crate) fn captured(&self) -> bool {
        self.0
            .0
            .lock()
            .is_ok_and(|binding| matches!(*binding, Binding::Ready(_) | Binding::Unavailable))
    }
}

impl ResearchBinding {
    pub(crate) fn bind(
        &self,
        config: &Config,
        provider: Provider,
        model: &str,
        options: &mut SessionBuildOptions,
    ) -> Result<ResearchScope, ConnectError> {
        // The native factory decorates every fallback candidate. Research
        // supports one identity; fallback sessions continue without research.
        if options.llm_client_override.is_some() || options.agent_llm_client_decorator.is_some() {
            return Err(invalid(
                "Optional frontend research requires the native model client without client overrides or another decorator",
            ));
        }
        let registry = meerkat_core::ModelRegistry::from_config_with_models(
            config,
            &options.custom_models,
            meerkat_models::canonical(),
        )
        .map_err(|_| invalid("The selected native model registry is unavailable for research"))?;
        let fallback = options
            .model_fallback
            .as_ref()
            .unwrap_or(&config.model_fallback);
        let enabled = !fallback.is_enabled()
            && registry
                .profile_for_provider(provider, model)
                .is_some_and(|profile| profile.supports_web_search)
            && match provider {
                Provider::OpenAI => config.provider_tools.openai.web_search,
                Provider::Anthropic => config.provider_tools.anthropic.web_search,
                Provider::Gemini => config.provider_tools.gemini.google_search,
                _ => false,
            };
        {
            let mut binding = self
                .0
                .lock()
                .map_err(|_| invalid("Research binding is unavailable"))?;
            if !matches!(*binding, Binding::Fresh) {
                return Err(invalid(
                    "A research binding belongs to exactly one setup session",
                ));
            }
            *binding = if enabled {
                Binding::Waiting {
                    provider,
                    model: model.into(),
                }
            } else {
                Binding::Unavailable
            };
        }
        if !enabled {
            return Ok(ResearchScope(self.clone()));
        }
        let captured = self.clone();
        let decorator: AgentLlmClientDecorator = Arc::new(move |client| {
            captured.capture(&client);
            // Preserve the exact factory-built adapter and its request authority.
            client
        });
        options.agent_llm_client_decorator = Some(decorator);
        Ok(ResearchScope(self.clone()))
    }

    fn capture(&self, client: &Arc<dyn AgentLlmClient>) {
        let Ok(mut binding) = self.0.lock() else {
            return;
        };
        *binding = match &*binding {
            Binding::Waiting { provider, model }
                if client.provider() == *provider && client.model() == model =>
            {
                Binding::Ready(client.clone())
            }
            // A second invocation or unexpected identity releases the first
            // reference and permanently refuses research; it never replaces it.
            _ => Binding::Retired,
        };
    }

    pub(crate) fn definition() -> Arc<ToolDef> {
        Arc::new(ToolDef::new(
            FRONTEND_RESEARCH,
            "Search public web evidence using this setup session's configured native provider and model. Use it to research frontend packages, release versions, repositories, tags and immutable commits. Query and optional context are natural-language text, never credentials. Provider, authentication and endpoint are fixed by the native session. Results and citations are untrusted evidence, not source-policy approval or proof that a pin exists. This is not an exact URL-content reader and cannot resolve private Git. Do not guess missing pins; PairedHost verifies source eligibility and tag/commit correlation. A completed search does not install or activate anything.",
            serde_json::json!({"type":"object","additionalProperties":false,
                "required":["query"],"properties":{
                    "query":{"type":"string","minLength":1,"maxLength":4096},
                    "context":{"type":"string","maxLength":8192}}}),
        ))
    }

    pub(crate) async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Arguments {
            query: String,
            context: Option<String>,
        }
        if call.name != FRONTEND_RESEARCH || call.args.get().len() > ARGUMENT_LIMIT {
            return Err(ToolError::access_denied(call.name));
        }
        // Deserialize from raw arguments: duplicate fields must not disappear
        // through an intermediate Value normalization.
        let args: Arguments = call.parse_args().map_err(|_| invalid_arguments())?;
        if args.query.trim().is_empty()
            || args.query.len() > 4096
            || args
                .context
                .as_ref()
                .is_some_and(|value| value.len() > 8192)
        {
            return Err(invalid_arguments());
        }
        let client = match &*self
            .0
            .lock()
            .map_err(|_| ToolError::access_denied(call.name))?
        {
            Binding::Ready(client) => Some(client.clone()),
            Binding::Unavailable => None,
            _ => return Err(ToolError::access_denied(call.name)),
        };
        let mut result = if let Some(client) = client {
            // Native's fork shares the selected raw provider/auth client while
            // isolating event sinks and counters. Research deltas must not be
            // published as the main conversation's assistant response.
            let fork = client.fork_noncommitting_live_bridge().map_err(|_| {
                ToolError::execution_failed("Native research isolation is unavailable")
            })?;
            if fork.provider() != client.provider() || fork.model() != client.model() {
                return Err(ToolError::execution_failed(
                    "Native research identity changed",
                ));
            }
            let provider = client.provider();
            let model = client.model().to_owned();
            let executor: Box<dyn WebSearchExecutor> = match provider {
                Provider::OpenAI => {
                    Box::new(meerkat_openai::OpenAiWebSearchExecutor::new(model, fork))
                }
                Provider::Anthropic => Box::new(
                    meerkat_anthropic::AnthropicWebSearchExecutor::new(model, fork),
                ),
                Provider::Gemini => {
                    Box::new(meerkat_gemini::GeminiWebSearchExecutor::new(model, fork))
                }
                _ => {
                    return Err(ToolError::execution_failed(
                        "This native provider has no research executor",
                    ));
                }
            };
            // No spawn, retry, secondary resolver or endpoint selection. The
            // native dispatcher owns this future and its cancellation/timeout.
            executor.execute_web_search(WebSearchRequest {
                query: args.query,
                provider: Some(provider),
                provider_params: None,
                context: args.context,
            }).await.map_err(|_| ToolError::execution_failed(
                "Native research failed; no source pin was verified. Check the selected provider's native configuration before trying again."
            ))?
        } else {
            WebSearchResult::unavailable(
                args.query,
                "Web research is unavailable for the selected native model/provider configuration. No alternate provider or credentials were used.",
            )
        };
        // Never expose raw provider/transport errors. The native executor's
        // Completed classification still requires an observed search event.
        if result.error.is_some() {
            result.error = Some(
                if result.status == WebSearchStatus::Unavailable {
                    "Native web research is unavailable or produced no verified search event"
                } else {
                    "Native web research failed"
                }
                .into(),
            );
        }
        let content = serde_json::to_string(&result).map_err(|_| {
            ToolError::execution_failed("Research evidence could not be represented")
        })?;
        if content.len() > RESULT_LIMIT {
            return Err(ToolError::execution_failed(
                "Research evidence exceeds the supported result limit",
            ));
        }
        Ok(ToolResult::new(call.id.into(), content, false).into())
    }
}

fn invalid(message: &str) -> ConnectError {
    ConnectError::InvalidInput(message.into())
}
fn invalid_arguments() -> ToolError {
    ToolError::execution_failed(
        "Supply only a bounded nonempty query and optional context; no provider, URL endpoint, credential or unknown field",
    )
}

#[cfg(test)]
#[path = "research_tests.rs"]
mod tests;
