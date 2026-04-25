//! meerkat-providers — shim re-exports of generic provider-runtime +
//! auth primitives.
//!
//! Runtime traits + registry come from `meerkat-llm-core::provider_runtime`.
//! Auth primitives (TokenStore, RefreshCoordinator, OAuth helpers,
//! cloud-IAM authorizers) come from `meerkat-auth-core`.
//!
//! Per-provider types (AnthropicProviderRuntime, OpenAiProviderRuntime,
//! GoogleProviderRuntime, per-provider `oauth` modules) now live in the
//! corresponding provider crates (`meerkat-anthropic`, `meerkat-openai`,
//! `meerkat-gemini`) and are NOT re-exported here — depending on a
//! provider's vertical requires a direct dep on that crate. B2 split
//! (2026-04-18).

pub mod runtime {
    #[cfg(not(target_arch = "wasm32"))]
    pub use meerkat_auth_core::resolver::{resolve_external_authorizer, resolve_simple_secret};
    pub use meerkat_llm_core::provider_runtime::{
        AuthLease, DynamicLease, ExternalAuthResolverHandle, NormalizedAuthMethod,
        NormalizedBackendKind, ProviderAuthError, ProviderBindingError, ProviderClientError,
        ProviderRuntime, ProviderRuntimeRegistry, ResolvedConnection, ResolverEnvironment,
        StaticLease, ValidatedBinding,
    };
}

pub use meerkat_llm_core::provider_runtime::{
    AuthLease, DynamicLease, ExternalAuthResolverHandle, NormalizedAuthMethod,
    NormalizedBackendKind, ProviderAuthError, ProviderBindingError, ProviderClientError,
    ProviderRuntime, ProviderRuntimeRegistry, ResolvedConnection, ResolverEnvironment, StaticLease,
    ValidatedBinding,
};

// auth-core impls are non-wasm by construction (filesystem, keyring,
// OS lockfile primitives are not available in the browser).
#[cfg(not(target_arch = "wasm32"))]
pub mod auth_oauth {
    pub use meerkat_auth_core::auth_oauth::*;
}
#[cfg(not(target_arch = "wasm32"))]
pub mod oauth_flow {
    pub use meerkat_auth_core::oauth_flow::*;
}
#[cfg(not(target_arch = "wasm32"))]
pub mod auth_store {
    pub use meerkat_auth_core::auth_store::*;
}
#[cfg(not(target_arch = "wasm32"))]
pub mod authorizers {
    pub use meerkat_auth_core::authorizers::*;
}
