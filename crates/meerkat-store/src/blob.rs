use async_trait::async_trait;
use meerkat_core::{BlobId, BlobPayload, BlobRef, BlobStore, BlobStoreError};
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use ::tokio::sync::RwLock;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredBlob {
    media_type: String,
    data: String,
}

/// Maximum non-payload bytes admitted by the bounded filesystem read seam.
///
/// `StoredBlob` is a tiny JSON object in normal operation. Reserving 64 KiB
/// keeps legacy media-type/JSON overhead compatible while ensuring an
/// attacker-controlled file cannot make a bounded payload read allocate an
/// arbitrarily large buffer before deserialization checks `data.len()`.
#[cfg(not(target_arch = "wasm32"))]
const MAX_STORED_BLOB_METADATA_BYTES: u64 = 64 * 1024;

/// A held maintenance fence is an admission verdict, not a write failure:
/// callers distinguish "the realm is fenced, retry later" from real I/O.
#[cfg(not(target_arch = "wasm32"))]
fn map_blob_admission_error(err: meerkat_sqlite::SqliteStoreError) -> BlobStoreError {
    match err {
        meerkat_sqlite::SqliteStoreError::MaintenanceFenceHeld { path, .. } => {
            BlobStoreError::MaintenanceFenceHeld { path }
        }
        other => BlobStoreError::WriteFailed(other.to_string()),
    }
}

fn compute_blob_id(media_type: &str, data: &str) -> BlobId {
    // Single owner of the content-addressed blob identity lives in
    // meerkat-core so transcript-identity digests and the blob store agree on
    // one definition.
    meerkat_core::blob::content_blob_id(media_type, data)
}

/// In-memory blob store for ephemeral and test paths.
pub struct MemoryBlobStore {
    blobs: RwLock<HashMap<BlobId, StoredBlob>>,
}

impl MemoryBlobStore {
    pub fn new() -> Self {
        Self {
            blobs: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryBlobStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BlobStore for MemoryBlobStore {
    async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
        let media_type = meerkat_core::image_generation::MediaType::canonical_str(media_type);
        let blob_id = compute_blob_id(&media_type, data);
        let stored = StoredBlob {
            media_type: media_type.clone(),
            data: data.to_string(),
        };
        // A content-addressed retry is also a repair operation. Replacing the
        // exact key is safe and ensures an earlier partial/corrupt in-memory
        // object can never be accepted merely because the path exists.
        self.blobs.write().await.insert(blob_id.clone(), stored);
        Ok(BlobRef {
            blob_id,
            media_type,
        })
    }

    async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
        let blobs = self.blobs.read().await;
        let stored = blobs
            .get(blob_id)
            .ok_or_else(|| BlobStoreError::NotFound(blob_id.clone()))?;
        Ok(BlobPayload {
            blob_id: blob_id.clone(),
            media_type: stored.media_type.clone(),
            data: stored.data.clone(),
        })
    }

    async fn get_with_encoded_limit(
        &self,
        blob_id: &BlobId,
        max_encoded_bytes: usize,
    ) -> Result<BlobPayload, BlobStoreError> {
        let blobs = self.blobs.read().await;
        let stored = blobs
            .get(blob_id)
            .ok_or_else(|| BlobStoreError::NotFound(blob_id.clone()))?;
        if stored.data.len() > max_encoded_bytes {
            return Err(BlobStoreError::ReadLimitExceeded {
                blob_id: blob_id.clone(),
                max_encoded_bytes,
                actual_encoded_bytes: Some(stored.data.len()),
            });
        }
        Ok(BlobPayload {
            blob_id: blob_id.clone(),
            media_type: stored.media_type.clone(),
            data: stored.data.clone(),
        })
    }

    async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
        self.blobs.write().await.remove(blob_id);
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct FsBlobStore {
    root: PathBuf,
    /// `Some(<realm_dir>/realm)` when `root` sits inside a realm directory:
    /// write operations take the shared realm write-admission guard on it,
    /// so the realm maintenance fence quiesces blob writes exactly like the
    /// SQLite per-file fences quiesce the databases. Derived once at
    /// construction (cheap hot path).
    realm_admission: Option<PathBuf>,
    #[cfg(test)]
    parent_dir_syncs: std::sync::atomic::AtomicUsize,
}

#[cfg(not(target_arch = "wasm32"))]
impl FsBlobStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            realm_admission: crate::migrate::store_realm_admission_target(&root),
            root,
            #[cfg(test)]
            parent_dir_syncs: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Take the realm write-admission guard for one write operation, when
    /// this store sits inside a realm directory. Fails while the realm is
    /// under offline maintenance; the maintenance holder's own in-process
    /// operations self-admit (see `meerkat_sqlite::fence`).
    fn realm_admission_guard(
        &self,
    ) -> Result<Option<meerkat_sqlite::OperationGuard>, meerkat_sqlite::SqliteStoreError> {
        self.realm_admission
            .as_deref()
            .map(meerkat_sqlite::OperationGuard::for_database)
            .transpose()
    }

    fn path_for(&self, blob_id: &BlobId) -> Result<PathBuf, BlobStoreError> {
        if !blob_id.is_canonical_sha256() {
            return Err(BlobStoreError::InvalidId(blob_id.clone()));
        }
        let Some(key) = blob_id.as_str().strip_prefix("sha256:") else {
            return Err(BlobStoreError::InvalidId(blob_id.clone()));
        };
        let prefix = key.get(0..2).unwrap_or("xx");
        Ok(self.root.join(prefix).join(format!("{key}.json")))
    }

    async fn ensure_parent_dir(path: &Path) -> Result<(), BlobStoreError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
        }
        Ok(())
    }

    async fn stored_object_matches(&self, blob_id: &BlobId, media_type: &str, data: &str) -> bool {
        self.get_with_encoded_limit(blob_id, data.len())
            .await
            .is_ok_and(|stored| {
                stored.media_type == media_type
                    && stored.data == data
                    && compute_blob_id(&stored.media_type, &stored.data) == *blob_id
            })
    }

    async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), BlobStoreError> {
        let temp_path = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = async {
            let mut file = tokio::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp_path)
                .await
                .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
            file.write_all(bytes)
                .await
                .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
            file.sync_all()
                .await
                .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
            drop(file);
            tokio::fs::rename(&temp_path, path)
                .await
                .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))
        }
        .await;
        if result.is_err() {
            // The uniquely named temporary object is ours. Never unlink the
            // content-addressed destination: another writer may own it.
            let _ = tokio::fs::remove_file(&temp_path).await;
        }
        result
    }

    async fn sync_parent_dir(&self, path: &Path) -> Result<(), BlobStoreError> {
        #[cfg(unix)]
        {
            let parent = path.parent().ok_or_else(|| {
                BlobStoreError::WriteFailed("blob path has no parent directory".to_string())
            })?;
            let mut directories = vec![parent];
            if parent != self.root {
                directories.push(self.root.as_path());
            }
            if let Some(root_parent) = self.root.parent()
                && !root_parent.as_os_str().is_empty()
                && !directories.contains(&root_parent)
            {
                directories.push(root_parent);
            }
            for directory_path in directories {
                let directory = tokio::fs::File::open(directory_path)
                    .await
                    .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
                directory
                    .sync_all()
                    .await
                    .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
            }
        }
        #[cfg(test)]
        self.parent_dir_syncs
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl BlobStore for FsBlobStore {
    async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
        // Held across the whole write (including the repair/readback arms).
        let _admission = self
            .realm_admission_guard()
            .map_err(map_blob_admission_error)?;
        let media_type = meerkat_core::image_generation::MediaType::canonical_str(media_type);
        let blob_id = compute_blob_id(&media_type, data);
        let path = self.path_for(&blob_id)?;
        if self
            .stored_object_matches(&blob_id, &media_type, data)
            .await
        {
            // The existing bytes are not a durable success until the shard
            // directory entry itself is synchronized. This also repairs the
            // uncertain outcome of a prior rename whose directory fsync
            // failed or whose process crashed before it.
            self.sync_parent_dir(&path).await?;
            return Ok(BlobRef {
                blob_id,
                media_type,
            });
        }

        Self::ensure_parent_dir(&path).await?;
        let stored = StoredBlob {
            media_type: media_type.clone(),
            data: data.to_string(),
        };
        let bytes = serde_json::to_vec(&stored)
            .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
        if let Err(write_error) = Self::write_atomic(&path, &bytes).await {
            // On platforms where rename cannot replace an existing file, a
            // concurrent exact writer may have won. Accept only after bounded
            // byte-for-byte verification; otherwise surface the write fault.
            if !self
                .stored_object_matches(&blob_id, &media_type, data)
                .await
            {
                return Err(write_error);
            }
        }
        self.sync_parent_dir(&path).await?;
        if !self
            .stored_object_matches(&blob_id, &media_type, data)
            .await
        {
            return Err(BlobStoreError::WriteFailed(format!(
                "atomic blob write failed read-back verification for {blob_id}"
            )));
        }
        Ok(BlobRef {
            blob_id,
            media_type,
        })
    }

    async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
        let path = self.path_for(blob_id)?;
        let bytes = tokio::fs::read(&path).await.map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                BlobStoreError::NotFound(blob_id.clone())
            } else {
                BlobStoreError::ReadFailed(err.to_string())
            }
        })?;
        let stored: StoredBlob =
            serde_json::from_slice(&bytes).map_err(|err| BlobStoreError::Corrupt {
                blob_id: blob_id.clone(),
                detail: err.to_string(),
            })?;
        Ok(BlobPayload {
            blob_id: blob_id.clone(),
            media_type: stored.media_type,
            data: stored.data,
        })
    }

    async fn get_with_encoded_limit(
        &self,
        blob_id: &BlobId,
        max_encoded_bytes: usize,
    ) -> Result<BlobPayload, BlobStoreError> {
        let path = self.path_for(blob_id)?;
        let file = tokio::fs::File::open(&path).await.map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                BlobStoreError::NotFound(blob_id.clone())
            } else {
                BlobStoreError::ReadFailed(err.to_string())
            }
        })?;
        let metadata = file
            .metadata()
            .await
            .map_err(|err| BlobStoreError::ReadFailed(err.to_string()))?;
        let max_file_bytes = u64::try_from(max_encoded_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(MAX_STORED_BLOB_METADATA_BYTES);
        if metadata.len() > max_file_bytes {
            return Err(BlobStoreError::ReadLimitExceeded {
                blob_id: blob_id.clone(),
                max_encoded_bytes,
                actual_encoded_bytes: None,
            });
        }

        // The metadata preflight bounds this allocation even for malformed
        // legacy files. `take` also closes the metadata/read TOCTOU window: a
        // file that grows after the preflight can contribute at most one byte
        // beyond the bound, which is rejected before JSON parsing.
        let mut bytes = Vec::new();
        let mut bounded = file.take(max_file_bytes.saturating_add(1));
        bounded
            .read_to_end(&mut bytes)
            .await
            .map_err(|err| BlobStoreError::ReadFailed(err.to_string()))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_file_bytes {
            return Err(BlobStoreError::ReadLimitExceeded {
                blob_id: blob_id.clone(),
                max_encoded_bytes,
                actual_encoded_bytes: None,
            });
        }
        // The exact encoded-payload check follows JSON parsing, because total
        // file bytes also include the media type and JSON syntax.
        let stored: StoredBlob =
            serde_json::from_slice(&bytes).map_err(|err| BlobStoreError::Corrupt {
                blob_id: blob_id.clone(),
                detail: err.to_string(),
            })?;
        if stored.data.len() > max_encoded_bytes {
            return Err(BlobStoreError::ReadLimitExceeded {
                blob_id: blob_id.clone(),
                max_encoded_bytes,
                actual_encoded_bytes: Some(stored.data.len()),
            });
        }
        Ok(BlobPayload {
            blob_id: blob_id.clone(),
            media_type: stored.media_type,
            data: stored.data,
        })
    }

    async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
        let _admission = self
            .realm_admission_guard()
            .map_err(map_blob_admission_error)?;
        let path = self.path_for(blob_id)?;
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(BlobStoreError::DeleteFailed(err.to_string())),
        }
    }

    fn is_persistent(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_bounded_read_rejects_before_cloning_payload() {
        let store = MemoryBlobStore::new();
        let blob = store
            .put_image("image/png", "AQID")
            .await
            .expect("store test blob");

        let error = store
            .get_with_encoded_limit(&blob.blob_id, 3)
            .await
            .expect_err("four encoded bytes must not fit a three-byte read limit");
        assert!(matches!(
            error,
            BlobStoreError::ReadLimitExceeded {
                blob_id,
                max_encoded_bytes: 3,
                actual_encoded_bytes: Some(4),
            } if blob_id == blob.blob_id
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn filesystem_bounded_read_preflights_oversized_file_before_json_parse() {
        let temp = tempfile::tempdir().expect("temporary blob root");
        let store = FsBlobStore::new(temp.path().to_path_buf());
        let blob_id = BlobId::new(format!("sha256:{}", "ab".repeat(32)));
        let path = store.path_for(&blob_id).expect("canonical blob path");
        FsBlobStore::ensure_parent_dir(&path)
            .await
            .expect("create blob shard directory");
        // Deliberately malformed JSON: if the bounded seam reads and parses
        // the object before checking metadata, the result is ReadFailed.
        tokio::fs::write(
            &path,
            vec![b'x'; MAX_STORED_BLOB_METADATA_BYTES as usize + 2],
        )
        .await
        .expect("write oversized malformed blob");

        let error = store
            .get_with_encoded_limit(&blob_id, 1)
            .await
            .expect_err("metadata preflight must reject the oversized file");
        assert!(matches!(
            error,
            BlobStoreError::ReadLimitExceeded {
                blob_id: rejected,
                max_encoded_bytes: 1,
                actual_encoded_bytes: None,
            } if rejected == blob_id
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn filesystem_put_repairs_corrupt_existing_object_atomically() {
        let temp = tempfile::tempdir().expect("temporary blob root");
        let store = FsBlobStore::new(temp.path().to_path_buf());
        let data = "iVBORw0KGgo=";
        let blob_id = compute_blob_id("image/png", data);
        let path = store.path_for(&blob_id).expect("canonical blob path");
        FsBlobStore::ensure_parent_dir(&path)
            .await
            .expect("create blob shard directory");
        tokio::fs::write(&path, b"{corrupt")
            .await
            .expect("seed corrupt content-addressed destination");

        let repaired = store
            .put_image(" Image/PNG; charset=binary ", data)
            .await
            .expect("exact retry must repair a corrupt destination");
        assert_eq!(repaired.blob_id, blob_id);
        assert_eq!(repaired.media_type, "image/png");
        let payload = store
            .get_with_encoded_limit(&blob_id, data.len())
            .await
            .expect("repaired object must pass bounded readback");
        assert_eq!(payload.media_type, "image/png");
        assert_eq!(payload.data, data);
        assert_eq!(compute_blob_id(&payload.media_type, &payload.data), blob_id);
        assert!(
            store
                .parent_dir_syncs
                .load(std::sync::atomic::Ordering::Relaxed)
                >= 1,
            "a successful atomic repair must fsync its shard directory"
        );

        let entries = std::fs::read_dir(path.parent().expect("shard parent"))
            .expect("read shard directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect shard directory entries");
        assert_eq!(entries.len(), 1, "atomic repair must not leak temp files");
    }

    /// Realm-scoped blob stores must honor the realm write-admission fence:
    /// a foreign maintenance holder excludes blob writes and deletes.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn filesystem_writes_respect_realm_write_admission_fence() {
        let temp = tempfile::tempdir().expect("temporary realm root");
        let realm_dir = temp.path().join("team");
        std::fs::create_dir_all(&realm_dir).expect("realm dir");
        std::fs::write(
            realm_dir.join(meerkat_core::REALM_MANIFEST_FILE_NAME),
            b"{\"realm_id\":\"team\",\"backend\":\"sqlite\"}",
        )
        .expect("manifest");
        let store = FsBlobStore::new(realm_dir.join("blobs"));

        let blob = store
            .put_image("image/png", "AQID")
            .await
            .expect("unfenced write succeeds");

        // A FOREIGN process holding the realm write-admission fence (raw
        // exclusive lock, no in-process holder registry entry).
        let admission_lock = meerkat_sqlite::fence_lock_path(
            &crate::migrate::realm_write_admission_target(&realm_dir),
        );
        let foreign = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&admission_lock)
            .expect("open admission lock");
        foreign.try_lock().expect("foreign exclusive lock");

        assert!(matches!(
            store.put_image("image/png", "BBBB").await,
            Err(BlobStoreError::MaintenanceFenceHeld { .. })
        ));
        assert!(matches!(
            store.delete(&blob.blob_id).await,
            Err(BlobStoreError::MaintenanceFenceHeld { .. })
        ));
        // Reads stay available under the fence.
        store
            .get(&blob.blob_id)
            .await
            .expect("reads pass while the fence is held");

        drop(foreign);
        store
            .put_image("image/png", "BBBB")
            .await
            .expect("write succeeds after fence release");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn filesystem_reads_reject_noncanonical_traversal_ids_before_path_lookup() {
        let temp = tempfile::tempdir().expect("temporary blob root");
        let store = FsBlobStore::new(temp.path().join("blobs"));
        let traversal = BlobId::new("sha256:../../outside");

        assert!(matches!(
            store.get(&traversal).await,
            Err(BlobStoreError::InvalidId(ref rejected)) if rejected == &traversal
        ));
        assert!(matches!(
            store.get_with_encoded_limit(&traversal, 1024).await,
            Err(BlobStoreError::InvalidId(ref rejected)) if rejected == &traversal
        ));
        assert!(matches!(
            store.delete(&traversal).await,
            Err(BlobStoreError::InvalidId(ref rejected)) if rejected == &traversal
        ));
        assert!(
            !temp.path().join("blobs").exists(),
            "invalid ids must be rejected before filesystem traversal"
        );
    }
}
