//! meerkat-client - LLM provider abstraction for Meerkat
//!
//! This crate provides a unified interface for calling different LLM providers.
//! Each provider implementation normalizes its streaming response to the common
//! `LlmEvent` type, hiding provider-specific quirks.

pub mod adapter;
pub mod error;
pub mod factory;
pub mod provider;
mod streaming;
mod test_client;
pub mod types;

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "openai")]
pub mod openai;

#[cfg(feature = "gemini")]
pub mod gemini;

pub use adapter::LlmClientAdapter;
pub use error::LlmError;
pub use factory::{
    DefaultClientFactory, DefaultFactoryConfig, FactoryError, LlmClientFactory, LlmProvider,
};
pub use provider::ProviderResolver;
pub use test_client::TestClient;
pub use types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmResponse, ToolCallBuffer};

#[cfg(feature = "anthropic")]
pub use anthropic::AnthropicClient;

#[cfg(feature = "openai")]
pub use openai::OpenAiClient;

#[cfg(feature = "gemini")]
pub use gemini::GeminiClient;
