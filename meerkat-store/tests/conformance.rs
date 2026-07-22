//! In-repo instantiation of the `meerkat-store-conformance` suite.
//!
//! Chapters are instantiated per declared capability:
//!
//! | backend              | baseline | incremental | guarded_projection | append_only | legacy_data |
//! |----------------------|----------|-------------|--------------------|-------------|-------------|
//! | `SqliteSessionStore` | yes      | yes         | yes                | yes         | yes         |
//! | `MemoryStore`        | yes      | yes         | yes                | yes         | yes         |
//! | `JsonlStore`         | yes      | no (by design) | yes             | yes         | yes         |
//!
//! Blob chapters run over `FsBlobStore` and `MemoryBlobStore`; artifact
//! chapters over `FsArtifactStore` and `MemoryArtifactStore`. The
//! capability-discovery swallow test runs the reference forwarding and
//! swallowing wrappers over the incremental backends.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::{ArtifactStore, BlobStore, SessionStore};
use meerkat_store::{MemoryArtifactStore, MemoryBlobStore};
use meerkat_store_conformance::{
    ArtifactStoreFactory, BlobStoreFactory, ConformanceFailure, ForwardingSessionStore,
    SessionStoreFactory, SwallowingSessionStore, chapters,
};

fn factory_failure(error: impl std::fmt::Display) -> ConformanceFailure {
    ConformanceFailure::new("factory", "open", error.to_string())
}

// ---------------------------------------------------------------------------
// Session store factories
// ---------------------------------------------------------------------------

#[cfg(feature = "memory")]
struct MemoryFactory {
    store: Arc<meerkat_store::MemoryStore>,
}

#[cfg(feature = "memory")]
impl MemoryFactory {
    fn new() -> Self {
        Self {
            store: Arc::new(meerkat_store::MemoryStore::new()),
        }
    }
}

#[cfg(feature = "memory")]
#[async_trait]
impl SessionStoreFactory for MemoryFactory {
    async fn open(&self) -> Result<Arc<dyn SessionStore>, ConformanceFailure> {
        // Deliberately non-persistent: reopen shares state.
        Ok(Arc::clone(&self.store) as Arc<dyn SessionStore>)
    }
}

#[cfg(feature = "jsonl")]
struct JsonlFactory {
    dir: tempfile::TempDir,
}

#[cfg(feature = "jsonl")]
impl JsonlFactory {
    fn new() -> Self {
        Self {
            dir: tempfile::TempDir::new().expect("temp dir"),
        }
    }
}

#[cfg(feature = "jsonl")]
#[async_trait]
impl SessionStoreFactory for JsonlFactory {
    async fn open(&self) -> Result<Arc<dyn SessionStore>, ConformanceFailure> {
        let store = meerkat_store::JsonlStore::new(self.dir.path().to_path_buf());
        store.init().await.map_err(factory_failure)?;
        Ok(Arc::new(store) as Arc<dyn SessionStore>)
    }
}

#[cfg(feature = "sqlite")]
struct SqliteFactory {
    dir: tempfile::TempDir,
}

#[cfg(feature = "sqlite")]
impl SqliteFactory {
    fn new() -> Self {
        Self {
            dir: tempfile::TempDir::new().expect("temp dir"),
        }
    }

    fn open_store(&self) -> Result<meerkat_store::SqliteSessionStore, ConformanceFailure> {
        meerkat_store::SqliteSessionStore::open(self.dir.path().join("sessions.sqlite3"))
            .map_err(factory_failure)
    }
}

#[cfg(feature = "sqlite")]
#[async_trait]
impl SessionStoreFactory for SqliteFactory {
    async fn open(&self) -> Result<Arc<dyn SessionStore>, ConformanceFailure> {
        Ok(Arc::new(self.open_store()?) as Arc<dyn SessionStore>)
    }
}

// ---------------------------------------------------------------------------
// Blob / artifact factories
// ---------------------------------------------------------------------------

struct MemoryBlobFactory {
    store: Arc<MemoryBlobStore>,
}

impl MemoryBlobFactory {
    fn new() -> Self {
        Self {
            store: Arc::new(MemoryBlobStore::new()),
        }
    }
}

#[async_trait]
impl BlobStoreFactory for MemoryBlobFactory {
    async fn open(&self) -> Result<Arc<dyn BlobStore>, ConformanceFailure> {
        Ok(Arc::clone(&self.store) as Arc<dyn BlobStore>)
    }
}

struct FsBlobFactory {
    dir: tempfile::TempDir,
}

impl FsBlobFactory {
    fn new() -> Self {
        Self {
            dir: tempfile::TempDir::new().expect("temp dir"),
        }
    }
}

#[async_trait]
impl BlobStoreFactory for FsBlobFactory {
    async fn open(&self) -> Result<Arc<dyn BlobStore>, ConformanceFailure> {
        Ok(Arc::new(meerkat_store::FsBlobStore::new(
            self.dir.path().join("blobs"),
        )) as Arc<dyn BlobStore>)
    }
}

struct MemoryArtifactFactory {
    store: Arc<MemoryArtifactStore>,
}

impl MemoryArtifactFactory {
    fn new() -> Self {
        Self {
            store: Arc::new(MemoryArtifactStore::new()),
        }
    }
}

#[async_trait]
impl ArtifactStoreFactory for MemoryArtifactFactory {
    async fn open(&self) -> Result<Arc<dyn ArtifactStore>, ConformanceFailure> {
        Ok(Arc::clone(&self.store) as Arc<dyn ArtifactStore>)
    }
}

struct FsArtifactFactory {
    dir: tempfile::TempDir,
}

impl FsArtifactFactory {
    fn new() -> Self {
        Self {
            dir: tempfile::TempDir::new().expect("temp dir"),
        }
    }
}

#[async_trait]
impl ArtifactStoreFactory for FsArtifactFactory {
    async fn open(&self) -> Result<Arc<dyn ArtifactStore>, ConformanceFailure> {
        Ok(Arc::new(meerkat_store::FsArtifactStore::new(
            self.dir.path().join("artifacts"),
        )) as Arc<dyn ArtifactStore>)
    }
}

// ---------------------------------------------------------------------------
// MemoryStore: full profile set (incremental-capable, in-memory)
// ---------------------------------------------------------------------------

#[cfg(feature = "memory")]
mod memory_store {
    use super::*;

    #[tokio::test]
    async fn baseline() {
        chapters::baseline(&MemoryFactory::new())
            .await
            .expect("MemoryStore must pass the baseline chapter");
    }

    #[tokio::test]
    async fn incremental() {
        chapters::incremental(&MemoryFactory::new())
            .await
            .expect("MemoryStore must pass the incremental profile");
    }

    #[tokio::test]
    async fn guarded_projection() {
        chapters::guarded_projection(&MemoryFactory::new())
            .await
            .expect("MemoryStore must pass the guarded-projection profile");
    }

    #[tokio::test]
    async fn append_only() {
        chapters::append_only(&MemoryFactory::new())
            .await
            .expect("MemoryStore must pass the append-only chapter");
    }

    #[tokio::test]
    async fn legacy_data() {
        chapters::legacy_data(&MemoryFactory::new())
            .await
            .expect("MemoryStore must pass the legacy-data chapter");
    }

    #[tokio::test]
    async fn forwarding_wrapper_preserves_incremental_capability() {
        let inner: Arc<dyn SessionStore> = Arc::new(meerkat_store::MemoryStore::new());
        chapters::assert_forwards_incremental(inner, ForwardingSessionStore::wrap)
            .await
            .expect("a forwarding wrapper must preserve as_incremental");
    }

    #[tokio::test]
    async fn swallowing_wrapper_is_detected_loudly() {
        let inner: Arc<dyn SessionStore> = Arc::new(meerkat_store::MemoryStore::new());
        let failure = chapters::assert_forwards_incremental(inner, SwallowingSessionStore::wrap)
            .await
            .expect_err("the swallow test must fail loudly for a swallowing wrapper");
        assert_eq!(failure.chapter(), "capability_discovery");
        assert_eq!(failure.step(), "as_incremental_forwarded");
    }
}

// ---------------------------------------------------------------------------
// SqliteSessionStore: full profile set (incremental-capable, durable)
// ---------------------------------------------------------------------------

#[cfg(feature = "sqlite")]
mod sqlite_store {
    use super::*;

    #[tokio::test]
    async fn baseline() {
        chapters::baseline(&SqliteFactory::new())
            .await
            .expect("SqliteSessionStore must pass the baseline chapter");
    }

    #[tokio::test]
    async fn incremental() {
        chapters::incremental(&SqliteFactory::new())
            .await
            .expect("SqliteSessionStore must pass the incremental profile");
    }

    #[tokio::test]
    async fn guarded_projection() {
        chapters::guarded_projection(&SqliteFactory::new())
            .await
            .expect("SqliteSessionStore must pass the guarded-projection profile");
    }

    #[tokio::test]
    async fn append_only() {
        chapters::append_only(&SqliteFactory::new())
            .await
            .expect("SqliteSessionStore must pass the append-only chapter");
    }

    #[tokio::test]
    async fn legacy_data() {
        chapters::legacy_data(&SqliteFactory::new())
            .await
            .expect("SqliteSessionStore must pass the legacy-data chapter");
    }

    #[tokio::test]
    async fn forwarding_wrapper_preserves_incremental_capability() {
        let factory = SqliteFactory::new();
        let inner: Arc<dyn SessionStore> = Arc::new(factory.open_store().expect("sqlite store"));
        chapters::assert_forwards_incremental(inner, ForwardingSessionStore::wrap)
            .await
            .expect("a forwarding wrapper over sqlite must preserve as_incremental");
    }

    #[tokio::test]
    async fn dangling_blob_reference_with_fs_blobs() {
        chapters::dangling_blob_reference(&SqliteFactory::new(), &FsBlobFactory::new())
            .await
            .expect("sqlite sessions + fs blobs must surface dangling references as NotFound");
    }
}

// ---------------------------------------------------------------------------
// JsonlStore: whole-blob profile set (deliberately no incremental capability)
// ---------------------------------------------------------------------------

#[cfg(feature = "jsonl")]
mod jsonl_store {
    use super::*;

    #[tokio::test]
    async fn baseline() {
        chapters::baseline(&JsonlFactory::new())
            .await
            .expect("JsonlStore must pass the baseline chapter");
    }

    #[tokio::test]
    async fn guarded_projection() {
        chapters::guarded_projection(&JsonlFactory::new())
            .await
            .expect("JsonlStore must pass the guarded-projection profile");
    }

    #[tokio::test]
    async fn append_only() {
        chapters::append_only(&JsonlFactory::new())
            .await
            .expect("JsonlStore must pass the append-only chapter");
    }

    #[tokio::test]
    async fn legacy_data() {
        chapters::legacy_data(&JsonlFactory::new())
            .await
            .expect("JsonlStore must pass the legacy-data chapter");
    }

    /// Pins the deliberate capability boundary: JSONL stays on the
    /// whole-blob compat path, so the incremental profile must NOT be
    /// instantiated for it (and the harness refuses to run it vacuously).
    #[tokio::test]
    async fn has_no_incremental_capability_by_design() {
        let store: Arc<dyn SessionStore> =
            Arc::new(meerkat_store::JsonlStore::new(std::env::temp_dir()));
        assert!(store.as_incremental().is_none());

        let failure = chapters::incremental(&JsonlFactory::new())
            .await
            .expect_err("the incremental profile must refuse a whole-blob store");
        assert_eq!(failure.step(), "capability_probe");
    }
}

// ---------------------------------------------------------------------------
// Blob / artifact chapters
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fs_blob_store_conformance() {
    chapters::blobs(&FsBlobFactory::new())
        .await
        .expect("FsBlobStore must pass the blob chapter");
}

#[tokio::test]
async fn memory_blob_store_conformance() {
    chapters::blobs(&MemoryBlobFactory::new())
        .await
        .expect("MemoryBlobStore must pass the blob chapter");
}

#[cfg(feature = "memory")]
#[tokio::test]
async fn dangling_blob_reference_with_memory_stores() {
    chapters::dangling_blob_reference(&MemoryFactory::new(), &MemoryBlobFactory::new())
        .await
        .expect("memory sessions + memory blobs must surface dangling references as NotFound");
}

#[tokio::test]
async fn fs_artifact_store_conformance() {
    chapters::artifacts(&FsArtifactFactory::new())
        .await
        .expect("FsArtifactStore must pass the artifact chapter");
}

#[tokio::test]
async fn memory_artifact_store_conformance() {
    chapters::artifacts(&MemoryArtifactFactory::new())
        .await
        .expect("MemoryArtifactStore must pass the artifact chapter");
}

/// The documented silent-durability hazard: the in-memory artifact/blob
/// stores must report themselves non-persistent so fail-closed durability
/// composition can refuse them for durable slots.
#[tokio::test]
async fn memory_stores_report_non_persistent() {
    assert!(!MemoryBlobStore::new().is_persistent());
    assert!(!MemoryArtifactStore::new().is_persistent());
}
