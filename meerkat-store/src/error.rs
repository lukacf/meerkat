//! Storage errors

use meerkat_core::{SessionId, SessionStoreError};

/// Backend-specific error type used internally by meerkat-store implementations.
///
/// External consumers should use [`SessionStoreError`] (from `meerkat-core`) for
/// the `SessionStore` trait boundary. This type carries backend-specific variants
/// (for example rusqlite) that the trait contract intentionally erases.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[cfg(not(target_arch = "wasm32"))]
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Session not found: {0}")]
    NotFound(SessionId),

    #[error("Session corrupted: {0}")]
    Corrupted(SessionId),

    #[cfg(not(target_arch = "wasm32"))]
    #[error("Task join error: {0}")]
    Join(#[from] tokio::task::JoinError),

    #[error("Internal error: {0}")]
    Internal(String),

    #[cfg(not(target_arch = "wasm32"))]
    #[error("timed out acquiring realm manifest lock for '{realm_id}'")]
    RealmManifestLockTimeout { realm_id: String },

    #[cfg(not(target_arch = "wasm32"))]
    #[error(
        "realm backend mismatch for '{realm_id}': requested '{requested}', existing '{existing}'"
    )]
    RealmBackendMismatch {
        realm_id: String,
        requested: String,
        existing: String,
    },

    #[cfg(not(target_arch = "wasm32"))]
    #[error("unsupported realm backend for '{realm_id}': '{backend}'")]
    UnsupportedRealmBackend { realm_id: String, backend: String },
}

impl StoreError {
    /// Convert to the backend-agnostic [`SessionStoreError`] at the trait boundary.
    pub fn into_session_store_error(self) -> SessionStoreError {
        match self {
            StoreError::Io(e) => SessionStoreError::Io(e),
            StoreError::Serialization(e) => SessionStoreError::Serialization(e.to_string()),
            StoreError::NotFound(id) => SessionStoreError::NotFound(id),
            StoreError::Corrupted(id) => SessionStoreError::Corrupted(id),
            other => SessionStoreError::Internal(other.to_string()),
        }
    }
}

/// Convert [`StoreError`] to [`SessionStoreError`] at the trait boundary.
///
/// Used as `.map_err(into_session_store_error)` in `SessionStore` trait impls.
/// Only needed on native targets where the persistent store backends exist.
#[cfg(not(target_arch = "wasm32"))]
#[cfg(any(feature = "jsonl", feature = "sqlite"))]
pub(crate) fn into_session_store_error(e: StoreError) -> SessionStoreError {
    e.into_session_store_error()
}
