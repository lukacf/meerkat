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

    /// The requested realm id sanitizes to the same on-disk path as an
    /// existing manifest that pins a *different* realm identity (e.g. the
    /// raw slugs `a.b` and `a_b` both sanitize to the `a_b` directory).
    /// Two distinct realm identities must never silently share one
    /// manifest, so the path-aliased open is rejected fail-closed rather
    /// than handing back the wrong realm's manifest.
    #[cfg(not(target_arch = "wasm32"))]
    #[error(
        "realm identity mismatch: requested '{requested}' aliases existing manifest '{existing}'"
    )]
    RealmIdentityMismatch { requested: String, existing: String },

    /// Persisted manifest carried a realm id that fails the typed
    /// slug validator (wave-c C-12 sibling retype — the on-disk form
    /// is free-string but the domain type is `RealmId` which enforces
    /// the slug grammar). Reported when an on-disk manifest was
    /// hand-edited to an unparseable realm slug.
    #[cfg(not(target_arch = "wasm32"))]
    #[error("invalid realm id slug in persisted manifest: '{0}'")]
    InvalidRealmSlug(String),

    /// The file's schema ledger records a version newer than this binary
    /// supports: a newer binary migrated the file and this one must refuse
    /// it (typed, health-visible refusal — never a crash loop).
    #[cfg(not(target_arch = "wasm32"))]
    #[error(
        "schema for domain '{domain}' is from the future: file has version {found}, \
         this binary supports up to {supported}"
    )]
    SchemaFromTheFuture {
        domain: String,
        found: i64,
        supported: i64,
    },

    /// The exclusive maintenance fence is held for this database; storage is
    /// under offline maintenance.
    #[cfg(not(target_arch = "wasm32"))]
    #[error("maintenance fence is held for '{path}'; storage is under offline maintenance")]
    MaintenanceFenceHeld { path: std::path::PathBuf },

    /// The realm manifest's format version is newer than this binary
    /// supports (typed refusal: an unknown format may have relocated
    /// storage this binary would otherwise recreate empty).
    #[cfg(not(target_arch = "wasm32"))]
    #[error(
        "realm manifest for '{realm_id}' has format {found}, this binary supports up to \
         {supported}; refusing to open"
    )]
    ManifestFromTheFuture {
        realm_id: String,
        found: u32,
        supported: u32,
    },

    /// The realm is pinned to an external storage provider that this
    /// composition does not supply.
    #[cfg(not(target_arch = "wasm32"))]
    #[error("realm '{realm_id}' is pinned to external storage provider '{provider}'")]
    ExternalProviderRealm { realm_id: String, provider: String },
}

#[cfg(not(target_arch = "wasm32"))]
impl From<meerkat_sqlite::SqliteStoreError> for StoreError {
    fn from(err: meerkat_sqlite::SqliteStoreError) -> Self {
        use meerkat_sqlite::SqliteStoreError as E;
        match err {
            E::Io(io) => StoreError::Io(io),
            E::Sqlite(sql) => StoreError::Sqlite(sql),
            E::SchemaFromTheFuture {
                domain,
                found,
                supported,
            } => StoreError::SchemaFromTheFuture {
                domain,
                found,
                supported,
            },
            E::MaintenanceFenceHeld { path } => StoreError::MaintenanceFenceHeld { path },
            other @ (E::MigrationFailed { .. }
            | E::InvalidMigrationList { .. }
            | E::OpenRefused { .. }) => StoreError::Internal(other.to_string()),
        }
    }
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
