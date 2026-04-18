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

#[cfg(not(target_arch = "wasm32"))]
pub use resolver::{resolve_external_authorizer, resolve_simple_secret};
