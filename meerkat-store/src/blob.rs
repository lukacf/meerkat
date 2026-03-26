use async_trait::async_trait;
use meerkat_core::{BlobId, BlobPayload, BlobRef, BlobStore, BlobStoreError};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use ::tokio::sync::RwLock;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredBlob {
    media_type: String,
    data: String,
}

fn compute_blob_id(media_type: &str, data: &str) -> BlobId {
    let mut hasher = Sha256::new();
    hasher.update(media_type.as_bytes());
    hasher.update([0]);
    hasher.update(data.as_bytes());
    BlobId::new(format!("sha256:{:x}", hasher.finalize()))
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
        let blob_id = compute_blob_id(media_type, data);
        let stored = StoredBlob {
            media_type: media_type.to_string(),
            data: data.to_string(),
        };
        self.blobs
            .write()
            .await
            .entry(blob_id.clone())
            .or_insert(stored);
        Ok(BlobRef {
            blob_id,
            media_type: media_type.to_string(),
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
}

#[cfg(not(target_arch = "wasm32"))]
impl FsBlobStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn path_for(&self, blob_id: &BlobId) -> PathBuf {
        let key = blob_id
            .as_str()
            .strip_prefix("sha256:")
            .unwrap_or(blob_id.as_str())
            .to_string();
        let prefix = key.get(0..2).unwrap_or("xx");
        self.root.join(prefix).join(format!("{key}.json"))
    }

    async fn ensure_parent_dir(path: &Path) -> Result<(), BlobStoreError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl BlobStore for FsBlobStore {
    async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
        let blob_id = compute_blob_id(media_type, data);
        let path = self.path_for(&blob_id);
        if tokio::fs::try_exists(&path)
            .await
            .map_err(|err| BlobStoreError::ReadFailed(err.to_string()))?
        {
            return Ok(BlobRef {
                blob_id,
                media_type: media_type.to_string(),
            });
        }

        Self::ensure_parent_dir(&path).await?;
        let stored = StoredBlob {
            media_type: media_type.to_string(),
            data: data.to_string(),
        };
        let bytes = serde_json::to_vec(&stored)
            .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
        Ok(BlobRef {
            blob_id,
            media_type: media_type.to_string(),
        })
    }

    async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
        let path = self.path_for(blob_id);
        let bytes = tokio::fs::read(&path).await.map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                BlobStoreError::NotFound(blob_id.clone())
            } else {
                BlobStoreError::ReadFailed(err.to_string())
            }
        })?;
        let stored: StoredBlob = serde_json::from_slice(&bytes)
            .map_err(|err| BlobStoreError::ReadFailed(err.to_string()))?;
        Ok(BlobPayload {
            blob_id: blob_id.clone(),
            media_type: stored.media_type,
            data: stored.data,
        })
    }

    async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
        let path = self.path_for(blob_id);
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
