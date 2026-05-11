//! meerkat-anthropic — Anthropic (Claude) vertical.
//!
//! Owns the Anthropic LLM client, provider runtime, and OAuth/Bedrock/
//! Vertex/Foundry backend wiring. Capability tables and profile rules
//! live in `meerkat_core::model_profile` (Phase 1 of the B2 split —
//! per-provider capability/profile migration deferred).

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod client;
pub mod runtime;
pub mod web_search;

pub use client::AnthropicClient;
pub use runtime::{AnthropicAuthMethod, AnthropicBackendKind, AnthropicProviderRuntime};
pub use web_search::AnthropicWebSearchExecutor;
