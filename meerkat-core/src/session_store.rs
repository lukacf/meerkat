//! SessionStore trait — canonical session persistence contract.
//!
//! This trait lives in `meerkat-core` so that custom storage implementations
//! (Postgres, DynamoDB, etc.) can be written without depending on `meerkat-store`.

use async_trait::async_trait;

use crate::Session;
use crate::session::SessionMeta;
use crate::time_compat::SystemTime;
use crate::types::SessionId;

/// Filter for listing sessions.
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// Only sessions created after this time.
    pub created_after: Option<SystemTime>,
    /// Only sessions updated after this time.
    pub updated_after: Option<SystemTime>,
    /// Maximum number of results.
    pub limit: Option<usize>,
    /// Offset for pagination.
    pub offset: Option<usize>,
}

/// Errors from session store operations.
///
/// Backend-specific details (rusqlite, filesystem, etc.) are erased to strings
/// so that the trait contract carries no I/O dependencies.
#[derive(Debug, thiserror::Error)]
pub enum SessionStoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Session not found: {0}")]
    NotFound(SessionId),

    #[error("Session corrupted: {0}")]
    Corrupted(SessionId),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<serde_json::Error> for SessionStoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization(e.to_string())
    }
}

/// Abstraction over session storage backends.
///
/// All methods take `&self` — implementations must handle interior mutability.
/// Object-safe: consumed as `Arc<dyn SessionStore>` throughout the system.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionStore: Send + Sync {
    /// Save a session (create or update).
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError>;

    /// Load a session by ID.
    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError>;

    /// List sessions matching filter.
    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError>;

    /// Delete a session.
    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError>;

    /// Check if a session exists.
    async fn exists(&self, id: &SessionId) -> Result<bool, SessionStoreError> {
        Ok(self.load(id).await?.is_some())
    }
}
