//! meerkat-store - Session persistence for Meerkat
//!
//! This crate provides storage backends for persisting conversation sessions.

// On wasm32, use tokio_with_wasm as a drop-in replacement for tokio.
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod adapter;
pub mod blob;
mod error;

#[cfg(not(target_arch = "wasm32"))]
pub mod index;
#[cfg(not(target_arch = "wasm32"))]
pub mod realm;
#[cfg(not(target_arch = "wasm32"))]
pub mod schedule_redb_store;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod schedule_sqlite_store;

#[cfg(all(feature = "jsonl", not(target_arch = "wasm32")))]
pub mod jsonl;

#[cfg(feature = "memory")]
pub mod memory;

#[cfg(not(target_arch = "wasm32"))]
pub mod redb_store;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod sqlite_store;

pub use adapter::StoreAdapter;
pub use blob::MemoryBlobStore;
pub use error::StoreError;

// Re-export the canonical trait, filter, and error from meerkat-core.
// Custom storage backends depend only on meerkat-core; existing consumers
// of meerkat-store see no change.
pub use meerkat_core::{SessionFilter, SessionStore, SessionStoreError};

#[cfg(not(target_arch = "wasm32"))]
pub use blob::FsBlobStore;
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
#[cfg(not(target_arch = "wasm32"))]
pub use schedule_redb_store::RedbScheduleStore;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub use schedule_sqlite_store::SqliteScheduleStore;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub use sqlite_store::SqliteSessionStore;

#[cfg(all(feature = "jsonl", not(target_arch = "wasm32")))]
pub use jsonl::JsonlStore;

#[cfg(feature = "memory")]
pub use memory::MemoryStore;
