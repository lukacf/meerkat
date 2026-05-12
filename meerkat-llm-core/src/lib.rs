//! meerkat-llm-core — provider-neutral LLM wire client trait + plumbing.
//!
//! Owns the `LlmClient` trait, request/response/event types, error taxonomy,
//! block assembly, streaming helpers, realtime session traits, and test
//! client. Provider-specific LLM clients (Anthropic, OpenAI, Gemini) live in
//! their own crates and implement this trait.
//!
//! Deferral §3 B2 split (2026-04-18): this crate was extracted from
//! `meerkat-client` alongside per-provider crates. The `meerkat` facade
//! re-exports the public surface so downstream SDKs continue to work.

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod adapter;
pub mod block_assembler;
pub mod error;
pub mod factory;
pub mod http;
pub mod provider_runtime;
pub mod realtime_session;
pub mod streaming;
pub mod test_client;
pub mod types;

pub use adapter::LlmClientAdapter;
pub use block_assembler::{BlockAssembler, BlockKey, StreamAssemblyError};
pub use error::LlmError;
pub use factory::FactoryError;
pub use realtime_session::{
    RealtimeExternalSessionTarget, RealtimeSession, RealtimeSessionEvent, RealtimeSessionFactory,
};
pub use test_client::TestClient;
pub use types::{
    ImageGenerationExecutor, LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmResponse,
    LlmStream, ProviderGeneratedImage, ProviderImageGenerationOutput,
    ProviderImageGenerationRequest, ToolCallBuffer, WebSearchExecutor,
    dimensions_from_size_preference, media_type_from_format_preference,
    normalize_base64_image_data,
};
