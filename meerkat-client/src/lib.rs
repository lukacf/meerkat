//! meerkat-client — shim re-exports.
//!
//! The LLM wire-client trait + plumbing moved to `meerkat-llm-core`;
//! per-provider clients (`AnthropicClient`, `OpenAiClient`, `GeminiClient`,
//! `OpenAiLiveClient`, `OpenAiCompatibleClient`)
//! moved to `meerkat-anthropic`, `meerkat-openai`, `meerkat-gemini`.
//! Provider-runtime types moved to `meerkat-core::provider_runtime`.
//! Auth primitives moved to `meerkat-auth-core`.
//!
//! This crate is retained as a thin shim so that downstream
//! `meerkat_client::*` imports continue to work without an import sweep.
//! B2 split (2026-04-18).

pub use meerkat_llm_core::{
    BlockAssembler, BlockKey, FactoryError, LlmClient, LlmClientAdapter, LlmDoneOutcome, LlmError,
    LlmEvent, LlmRequest, LlmResponse, RealtimeExternalSessionTarget, RealtimeSession,
    RealtimeSessionEvent, RealtimeSessionFactory, StreamAssemblyError, TestClient, ToolCallBuffer,
};

#[cfg(feature = "anthropic")]
pub use meerkat_anthropic::AnthropicClient;

#[cfg(all(
    feature = "openai",
    feature = "openai-realtime",
    not(target_arch = "wasm32")
))]
pub use meerkat_openai::OpenAiLiveClient;
#[cfg(feature = "openai")]
pub use meerkat_openai::client_compatible::OpenAiCompatibleMode;
#[cfg(all(
    feature = "openai",
    feature = "openai-realtime",
    not(target_arch = "wasm32")
))]
pub use meerkat_openai::live::{
    OpenAiLiveCallTarget, OpenAiLiveClientEvent, OpenAiLiveServerEvent, OpenAiLiveSession,
    OpenAiLiveSessionFactory, OpenAiRealtimeSession, OpenAiRealtimeSessionFactory,
    openai_live_function_call_error_event, openai_live_function_call_success_events,
    pump_openai_live_session,
};
#[cfg(feature = "openai")]
pub use meerkat_openai::{OpenAiClient, OpenAiCompatibleClient};

#[cfg(feature = "gemini")]
pub use meerkat_gemini::GeminiClient;

// Re-export ResolverEnvironment/ProviderRuntimeRegistry so existing callers
// using `meerkat_client::{ResolverEnvironment, ProviderRuntimeRegistry}`
// continue to compile.
pub use meerkat_llm_core::provider_runtime::{ProviderRuntimeRegistry, ResolverEnvironment};

// Shim modules that expose the legacy paths as aliases to the new homes.
pub mod adapter {
    pub use meerkat_llm_core::LlmClientAdapter;
}
pub mod block_assembler {
    pub use meerkat_llm_core::{BlockAssembler, BlockKey, StreamAssemblyError};
}
pub mod error {
    pub use meerkat_llm_core::LlmError;
}
pub mod factory {
    pub use meerkat_llm_core::FactoryError;
}
pub mod realtime_session {
    pub use meerkat_llm_core::realtime_session::*;
}
pub mod types {
    pub use meerkat_llm_core::{
        LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmResponse, LlmStream, ToolCallBuffer,
    };
}

#[cfg(feature = "anthropic")]
pub mod anthropic {
    pub use meerkat_anthropic::*;
}

#[cfg(feature = "openai")]
pub mod openai {
    pub use meerkat_openai::client::*;
}
#[cfg(feature = "openai")]
pub mod openai_compatible {
    pub use meerkat_openai::client_compatible::*;
}
#[cfg(all(
    feature = "openai",
    feature = "openai-realtime",
    not(target_arch = "wasm32")
))]
pub mod openai_live {
    pub use meerkat_openai::live::*;
}
#[cfg(feature = "gemini")]
pub mod gemini {
    pub use meerkat_gemini::*;
}
