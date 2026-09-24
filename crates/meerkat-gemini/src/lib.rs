//! meerkat-gemini — Google (Gemini / Vertex AI / Code Assist) vertical.
//!
//! Owns the Gemini LLM client, provider runtime, and Google OAuth (Code
//! Assist) + ADC backends. Capability vocabulary lives in
//! `meerkat_core::model_profile`; the capability data rows live in
//! `meerkat-models`.

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod client;
pub mod image_generation;
pub mod runtime;
pub mod web_search;

pub use client::GeminiClient;
pub use image_generation::{
    GeminiImageGenerationProfile, GeminiImageOutputOptions, GeminiImageProviderParams,
    GeminiImageTurnPlan,
};
pub use runtime::{GoogleAuthMethod, GoogleBackendKind, GoogleProviderRuntime};
pub use web_search::GeminiWebSearchExecutor;
