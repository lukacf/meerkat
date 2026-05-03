//! Provider runtime layer — trait surface and shared types.
//!
//! Concrete provider runtimes (`meerkat-anthropic`, `meerkat-openai`,
//! `meerkat-gemini`) implement `ProviderRuntime`. Shared auth primitives
//! (TokenStore/OAuth/cloud-IAM authorizers) live in `meerkat-auth-core`.
//!
//! Moved from `meerkat-providers/src/runtime/*` in the B2 split
//! (2026-04-18) so that the trait surface is reachable without pulling
//! in provider-specific or heavy-IO dependencies.

pub mod binding;
pub mod catalog;
pub mod errors;
pub mod registry;
pub mod runtime;

pub use binding::{
    DynamicLease, NormalizedAuthMethod, NormalizedBackendKind, ResolvedConnection, StaticLease,
    ValidatedBinding,
};
pub use catalog::ProviderRuntimeCatalog;
pub use errors::{ProviderAuthError, ProviderBindingError, ProviderClientError};
pub use meerkat_core::AuthLease;
pub use registry::{ExternalAuthResolverHandle, ProviderRuntimeRegistry, ResolverEnvironment};
pub use runtime::ProviderRuntime;
