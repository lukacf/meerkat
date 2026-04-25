//! meerkat-auth-core — shared auth primitives for Meerkat.
//!
//! Owns the concrete implementations of the auth traits declared in
//! `meerkat-core`: TokenStore backends (File/Keyring/Auto/Ephemeral),
//! RefreshCoordinator impls (InMemory/FileLock), OAuth2 helpers
//! (PKCE/callback/device-code/token-exchange), and generic cloud-IAM
//! authorizers (AWS SigV4, Google ADC, Azure AD).
//!
//! Non-wasm32 by construction: the filesystem, keyring, and OS lockfile
//! primitives this crate wraps are not available in the browser. Provider
//! crates (`meerkat-anthropic`, `meerkat-openai`, `meerkat-gemini`) depend
//! on this crate for their cloud-IAM backends via feature flags.
//!
//! Deferral §3 B2 split (2026-04-18): extracted from `meerkat-providers`.

#[cfg(not(target_arch = "wasm32"))]
pub mod auth_oauth;
#[cfg(not(target_arch = "wasm32"))]
pub mod auth_store;
#[cfg(not(target_arch = "wasm32"))]
pub mod authorizers;
#[cfg(not(target_arch = "wasm32"))]
pub mod oauth_flow;
// `resolver` contains per-source-spec arms that are individually cfg-split
// for filesystem/command/managed-store sources. The InlineSecret, Env
// (host env_lookup), ExternalResolver, and authorizer-stub arms compile
// on wasm32 — that path is what browser WASM callers need when their
// bootstrap populates `config.realm` with
// `CredentialSourceSpec::InlineSecret` via
// `populate_realm_from_api_keys`. Exposing the module on wasm32 is what
// lets `meerkat-anthropic` / `meerkat-openai` / `meerkat-gemini`
// register their runtimes on wasm32 so `build_agent` can resolve
// provider credentials in the browser.
pub mod resolver;

#[cfg(all(not(target_arch = "wasm32"), feature = "keyring"))]
pub use auth_store::KeyringTokenStore;
#[cfg(all(not(target_arch = "wasm32"), feature = "file-lock"))]
pub use auth_store::refresh::FileLockCoordinator;
#[cfg(not(target_arch = "wasm32"))]
pub use auth_store::refresh::InMemoryCoordinator;
#[cfg(not(target_arch = "wasm32"))]
pub use auth_store::{
    AutoTokenStore, CommandCredentialRunner, CommandCredentialSpec, EphemeralTokenStore,
    FileTokenStore,
};
#[cfg(not(target_arch = "wasm32"))]
pub use meerkat_core::auth::{RefreshCoordinator, RefreshError, TokenStore};

pub use resolver::{resolve_external_authorizer, resolve_simple_secret};
