//! meerkat-store - Session persistence for Meerkat
//!
//! This crate provides storage backends for persisting conversation sessions.

pub mod adapter;
mod error;
pub mod index;

#[cfg(feature = "jsonl")]
pub mod jsonl;

#[cfg(feature = "memory")]
pub mod memory;

pub use adapter::StoreAdapter;
pub use error::StoreError;
pub use index::SessionIndex;

use async_trait::async_trait;
use meerkat_core::{Session, SessionId, SessionMeta};
use std::time::SystemTime;

/// Filter for listing sessions
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// Only sessions created after this time
    pub created_after: Option<SystemTime>,
    /// Only sessions updated after this time
    pub updated_after: Option<SystemTime>,
    /// Maximum number of results
    pub limit: Option<usize>,
    /// Offset for pagination
    pub offset: Option<usize>,
}

/// Abstraction over session storage backends
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Save a session (create or update)
    async fn save(&self, session: &Session) -> Result<(), StoreError>;

    /// Load a session by ID
    async fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError>;

    /// List sessions matching filter
    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError>;

    /// Delete a session
    async fn delete(&self, id: &SessionId) -> Result<(), StoreError>;

    /// Check if a session exists
    async fn exists(&self, id: &SessionId) -> Result<bool, StoreError> {
        Ok(self.load(id).await?.is_some())
    }
}

#[cfg(feature = "jsonl")]
pub use jsonl::JsonlStore;

#[cfg(feature = "memory")]
pub use memory::MemoryStore;
