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
pub mod image_generation;
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub mod live;
pub mod runtime;
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub mod text_adapter;
pub mod web_search;

pub use client::{AzureOpenAiWireConfig, OpenAiClient};
pub use client_compatible::OpenAiCompatibleClient;
pub use image_generation::{
    OpenAiImageGenerationProfile, OpenAiImageOutputOptions, OpenAiImageProviderParams,
    OpenAiImagesApiEndpoint, OpenAiImagesApiPlan, OpenAiImagesApiRequestShape,
    OpenAiResponsesImagePlan,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub use live::OpenAiLiveClient;
pub use runtime::{OpenAiAuthMethod, OpenAiBackendKind, OpenAiProviderRuntime};
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub use text_adapter::OpenAiRealtimeTextAdapter;
pub use web_search::OpenAiWebSearchExecutor;
