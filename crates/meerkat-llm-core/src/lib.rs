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

// Without `workspace-tests` (the published package, whose test command
// leaves it off) the unit tests that need the workspace compile out, and
// the test helpers only they use go unused. The workspace's test builds
// turn the feature on and keep both lints.
#![cfg_attr(
    all(test, not(feature = "workspace-tests")),
    allow(dead_code, unused_imports)
)]

// Test-feature canary. This crate's tests build with its test features,
// which no dev-dependency turns on: in this repository every test command
// passes them (`node scripts/core-workspace-args.mjs --tests -p meerkat-llm-core`
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
                "meerkat-llm-core: this repository's test builds need the crate's test features: \
                 `cargo test -p meerkat-llm-core --features workspace-tests` \
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

pub mod adapter;
pub mod block_assembler;
pub mod error;
pub mod factory;
pub mod http;
pub mod provider_runtime;
pub mod realtime_session;
pub mod request_authorization;
pub mod streaming;
pub mod test_client;
pub mod types;

pub use adapter::{LateEventChannel, LlmClientAdapter};
pub use block_assembler::{BlockAssembler, BlockKey, StreamAssemblyError};
pub use error::{LlmError, TOOL_CHOICE_UNSUPPORTED_DETAILS_CLASS, ToolChoiceRefusal};
pub use factory::FactoryError;
pub use realtime_session::{
    RealtimeExternalSessionTarget, RealtimeSession, RealtimeSessionEvent, RealtimeSessionFactory,
};
pub use test_client::TestClient;
pub use types::{
    ImageGenerationExecutor, LlmClient, LlmDoneOutcome, LlmEvent, LlmReplayProjection, LlmRequest,
    LlmRequestRouteWitness, LlmResponse, LlmStream, PlainModelRoute, PreparedLlmRequest,
    ProviderGeneratedImage, ProviderImageGenerationOutput, ProviderImageGenerationRequest,
    ToolCallBuffer, WebSearchExecutor, WebSearchStop, classify_web_search_stop,
    dimensions_from_size_preference, media_type_from_format_preference,
    normalize_base64_image_data,
};
