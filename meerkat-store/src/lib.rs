//! meerkat-store - Session persistence for Meerkat
//!
//! This crate provides storage backends for persisting conversation sessions.

// On wasm32, use tokio_with_wasm as a drop-in replacement for tokio.
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod adapter;
mod error;

#[cfg(not(target_arch = "wasm32"))]
pub mod index;
#[cfg(not(target_arch = "wasm32"))]
pub mod realm;

#[cfg(all(feature = "jsonl", not(target_arch = "wasm32")))]
pub mod jsonl;

#[cfg(feature = "memory")]
pub mod memory;

#[cfg(not(target_arch = "wasm32"))]
pub mod redb_store;

pub use adapter::StoreAdapter;
pub use error::StoreError;

#[cfg(not(target_arch = "wasm32"))]
pub use index::SessionIndex;
#[cfg(not(target_arch = "wasm32"))]
pub use realm::{
    REALM_LEASE_HEARTBEAT_SECS, REALM_LEASE_STALE_TTL_SECS, RealmBackend, RealmLeaseGuard,
    RealmLeaseRecord, RealmLeaseStatus, RealmManifest, RealmManifestEntry, RealmOrigin, RealmPaths,
    derive_workspace_realm_id, ensure_realm_manifest, ensure_realm_manifest_in, fnv1a64_hex,
    generate_realm_id, inspect_realm_leases, inspect_realm_leases_in, list_realm_manifests_in,
    open_realm_session_store, open_realm_session_store_in, realm_lease_dir, realm_paths,
    realm_paths_in, sanitize_realm_id, start_realm_lease, start_realm_lease_in,
};
#[cfg(not(target_arch = "wasm32"))]
pub use redb_store::RedbSessionStore;

use async_trait::async_trait;
use meerkat_core::time_compat::SystemTime;
use meerkat_core::{Session, SessionId, SessionMeta};

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
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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

#[cfg(all(feature = "jsonl", not(target_arch = "wasm32")))]
pub use jsonl::JsonlStore;

#[cfg(feature = "memory")]
pub use memory::MemoryStore;
