//! Provider-specific auth-method and backend-kind enums.
//!
//! These typed enums enumerate the `(backend, auth_method)` matrix for
//! each provider. They are pure value types (no I/O, no heavy deps) so
//! they live in core. Consumed by `meerkat-llm-core::provider_runtime`'s
//! `NormalizedBackendKind` / `NormalizedAuthMethod` and by each provider
//! crate's runtime module.
//!
//! Moved from per-provider `runtime/{auth,backend}.rs` in the B2 split
//! (2026-04-18) to break the cycle: llm-core's `binding.rs` references
//! these, and llm-core sits below the provider crates in the dep graph.

pub mod anthropic_auth;
pub mod anthropic_backend;
pub mod google_auth;
pub mod google_backend;
pub mod openai_auth;
pub mod openai_backend;
pub mod other_auth;
pub mod other_backend;
pub mod self_hosted_auth;
pub mod self_hosted_backend;

pub mod anthropic {
    pub use super::anthropic_auth::AnthropicAuthMethod;
    pub use super::anthropic_backend::AnthropicBackendKind;
}

pub mod google {
    pub use super::google_auth::GoogleAuthMethod;
    pub use super::google_backend::GoogleBackendKind;
}

pub mod openai {
    pub use super::openai_auth::OpenAiAuthMethod;
    pub use super::openai_backend::OpenAiBackendKind;
}

pub mod other {
    pub use super::other_auth::OtherAuthMethod;
    pub use super::other_backend::OtherBackendKind;
}

pub mod self_hosted {
    pub use super::self_hosted_auth::SelfHostedAuthMethod;
    pub use super::self_hosted_backend::SelfHostedBackendKind;
}

pub use anthropic::{AnthropicAuthMethod, AnthropicBackendKind};
pub use google::{GoogleAuthMethod, GoogleBackendKind};
pub use openai::{OpenAiAuthMethod, OpenAiBackendKind};
pub use other::{OtherAuthMethod, OtherBackendKind};
pub use self_hosted::{SelfHostedAuthMethod, SelfHostedBackendKind};
