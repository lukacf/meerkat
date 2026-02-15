//! Storage errors

use meerkat_core::SessionId;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Database error: {0}")]
    Database(#[from] Box<redb::Error>),

    #[error("Session not found: {0}")]
    NotFound(SessionId),

    #[error("Session corrupted: {0}")]
    Corrupted(SessionId),

    #[error("Task join error: {0}")]
    Join(#[from] tokio::task::JoinError),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("timed out acquiring realm manifest lock for '{realm_id}'")]
    RealmManifestLockTimeout { realm_id: String },

    #[error(
        "realm backend mismatch for '{realm_id}': requested '{requested}', existing '{existing}'"
    )]
    RealmBackendMismatch {
        realm_id: String,
        requested: String,
        existing: String,
    },
}
