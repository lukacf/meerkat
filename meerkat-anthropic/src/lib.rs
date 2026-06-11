//! meerkat-anthropic — Anthropic (Claude) vertical.
//!
//! Owns the Anthropic LLM client, provider runtime, OAuth/Bedrock/
//! Vertex/Foundry backend wiring, and Anthropic request-shaping helpers.
//! Capability vocabulary lives in `meerkat_core::model_profile`; the
//! capability data rows live in `meerkat-models`.

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod client;
pub(crate) mod request_support;
pub mod runtime;
pub mod web_search;

pub use client::AnthropicClient;
pub use runtime::{AnthropicAuthMethod, AnthropicBackendKind, AnthropicProviderRuntime};
pub use web_search::AnthropicWebSearchExecutor;
