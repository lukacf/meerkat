//! Provider resolution helpers shared across interfaces.

use crate::error::LlmError;
use crate::factory::{DefaultClientFactory, DefaultFactoryConfig, LlmClientFactory, LlmProvider};
use crate::types::{LlmClient, LlmEvent, LlmRequest};
use async_trait::async_trait;
use futures::Stream;
use meerkat_core::Provider;
use std::pin::Pin;
use std::sync::Arc;

/// Resolves providers, API keys, and client instances from shared rules.
pub struct ProviderResolver;

impl ProviderResolver {
    /// Infer provider from a model string.
    pub fn infer_from_model(model: &str) -> Provider {
        let lower = model.to_lowercase();
        if lower.contains("claude") || lower.contains("anthropic") {
            Provider::Anthropic
        } else if lower.contains("gpt") || lower.contains("openai") {
            Provider::OpenAI
        } else if lower.contains("gemini") || lower.contains("google") {
            Provider::Gemini
        } else {
            Provider::Other
        }
    }

    /// Resolve API key for a provider using env vars with RKAT_* precedence.
    pub fn api_key_for(provider: Provider) -> Option<String> {
        match provider {
            Provider::Anthropic => {
                env_preferring_rkat("RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY")
            }
            Provider::OpenAI => env_preferring_rkat("RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"),
            Provider::Gemini => env_preferring_rkat("RKAT_GEMINI_API_KEY", "GEMINI_API_KEY")
                .or_else(|| std::env::var("GOOGLE_API_KEY").ok()),
            Provider::Other => None,
        }
    }

    /// Create an LLM client for the provider, optionally overriding base URL.
    pub fn client_for(provider: Provider, base_url: Option<String>) -> Arc<dyn LlmClient> {
        let Some(mapped) = map_provider(provider) else {
            return Arc::new(UnsupportedClient::new("unsupported provider"));
        };

        let mut config = DefaultFactoryConfig::default();
        if let Some(url) = base_url {
            match mapped {
                LlmProvider::Anthropic => config.anthropic_base_url = Some(url),
                LlmProvider::OpenAi => config.openai_base_url = Some(url),
                LlmProvider::Gemini => config.gemini_base_url = Some(url),
            }
        }

        let factory = DefaultClientFactory::with_config(config);
        match factory.create_client(mapped, Self::api_key_for(provider)) {
            Ok(client) => client,
            Err(err) => Arc::new(UnsupportedClient::new(&format!("{err}"))),
        }
    }
}

fn env_preferring_rkat(rkat_key: &str, provider_key: &str) -> Option<String> {
    std::env::var(rkat_key)
        .ok()
        .or_else(|| std::env::var(provider_key).ok())
}

fn map_provider(provider: Provider) -> Option<LlmProvider> {
    match provider {
        Provider::Anthropic => Some(LlmProvider::Anthropic),
        Provider::OpenAI => Some(LlmProvider::OpenAi),
        Provider::Gemini => Some(LlmProvider::Gemini),
        Provider::Other => None,
    }
}

struct UnsupportedClient {
    message: String,
}

impl UnsupportedClient {
    fn new(message: &str) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}

#[async_trait]
impl LlmClient for UnsupportedClient {
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let message = self.message.clone();
        Box::pin(futures::stream::once(async move {
            Err(LlmError::InvalidRequest { message })
        }))
    }

    fn provider(&self) -> &'static str {
        "unsupported"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Err(LlmError::InvalidRequest {
            message: self.message.clone(),
        })
    }
}
