use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Canonical realm-local blob identifier.
///
/// The identifier is content-addressed, but storage and GC semantics remain
/// realm-scoped.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlobId(String);

impl BlobId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Canonical content-addressed blob identifier for an image payload.
///
/// This is the single owner of the blob-identity hash: blob stores compute
/// stored ids through it, and transcript-identity digests canonicalize inline
/// image payloads through it so that the inline and blob-backed representations
/// of the same image share one identity. Because an inline image hydrates from
/// its blob's own bytes, `content_blob_id(media_type, hydrated_data)` equals the
/// id the blob store minted for that image.
pub fn content_blob_id(media_type: &str, data: &str) -> BlobId {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(media_type.as_bytes());
    hasher.update([0]);
    hasher.update(data.as_bytes());
    BlobId::new(format!("sha256:{:x}", hasher.finalize()))
}

impl From<String> for BlobId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for BlobId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Durable image reference owned by transcript/runtime state.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlobRef {
    pub blob_id: BlobId,
    pub media_type: String,
}

/// Resolved blob bytes returned by the blob store.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobPayload {
    pub blob_id: BlobId,
    pub media_type: String,
    /// Base64-encoded bytes.
    pub data: String,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum BlobStoreError {
    #[error("blob not found: {0}")]
    NotFound(BlobId),
    #[error("blob store read failed: {0}")]
    ReadFailed(String),
    #[error("blob store write failed: {0}")]
    WriteFailed(String),
    #[error("blob store delete failed: {0}")]
    DeleteFailed(String),
    #[error("blob store unsupported: {0}")]
    Unsupported(String),
    #[error("blob store internal error: {0}")]
    Internal(String),
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait BlobStore: Send + Sync {
    async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError>;

    async fn put_artifact(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
        self.put_image(media_type, data).await
    }

    async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError>;

    async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError>;

    async fn exists(&self, blob_id: &BlobId) -> Result<bool, BlobStoreError> {
        match self.get(blob_id).await {
            Ok(_) => Ok(true),
            Err(BlobStoreError::NotFound(_)) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Whether the store is persistent across process restarts.
    fn is_persistent(&self) -> bool;
}
