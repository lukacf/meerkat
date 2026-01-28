//! LLM client factory for creating provider-specific clients
//!
//! This module provides the [`LlmClientFactory`] trait and a default implementation
//! that can create LLM clients for different providers (Anthropic, OpenAI, Gemini).

use crate::test_client::TestClient;
use crate::types::LlmClient;
use std::sync::Arc;

/// Error types for factory operations
#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    /// The requested provider is not supported
    #[error("Unsupported provider: {0}")]
    UnsupportedProvider(String),

    /// API key is missing for the requested provider
    #[error("Missing API key for provider: {0}")]
    MissingApiKey(String),

    /// Client creation failed
    #[error("Failed to create client: {0}")]
    ClientCreationFailed(String),
}

/// Supported LLM providers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LlmProvider {
    /// Anthropic (Claude models)
    Anthropic,
    /// OpenAI (GPT models)
    OpenAi,
    /// Google (Gemini models)
    Gemini,
}

impl LlmProvider {
    /// Parse provider from string (case-insensitive)
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "anthropic" | "claude" => Some(Self::Anthropic),
            "openai" | "gpt" | "chatgpt" => Some(Self::OpenAi),
            "gemini" | "google" => Some(Self::Gemini),
            _ => None,
        }
    }

    /// Get the environment variable name for this provider's API key
    pub fn env_var(&self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
        }
    }

    /// Get the provider name as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
        }
    }
}

impl std::fmt::Display for LlmProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Trait for creating LLM clients
///
/// This trait abstracts the creation of LLM clients, allowing different
/// implementations (e.g., production vs. testing).
pub trait LlmClientFactory: Send + Sync {
    /// Create a client for the specified provider
    fn create_client(
        &self,
        provider: LlmProvider,
        api_key: Option<String>,
    ) -> Result<Arc<dyn LlmClient>, FactoryError>;

    /// Get list of supported providers
    fn supported_providers(&self) -> Vec<LlmProvider>;

    /// Check if a provider is supported
    fn is_supported(&self, provider: LlmProvider) -> bool {
        self.supported_providers().contains(&provider)
    }
}

/// Configuration for the default client factory
#[derive(Debug, Clone, Default)]
pub struct DefaultFactoryConfig {
    /// API key for Anthropic (overrides env var)
    pub anthropic_api_key: Option<String>,
    /// API key for OpenAI (overrides env var)
    pub openai_api_key: Option<String>,
    /// API key for Gemini (overrides env var)
    pub gemini_api_key: Option<String>,
    /// Custom base URL for Anthropic
    pub anthropic_base_url: Option<String>,
    /// Custom base URL for OpenAI
    pub openai_base_url: Option<String>,
    /// Custom base URL for Gemini
    pub gemini_base_url: Option<String>,
}

impl DefaultFactoryConfig {
    /// Create a new config with no pre-set keys (will use env vars)
    pub fn new() -> Self {
        Self::default()
    }

    /// Set Anthropic API key
    pub fn with_anthropic_key(mut self, key: impl Into<String>) -> Self {
        self.anthropic_api_key = Some(key.into());
        self
    }

    /// Set OpenAI API key
    pub fn with_openai_key(mut self, key: impl Into<String>) -> Self {
        self.openai_api_key = Some(key.into());
        self
    }

    /// Set Gemini API key
    pub fn with_gemini_key(mut self, key: impl Into<String>) -> Self {
        self.gemini_api_key = Some(key.into());
        self
    }

    /// Set custom Anthropic base URL
    pub fn with_anthropic_base_url(mut self, url: impl Into<String>) -> Self {
        self.anthropic_base_url = Some(url.into());
        self
    }

    /// Set custom OpenAI base URL
    pub fn with_openai_base_url(mut self, url: impl Into<String>) -> Self {
        self.openai_base_url = Some(url.into());
        self
    }

    /// Set custom Gemini base URL
    pub fn with_gemini_base_url(mut self, url: impl Into<String>) -> Self {
        self.gemini_base_url = Some(url.into());
        self
    }

    /// Get API key for provider (config override, then env var with RKAT_* precedence)
    fn get_api_key(&self, provider: LlmProvider) -> Option<String> {
        match provider {
            LlmProvider::Anthropic => self
                .anthropic_api_key
                .clone()
                .or_else(|| env_preferring_rkat("RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY")),
            LlmProvider::OpenAi => self
                .openai_api_key
                .clone()
                .or_else(|| env_preferring_rkat("RKAT_OPENAI_API_KEY", "OPENAI_API_KEY")),
            LlmProvider::Gemini => self
                .gemini_api_key
                .clone()
                .or_else(|| env_preferring_rkat("RKAT_GEMINI_API_KEY", "GEMINI_API_KEY"))
                .or_else(|| std::env::var("GOOGLE_API_KEY").ok()),
        }
    }

    /// Get base URL for provider
    fn get_base_url(&self, provider: LlmProvider) -> Option<&str> {
        match provider {
            LlmProvider::Anthropic => self.anthropic_base_url.as_deref(),
            LlmProvider::OpenAi => self.openai_base_url.as_deref(),
            LlmProvider::Gemini => self.gemini_base_url.as_deref(),
        }
    }
}

/// Default LLM client factory
///
/// Creates clients for Anthropic, OpenAI, and Gemini based on available
/// feature flags and API keys.
pub struct DefaultClientFactory {
    config: DefaultFactoryConfig,
}

impl DefaultClientFactory {
    /// Create a new factory with default configuration (uses env vars for keys)
    pub fn new() -> Self {
        Self {
            config: DefaultFactoryConfig::default(),
        }
    }

    /// Create a new factory with custom configuration
    pub fn with_config(config: DefaultFactoryConfig) -> Self {
        Self { config }
    }
}

impl Default for DefaultClientFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmClientFactory for DefaultClientFactory {
    fn create_client(
        &self,
        provider: LlmProvider,
        api_key: Option<String>,
    ) -> Result<Arc<dyn LlmClient>, FactoryError> {
        if std::env::var("RKAT_TEST_CLIENT").ok().as_deref() == Some("1") {
            return Ok(Arc::new(TestClient::default()));
        }
        // Use provided key, or fall back to config/env
        let key = api_key.or_else(|| self.config.get_api_key(provider));

        match provider {
            #[cfg(feature = "anthropic")]
            LlmProvider::Anthropic => {
                let key = key.ok_or_else(|| FactoryError::MissingApiKey("anthropic".into()))?;
                let mut client = crate::AnthropicClient::new(key)
                    .map_err(|e| FactoryError::UnsupportedProvider(e.to_string()))?;
                if let Some(base_url) = self.config.get_base_url(provider) {
                    client = client.with_base_url(base_url.to_string());
                }
                Ok(Arc::new(client))
            }
            #[cfg(not(feature = "anthropic"))]
            LlmProvider::Anthropic => Err(FactoryError::UnsupportedProvider(
                "anthropic (feature not enabled)".into(),
            )),

            #[cfg(feature = "openai")]
            LlmProvider::OpenAi => {
                let key = key.ok_or_else(|| FactoryError::MissingApiKey("openai".into()))?;
                let mut client = crate::OpenAiClient::new(key);
                if let Some(base_url) = self.config.get_base_url(provider) {
                    client = client.with_base_url(base_url.to_string());
                }
                Ok(Arc::new(client))
            }
            #[cfg(not(feature = "openai"))]
            LlmProvider::OpenAi => Err(FactoryError::UnsupportedProvider(
                "openai (feature not enabled)".into(),
            )),

            #[cfg(feature = "gemini")]
            LlmProvider::Gemini => {
                let key = key.ok_or_else(|| FactoryError::MissingApiKey("gemini".into()))?;
                let mut client = crate::GeminiClient::new(key);
                if let Some(base_url) = self.config.get_base_url(provider) {
                    client = client.with_base_url(base_url.to_string());
                }
                Ok(Arc::new(client))
            }
            #[cfg(not(feature = "gemini"))]
            LlmProvider::Gemini => Err(FactoryError::UnsupportedProvider(
                "gemini (feature not enabled)".into(),
            )),
        }
    }

    #[allow(clippy::vec_init_then_push)]
    fn supported_providers(&self) -> Vec<LlmProvider> {
        // Conditional feature-based compilation requires this pattern
        #[allow(unused_mut)]
        let mut providers = Vec::new();

        #[cfg(feature = "anthropic")]
        providers.push(LlmProvider::Anthropic);

        #[cfg(feature = "openai")]
        providers.push(LlmProvider::OpenAi);

        #[cfg(feature = "gemini")]
        providers.push(LlmProvider::Gemini);

        providers
    }
}

fn env_preferring_rkat(rkat_key: &str, provider_key: &str) -> Option<String> {
    std::env::var(rkat_key)
        .ok()
        .or_else(|| std::env::var(provider_key).ok())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_parse() {
        assert_eq!(
            LlmProvider::parse("anthropic"),
            Some(LlmProvider::Anthropic)
        );
        assert_eq!(LlmProvider::parse("claude"), Some(LlmProvider::Anthropic));
        assert_eq!(
            LlmProvider::parse("ANTHROPIC"),
            Some(LlmProvider::Anthropic)
        );

        assert_eq!(LlmProvider::parse("openai"), Some(LlmProvider::OpenAi));
        assert_eq!(LlmProvider::parse("gpt"), Some(LlmProvider::OpenAi));
        assert_eq!(LlmProvider::parse("chatgpt"), Some(LlmProvider::OpenAi));

        assert_eq!(LlmProvider::parse("gemini"), Some(LlmProvider::Gemini));
        assert_eq!(LlmProvider::parse("google"), Some(LlmProvider::Gemini));

        assert_eq!(LlmProvider::parse("unknown"), None);
    }

    #[test]
    fn test_provider_env_var() {
        assert_eq!(LlmProvider::Anthropic.env_var(), "ANTHROPIC_API_KEY");
        assert_eq!(LlmProvider::OpenAi.env_var(), "OPENAI_API_KEY");
        assert_eq!(LlmProvider::Gemini.env_var(), "GEMINI_API_KEY");
    }

    #[test]
    fn test_provider_as_str() {
        assert_eq!(LlmProvider::Anthropic.as_str(), "anthropic");
        assert_eq!(LlmProvider::OpenAi.as_str(), "openai");
        assert_eq!(LlmProvider::Gemini.as_str(), "gemini");
    }

    #[test]
    fn test_provider_display() {
        assert_eq!(format!("{}", LlmProvider::Anthropic), "anthropic");
    }

    #[test]
    fn test_factory_config_builder() {
        let config = DefaultFactoryConfig::new()
            .with_anthropic_key("test-key")
            .with_openai_base_url("https://custom.api.com");

        assert_eq!(config.anthropic_api_key, Some("test-key".to_string()));
        assert_eq!(
            config.openai_base_url,
            Some("https://custom.api.com".to_string())
        );
    }

    #[test]
    fn test_default_factory_supported_providers() {
        let factory = DefaultClientFactory::new();
        let providers = factory.supported_providers();

        // All providers should be supported when all features are enabled
        #[cfg(feature = "anthropic")]
        assert!(providers.contains(&LlmProvider::Anthropic));

        #[cfg(feature = "openai")]
        assert!(providers.contains(&LlmProvider::OpenAi));

        #[cfg(feature = "gemini")]
        assert!(providers.contains(&LlmProvider::Gemini));
    }

    #[test]
    fn test_factory_is_supported() {
        let factory = DefaultClientFactory::new();

        #[cfg(feature = "anthropic")]
        assert!(factory.is_supported(LlmProvider::Anthropic));

        #[cfg(not(feature = "anthropic"))]
        assert!(!factory.is_supported(LlmProvider::Anthropic));
    }

    #[test]
    fn test_factory_missing_api_key() {
        // Create factory with no keys configured
        let factory = DefaultClientFactory::with_config(DefaultFactoryConfig::new());

        // Temporarily unset env var to ensure we get MissingApiKey error
        let result = factory.create_client(LlmProvider::Anthropic, None);

        // Result depends on whether env var is set
        // In CI without keys, this should fail
        if std::env::var("ANTHROPIC_API_KEY").is_err() {
            #[cfg(feature = "anthropic")]
            assert!(matches!(result, Err(FactoryError::MissingApiKey(_))));
        }
    }

    #[test]
    fn test_factory_with_explicit_key() {
        let factory = DefaultClientFactory::new();

        #[cfg(feature = "anthropic")]
        {
            let result =
                factory.create_client(LlmProvider::Anthropic, Some("test-api-key".to_string()));
            // Should succeed with explicit key
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_factory_error_display() {
        let err = FactoryError::UnsupportedProvider("test".into());
        assert_eq!(err.to_string(), "Unsupported provider: test");

        let err = FactoryError::MissingApiKey("anthropic".into());
        assert_eq!(err.to_string(), "Missing API key for provider: anthropic");

        let err = FactoryError::ClientCreationFailed("timeout".into());
        assert_eq!(err.to_string(), "Failed to create client: timeout");
    }
}
