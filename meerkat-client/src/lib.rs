#![cfg_attr(test, allow(clippy::panic))]
//! meerkat-client - LLM provider abstraction for Meerkat
//!
//! This crate provides a unified interface for calling different LLM providers.
//! Each provider implementation normalizes its streaming response to the common
//! `LlmEvent` type, hiding provider-specific quirks.

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod adapter;
pub mod block_assembler;
pub mod error;
pub mod factory;
mod http;
pub mod realtime_session;
mod streaming;
mod test_client;
pub mod types;

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "openai")]
pub mod openai;
#[cfg(feature = "openai")]
pub mod openai_compatible;
#[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
pub mod openai_live;
#[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
pub mod openai_realtime_attachment;

#[cfg(feature = "gemini")]
pub mod gemini;

// === Provider runtime: typed backend + auth-method matrix routed through
// AgentFactory when `AgentBuildConfig.connection_ref` is set. Single
// canonical path for provider-client construction; the legacy flat
// `build_llm_client(provider, api_key, base_url)` entry point was removed
// in Phase 6. ===
pub mod providers;
pub mod runtime;

// === Token storage + refresh coordination + OAuth helpers (Phase 4a) ===
// Non-wasm by construction: filesystem, keyring, and OS lockfile primitives
// are not available in the browser. WASM-facing auth uses the external
// resolver seam (Phase 4d.wasm).
#[cfg(not(target_arch = "wasm32"))]
pub mod auth_oauth;
#[cfg(not(target_arch = "wasm32"))]
pub mod auth_store;
#[cfg(not(target_arch = "wasm32"))]
pub mod authorizers;

pub use adapter::LlmClientAdapter;
pub use block_assembler::{BlockAssembler, BlockKey, StreamAssemblyError};
pub use error::LlmError;
pub use factory::FactoryError;
pub use realtime_session::{
    RealtimeExternalSessionTarget, RealtimeSession, RealtimeSessionEvent, RealtimeSessionFactory,
};
pub use test_client::TestClient;
pub use types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmResponse, ToolCallBuffer};

#[cfg(feature = "anthropic")]
pub use anthropic::AnthropicClient;

#[cfg(feature = "openai")]
pub use openai::OpenAiClient;
#[cfg(feature = "openai")]
pub use openai_compatible::{OpenAiCompatibleClient, OpenAiCompatibleMode};
#[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
pub use openai_live::{
    OpenAiLiveCallTarget, OpenAiLiveClient, OpenAiLiveClientEvent, OpenAiLiveServerEvent,
    OpenAiLiveSession, OpenAiLiveSessionFactory, OpenAiRealtimeSession,
    OpenAiRealtimeSessionFactory, openai_live_function_call_error_event,
    openai_live_function_call_success_events, pump_openai_live_session,
};
#[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
pub use openai_realtime_attachment::{
    OpenAiRealtimeAttachmentOrchestrator, RealtimeAttachmentToolDispatchHost,
};

#[cfg(feature = "gemini")]
pub use gemini::GeminiClient;

// Provider-runtime re-exports (Phase 2).
pub use runtime::{
    DynamicLease, ExternalAuthResolverHandle, NormalizedAuthMethod, NormalizedBackendKind,
    ProviderAuthError, ProviderBindingError, ProviderClientError, ProviderRuntime,
    ProviderRuntimeRegistry, ResolvedConnection, ResolverEnvironment, StaticLease,
    ValidatedBinding,
};

#[cfg(feature = "anthropic")]
pub use providers::anthropic::{
    AnthropicAuthMethod, AnthropicBackendKind, AnthropicProviderRuntime,
};
#[cfg(feature = "gemini")]
pub use providers::google::{GoogleAuthMethod, GoogleBackendKind, GoogleProviderRuntime};
#[cfg(feature = "openai")]
pub use providers::openai::{OpenAiAuthMethod, OpenAiBackendKind, OpenAiProviderRuntime};
