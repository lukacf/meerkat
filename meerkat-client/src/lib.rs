//! meerkat-client - LLM provider abstraction for Meerkat
//!
//! This crate provides a unified interface for calling different LLM providers.
//! Each provider implementation normalizes its streaming response to the common
//! `LlmEvent` type, hiding provider-specific quirks.

pub mod error;
pub mod types;

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "openai")]
pub mod openai;

#[cfg(feature = "gemini")]
pub mod gemini;

pub use error::LlmError;
pub use types::{LlmClient, LlmEvent, LlmRequest, LlmResponse};

#[cfg(feature = "anthropic")]
pub use anthropic::AnthropicClient;

#[cfg(feature = "openai")]
pub use openai::OpenAiClient;

#[cfg(feature = "gemini")]
pub use gemini::GeminiClient;
