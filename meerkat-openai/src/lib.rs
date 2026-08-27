//! meerkat-openai — OpenAI vertical.
//!
//! Owns the OpenAI LLM client, compatible client, live/realtime client,
//! provider runtime, ChatGPT OAuth backend wiring, and OpenAI
//! request-shaping helpers. Capability vocabulary lives in
//! `meerkat_core::model_profile`; the capability data rows live in
//! `meerkat-models`.

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod client;
pub mod client_compatible;
#[cfg(all(not(target_arch = "wasm32"), feature = "experimental-gpt-live"))]
pub mod gpt_live;
#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "experimental-gpt-live-gate0-harness"
))]
#[doc(hidden)]
pub mod gpt_live_gate0;
pub mod image_generation;
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub mod live;
pub(crate) mod request_support;
pub mod runtime;
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub mod text_adapter;
pub mod web_search;

pub use client::{AzureOpenAiWireConfig, OpenAiClient};
pub use client_compatible::{OpenAiCompatibleClient, OpenAiCompatibleClientOptions};
#[cfg(all(not(target_arch = "wasm32"), feature = "experimental-gpt-live"))]
pub use gpt_live::{
    GPT_LIVE_RESPONSES_BRIDGE_TOOL, GptLiveAppendToken, GptLiveBrokerBootstrap, GptLiveBrokerError,
    GptLiveBrokerFactory, GptLiveBrokerObservation, GptLiveBrokerOpenConfig, GptLiveBrokerSession,
    GptLiveBrokerTerminalClass, GptLiveDelegationRef, GptLiveResponsesSessionConfig,
    GptLiveTranscriptItemRef, GptLiveTurnRef, GptLiveTurnRole,
};
pub use image_generation::{
    OpenAiImageGenerationProfile, OpenAiImageOutputOptions, OpenAiImageProviderParams,
    OpenAiImagesApiEndpoint, OpenAiImagesApiPlan, OpenAiImagesApiRequestShape,
    OpenAiResponsesImagePlan,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub use live::OpenAiLiveClient;
#[cfg(all(not(target_arch = "wasm32"), feature = "copilot"))]
pub use runtime::OpenAiCopilotChatCompletionsClientFactory;
pub use runtime::{OpenAiAuthMethod, OpenAiBackendKind, OpenAiProviderRuntime};
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub use text_adapter::OpenAiRealtimeTextAdapter;
pub use web_search::OpenAiWebSearchExecutor;
