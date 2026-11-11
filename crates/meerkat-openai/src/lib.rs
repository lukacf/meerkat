//! meerkat-openai — OpenAI vertical.
//!
//! Owns the OpenAI LLM client, compatible client, live/realtime client,
//! provider runtime, ChatGPT OAuth backend wiring, and OpenAI
//! request-shaping helpers. Capability vocabulary lives in
//! `meerkat_core::model_profile`; the capability data rows live in
//! `meerkat-models`.

// Test-feature canary. This crate's tests build with its test features,
// which no dev-dependency turns on: in this repository every test command
// passes them (`node scripts/core-workspace-args.mjs --tests -p meerkat-openai`
// prints them). A test build without them would compile the tests they
// gate out and pass over fewer tests, so it fails to compile here instead.
// Only code generation instantiates the generic function below (a test
// binary does), so `cargo check`, clippy and an editor's check run, which
// generate no code, are not stopped by it.
// `.cargo/config.toml` [env] sets MEERKAT_WORKSPACE_TEST_FEATURES for this
// repository's builds only; the published package builds without it.
#[cfg(all(test, not(feature = "workspace-tests")))]
mod test_features_canary {
    fn require<T>() {
        const {
            let _ = size_of::<T>();
            assert!(
                option_env!("MEERKAT_WORKSPACE_TEST_FEATURES").is_none(),
                "meerkat-openai: this repository's test builds need the crate's test features: \
                 `cargo test -p meerkat-openai --features workspace-tests` \
                 (core-workspace-args.mjs --tests prints the flags for any selection)"
            );
        }
    }

    #[used]
    static REQUIRE: fn() = require::<()>;
}

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use meerkat_core::time_compat::wasm as time;
    pub use meerkat_core::tokio::spawn;
    pub use meerkat_core::tokio::task;
    pub use tokio_with_wasm::alias::*;
}

pub mod client;
pub mod client_compatible;
#[cfg(test)]
mod continuity_issuer_replay_tests;
#[cfg(all(not(target_arch = "wasm32"), feature = "experimental-gpt-live"))]
pub mod gpt_live;
#[cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "experimental-gpt-live", feature = "live")
))]
pub mod gpt_live_broker;
#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "experimental-gpt-live-gate0-harness"
))]
#[doc(hidden)]
pub mod gpt_live_gate0;
pub mod image_generation;
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub mod live;
#[cfg(test)]
mod prompt_cache_backend_gate_tests;
#[cfg(all(not(target_arch = "wasm32"), feature = "live"))]
pub mod public_live;
pub(crate) mod request_support;
mod responses_output;
pub mod runtime;
#[cfg(test)]
mod stream_refusal_tests;
#[cfg(test)]
mod structured_output_request_tests;
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub mod text_adapter;
#[cfg(test)]
mod tool_choice_request_tests;
pub mod tool_schema;
#[cfg(test)]
mod turn_cache_anchor_tests;
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
#[cfg(all(not(target_arch = "wasm32"), feature = "live"))]
pub use public_live::{
    GPT_LIVE_MODEL_FAMILY, GptLiveProviderInputLatency, GptLiveProviderInputLatencyStatus,
    PublicLiveBootstrap, PublicLiveBrokerFactory, PublicLiveBrokerSession, PublicLiveOpenConfig,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "copilot"))]
pub use runtime::OpenAiCopilotChatCompletionsClientFactory;
pub use runtime::{OpenAiAuthMethod, OpenAiBackendKind, OpenAiProviderRuntime};
#[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
pub use text_adapter::OpenAiRealtimeTextAdapter;
pub use tool_schema::normalize_openai_tool_parameters_schema;
pub use web_search::OpenAiWebSearchExecutor;
