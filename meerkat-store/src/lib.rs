//! meerkat-store - Session persistence for Meerkat
//!
//! This crate provides storage backends for persisting conversation sessions.

pub mod adapter;
mod error;
pub mod index;
pub mod realm;

#[cfg(feature = "jsonl")]
pub mod jsonl;

#[cfg(feature = "memory")]
pub mod memory;

pub mod redb_store;

pub use adapter::StoreAdapter;
pub use error::StoreError;
pub use index::SessionIndex;
pub use realm::{
    RealmBackend, RealmManifest, RealmPaths, derive_workspace_realm_id, ensure_realm_manifest,
    ensure_realm_manifest_in, fnv1a64_hex, generate_realm_id, open_realm_session_store,
    open_realm_session_store_in, realm_paths, realm_paths_in, sanitize_realm_id,
};
pub use redb_store::RedbSessionStore;

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

/// Resolve the session store path from config, with sensible fallbacks.
///
/// Precedence: `config.store.sessions_path` > `config.storage.directory` > platform data dir.
pub fn resolve_store_path(config: &meerkat_core::Config) -> std::path::PathBuf {
    config
        .store
        .sessions_path
        .clone()
        .or_else(|| config.storage.directory.clone())
        .unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("meerkat")
                .join("sessions")
        })
}

/// Resolve the database directory from config, with sensible fallbacks.
///
/// Precedence: `store.database_dir` > `storage.directory/db` > platform data dir.
pub fn resolve_database_dir(config: &meerkat_core::Config) -> std::path::PathBuf {
    config
        .store
        .database_dir
        .clone()
        .or_else(|| config.storage.directory.as_ref().map(|d| d.join("db")))
        .unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("meerkat")
                .join("db")
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::Config;
    use std::path::PathBuf;

    #[test]
    fn test_resolve_database_dir_explicit() {
        let mut config = Config::default();
        config.store.database_dir = Some(PathBuf::from("/explicit/db"));
        assert_eq!(resolve_database_dir(&config), PathBuf::from("/explicit/db"));
    }

    #[test]
    fn test_resolve_database_dir_from_storage_directory() {
        let mut config = Config::default();
        config.storage.directory = Some(PathBuf::from("/data/meerkat"));
        assert_eq!(
            resolve_database_dir(&config),
            PathBuf::from("/data/meerkat/db")
        );
    }

    #[test]
    fn test_resolve_database_dir_explicit_overrides_storage_directory() {
        let mut config = Config::default();
        config.store.database_dir = Some(PathBuf::from("/explicit/db"));
        config.storage.directory = Some(PathBuf::from("/data/meerkat"));
        assert_eq!(resolve_database_dir(&config), PathBuf::from("/explicit/db"));
    }

    #[test]
    fn test_resolve_database_dir_platform_default() {
        let config = Config::default();
        let result = resolve_database_dir(&config);
        // Falls through to platform data dir or "." fallback.
        assert!(
            result.ends_with("db"),
            "expected path ending in 'db', got: {result:?}"
        );
    }

    #[test]
    fn test_store_config_database_dir_serde_roundtrip() {
        let mut config = Config::default();
        config.store.database_dir = Some(PathBuf::from("/test/db"));
        let serialized = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(
            deserialized.store.database_dir,
            Some(PathBuf::from("/test/db"))
        );
    }

    #[test]
    fn test_store_config_backward_compat_without_database_dir() {
        // Config TOML without database_dir should deserialize with None.
        let toml_str = "[store]\n";
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.store.database_dir, None);
    }
}
