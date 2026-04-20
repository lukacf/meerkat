//! meerkat-openai — OpenAI vertical.
//!
//! Owns the OpenAI LLM client, compatible client, live/realtime client,
//! provider runtime, and ChatGPT OAuth backend wiring. Capability tables
//! and profile rules live in `meerkat_core::model_profile` (Phase 1 of
//! the B2 split).

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod client;
pub mod client_compatible;
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub mod live;
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub mod realtime_attachment;
pub mod runtime;
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub mod text_adapter;

pub use client::OpenAiClient;
pub use client_compatible::OpenAiCompatibleClient;
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub use live::OpenAiLiveClient;
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub use realtime_attachment::OpenAiRealtimeAttachmentOrchestrator;
pub use runtime::{OpenAiAuthMethod, OpenAiBackendKind, OpenAiProviderRuntime};
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub use text_adapter::OpenAiRealtimeTextAdapter;
