//! Provider resolution helpers shared across interfaces.

use crate::error::LlmError;
use crate::factory::{DefaultClientFactory, DefaultFactoryConfig, LlmClientFactory, LlmProvider};
use crate::test_client::TestClient;
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
    ///
    /// Delegates to [`Provider::infer_from_model`] and falls back to
    /// `Provider::Other` when no prefix matches.
    pub fn infer_from_model(model: &str) -> Provider {
        Provider::infer_from_model(model).unwrap_or(Provider::Other)
    }

    /// Resolve API key for a provider using env vars with RKAT_* precedence.
    pub fn api_key_for(provider: Provider) -> Option<String> {
        Self::api_key_for_with_env(provider, |key| std::env::var(key).ok())
    }

    /// Resolve API key for a provider from an explicit environment provider.
    ///
    /// This exists primarily to make tests deterministic without mutating the process-wide
    /// environment (which is unsafe in multi-threaded programs on Unix).
    #[doc(hidden)]
    pub fn api_key_for_with_env<F>(provider: Provider, mut env: F) -> Option<String>
    where
        F: FnMut(&str) -> Option<String>,
    {
        if env("RKAT_TEST_CLIENT").as_deref() == Some("1") {
            return Some("test-key".to_string());
        }
        match provider {
            Provider::Anthropic => {
                env("RKAT_ANTHROPIC_API_KEY").or_else(|| env("ANTHROPIC_API_KEY"))
            }
            Provider::OpenAI => env("RKAT_OPENAI_API_KEY").or_else(|| env("OPENAI_API_KEY")),
            Provider::Gemini => env("RKAT_GEMINI_API_KEY")
                .or_else(|| env("GEMINI_API_KEY"))
                .or_else(|| env("GOOGLE_API_KEY")),
            Provider::Other => None,
        }
    }

    /// Create an LLM client for the provider, optionally overriding base URL.
    pub fn client_for(provider: Provider, base_url: Option<String>) -> Arc<dyn LlmClient> {
        if test_client_enabled() {
            return Arc::new(TestClient::default());
        }
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

fn test_client_enabled() -> bool {
    std::env::var("RKAT_TEST_CLIENT").ok().as_deref() == Some("1")
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
        crate::streaming::ensure_terminal_done(Box::pin(futures::stream::once(async move {
            Err(LlmError::InvalidRequest { message })
        })))
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
