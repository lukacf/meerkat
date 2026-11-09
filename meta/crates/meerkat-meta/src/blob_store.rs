//! Meta-owned binary blob storage.
//!
//! Published Meerkat 0.6 exposes a `BlobStore` trait whose payload is base64
//! text. Meta keeps that compatibility seam for the runtime while storing
//! local blobs as raw bytes for HTTP serving and multipart upload paths.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use bytes::Bytes;
use meerkat_core::{
    BlobAddressAttestation, BlobId, BlobPayload, BlobRef, BlobStore, BlobStoreError,
};
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryBlobPayload {
    pub blob_id: BlobId,
    pub media_type: String,
    pub size: u64,
    pub data: Bytes,
}

/// Which typed form a caller injected the blob slot in (M4b).
///
/// Meta needs both faces of one store — the raw-bytes
/// [`BinaryBlobStore`] (HTTP `/blobs/{id}` serving, `meta/blob/*`) and
/// meerkat's base64 `BlobStore` (runtime image blobs). A caller who has the
/// binary form injects it directly and avoids the
/// [`BinaryBlobStoreAdapter`] base64 round-trip on the byte-serving paths;
/// a caller with only the meerkat form keeps the historical adapter shape.
#[derive(Clone)]
pub enum BlobStoreInjection {
    /// A meerkat `BlobStore` (base64 payloads); the binary face is adapted.
    Core(Arc<dyn BlobStore>),
    /// A raw-bytes [`BinaryBlobStore`]; the meerkat face is adapted.
    Binary(Arc<dyn BinaryBlobStore>),
}

impl BlobStoreInjection {
    /// Resolve both faces of the injected store.
    pub(crate) fn into_pair(self) -> (Arc<dyn BinaryBlobStore>, Arc<dyn BlobStore>) {
        match self {
            Self::Core(core) => (Arc::new(BinaryBlobStoreAdapter::new(core.clone())), core),
            Self::Binary(binary) => (
                binary.clone(),
                Arc::new(Base64BlobStoreAdapter::new(binary)),
            ),
        }
    }
}

#[async_trait]
pub trait BinaryBlobStore: Send + Sync {
    /// Store raw bytes under Meta's own address,
    /// `sha256(media_type || 0x00 || bytes)`.
    async fn put_bytes(&self, media_type: &str, data: Bytes) -> Result<BlobRef, BlobStoreError>;
    /// Store raw bytes under an address the caller already minted.
    ///
    /// Meerkat's `BlobStore::put_image` contract requires every image blob to
    /// be addressed by `meerkat_core::blob::content_blob_id` over the exact
    /// base64 text it was handed; meerkat's integrity gates
    /// (`ensure_stored_image_blob`, `verify_stored_image_blob`, realtime image
    /// hydration) recompute that id and refuse anything else. The
    /// [`Base64BlobStoreAdapter`] mints that id and stores through this
    /// method so the binary face serves the same object under the same id.
    async fn put_bytes_addressed(
        &self,
        blob_id: BlobId,
        media_type: &str,
        data: Bytes,
    ) -> Result<BlobRef, BlobStoreError>;
    async fn get_bytes(&self, blob_id: &BlobId) -> Result<BinaryBlobPayload, BlobStoreError>;
    async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError>;
    fn is_persistent(&self) -> bool;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlobMetadata {
    media_type: String,
    size: u64,
    created_at_ms: u64,
    source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyStoredBlob {
    media_type: String,
    data: String,
}

#[derive(Debug)]
enum BlobObjectBackend {
    ObjectStore {
        store: Arc<dyn ObjectStore>,
        persistent: bool,
    },
    Memory {
        blobs: RwLock<std::collections::BTreeMap<BlobId, (String, Bytes)>>,
    },
}

pub struct ObjectStoreBlobStore {
    backend: BlobObjectBackend,
    legacy_root: Option<PathBuf>,
}

impl ObjectStoreBlobStore {
    pub fn local(root: PathBuf) -> Result<Self, BlobStoreError> {
        std::fs::create_dir_all(&root).map_err(|err| BlobStoreError::Internal(err.to_string()))?;
        let store = object_store::local::LocalFileSystem::new_with_prefix(&root)
            .map_err(|err| BlobStoreError::Internal(err.to_string()))?;
        Ok(Self {
            backend: BlobObjectBackend::ObjectStore {
                store: Arc::new(store),
                persistent: true,
            },
            legacy_root: Some(root),
        })
    }

    pub fn memory() -> Self {
        Self {
            backend: BlobObjectBackend::Memory {
                blobs: RwLock::new(std::collections::BTreeMap::new()),
            },
            legacy_root: None,
        }
    }

    fn object_path(blob_id: &BlobId) -> ObjectPath {
        ObjectPath::from(format!("objects/{}.bin", storage_key(blob_id)))
    }

    fn meta_path(blob_id: &BlobId) -> ObjectPath {
        ObjectPath::from(format!("meta/{}.json", storage_key(blob_id)))
    }

    /// The files a local store keeps `blob_id` in: its bytes, then its
    /// metadata. The object store's local puts rename them into place
    /// without a sync, so a writer that must outlast a power cut syncs them,
    /// and the directories that name them, itself. `None` for a store with
    /// no local root.
    pub(crate) fn local_files(&self, blob_id: &BlobId) -> Option<[PathBuf; 2]> {
        let root = self.legacy_root.as_ref()?;
        Some(
            [Self::object_path(blob_id), Self::meta_path(blob_id)]
                .map(|path| root.join(AsRef::<str>::as_ref(&path))),
        )
    }

    fn legacy_path(&self, blob_id: &BlobId) -> Option<PathBuf> {
        let root = self.legacy_root.as_ref()?;
        let key = storage_key(blob_id);
        let prefix = key.get(0..2).unwrap_or("xx");
        Some(root.join(prefix).join(format!("{key}.json")))
    }

    async fn read_legacy(&self, blob_id: &BlobId) -> Result<BinaryBlobPayload, BlobStoreError> {
        let path = self
            .legacy_path(blob_id)
            .ok_or_else(|| BlobStoreError::NotFound(blob_id.clone()))?;
        let bytes = tokio::fs::read(&path).await.map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                BlobStoreError::NotFound(blob_id.clone())
            } else {
                BlobStoreError::ReadFailed(err.to_string())
            }
        })?;
        let stored: LegacyStoredBlob = serde_json::from_slice(&bytes)
            .map_err(|err| BlobStoreError::ReadFailed(err.to_string()))?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(stored.data.as_bytes())
            .map_err(|err| {
                BlobStoreError::ReadFailed(format!("invalid legacy blob base64: {err}"))
            })?;
        Ok(BinaryBlobPayload {
            blob_id: blob_id.clone(),
            media_type: stored.media_type,
            size: decoded.len() as u64,
            data: Bytes::from(decoded),
        })
    }
}

impl ObjectStoreBlobStore {
    async fn store_bytes(
        &self,
        blob_id: BlobId,
        media_type: &str,
        data: Bytes,
    ) -> Result<BlobRef, BlobStoreError> {
        if !is_valid_blob_id(&blob_id) {
            return Err(BlobStoreError::InvalidId(blob_id));
        }
        match &self.backend {
            BlobObjectBackend::ObjectStore { store, .. } => {
                let meta = BlobMetadata {
                    media_type: media_type.to_string(),
                    size: data.len() as u64,
                    created_at_ms: current_time_ms(),
                    source: "meta".to_string(),
                };
                let meta_bytes = serde_json::to_vec(&meta)
                    .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
                store
                    .put(&Self::object_path(&blob_id), PutPayload::from(data.clone()))
                    .await
                    .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
                store
                    .put(
                        &Self::meta_path(&blob_id),
                        PutPayload::from(Bytes::from(meta_bytes)),
                    )
                    .await
                    .map_err(|err| BlobStoreError::WriteFailed(err.to_string()))?;
            }
            BlobObjectBackend::Memory { blobs } => {
                blobs
                    .write()
                    .await
                    .entry(blob_id.clone())
                    .or_insert_with(|| (media_type.to_string(), data.clone()));
            }
        }
        Ok(BlobRef {
            blob_id,
            media_type: media_type.to_string(),
        })
    }
}

#[async_trait]
impl BinaryBlobStore for ObjectStoreBlobStore {
    async fn put_bytes(&self, media_type: &str, data: Bytes) -> Result<BlobRef, BlobStoreError> {
        let blob_id = compute_blob_id(media_type, &data);
        self.store_bytes(blob_id, media_type, data).await
    }

    async fn put_bytes_addressed(
        &self,
        blob_id: BlobId,
        media_type: &str,
        data: Bytes,
    ) -> Result<BlobRef, BlobStoreError> {
        self.store_bytes(blob_id, media_type, data).await
    }

    async fn get_bytes(&self, blob_id: &BlobId) -> Result<BinaryBlobPayload, BlobStoreError> {
        if !is_valid_blob_id(blob_id) {
            return Err(BlobStoreError::NotFound(blob_id.clone()));
        }
        match &self.backend {
            BlobObjectBackend::ObjectStore { store, .. } => {
                let meta_bytes = match store.get(&Self::meta_path(blob_id)).await {
                    Ok(result) => result
                        .bytes()
                        .await
                        .map_err(|err| BlobStoreError::ReadFailed(err.to_string()))?,
                    Err(object_store::Error::NotFound { .. }) => {
                        return self.read_legacy(blob_id).await;
                    }
                    Err(err) => return Err(BlobStoreError::ReadFailed(err.to_string())),
                };
                let meta: BlobMetadata = serde_json::from_slice(&meta_bytes)
                    .map_err(|err| BlobStoreError::ReadFailed(err.to_string()))?;
                let data = match store.get(&Self::object_path(blob_id)).await {
                    Ok(result) => result
                        .bytes()
                        .await
                        .map_err(|err| BlobStoreError::ReadFailed(err.to_string()))?,
                    Err(object_store::Error::NotFound { .. }) => {
                        return self.read_legacy(blob_id).await;
                    }
                    Err(err) => return Err(BlobStoreError::ReadFailed(err.to_string())),
                };
                Ok(BinaryBlobPayload {
                    blob_id: blob_id.clone(),
                    media_type: meta.media_type,
                    size: meta.size,
                    data,
                })
            }
            BlobObjectBackend::Memory { blobs } => {
                let blobs = blobs.read().await;
                let (media_type, data) = blobs
                    .get(blob_id)
                    .ok_or_else(|| BlobStoreError::NotFound(blob_id.clone()))?;
                Ok(BinaryBlobPayload {
                    blob_id: blob_id.clone(),
                    media_type: media_type.clone(),
                    size: data.len() as u64,
                    data: data.clone(),
                })
            }
        }
    }

    async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
        if !is_valid_blob_id(blob_id) {
            return Err(BlobStoreError::NotFound(blob_id.clone()));
        }
        match &self.backend {
            BlobObjectBackend::ObjectStore { store, .. } => {
                for path in [Self::object_path(blob_id), Self::meta_path(blob_id)] {
                    match store.delete(&path).await {
                        Ok(()) => {}
                        Err(object_store::Error::NotFound { .. }) => {}
                        Err(err) => return Err(BlobStoreError::DeleteFailed(err.to_string())),
                    }
                }
            }
            BlobObjectBackend::Memory { blobs } => {
                blobs.write().await.remove(blob_id);
            }
        }
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        match &self.backend {
            BlobObjectBackend::ObjectStore { persistent, .. } => *persistent,
            BlobObjectBackend::Memory { .. } => false,
        }
    }
}

pub struct Base64BlobStoreAdapter {
    inner: Arc<dyn BinaryBlobStore>,
}

impl Base64BlobStoreAdapter {
    pub fn new(inner: Arc<dyn BinaryBlobStore>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl BlobStore for Base64BlobStoreAdapter {
    /// Meerkat image blobs are addressed by meerkat's REQUIRED recipe,
    /// `content_blob_id(canonical_media_type, base64_text)`, and stored under
    /// the canonical media type, so meerkat's integrity gates that recompute
    /// the id on read-back accept what this store returns. Meta-native
    /// blobs written through [`BinaryBlobStore::put_bytes`] keep Meta's
    /// raw-bytes address and stay readable through [`BlobStore::get`].
    async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
        let blob_id = meerkat_core::blob::content_blob_id(media_type, data);
        let canonical_media_type =
            meerkat_core::image_generation::MediaType::canonical_str(media_type);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|err| BlobStoreError::WriteFailed(format!("invalid blob base64: {err}")))?;
        // The id is minted over the exact base64 text meerkat handed us, not
        // over a re-encoding of the decoded bytes, so non-canonical base64
        // input still matches what meerkat recomputes on read-back.
        self.inner
            .put_bytes_addressed(blob_id, &canonical_media_type, Bytes::from(bytes))
            .await
    }

    async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
        let payload = self.inner.get_bytes(blob_id).await?;
        Ok(BlobPayload {
            blob_id: payload.blob_id,
            media_type: payload.media_type,
            data: base64::engine::general_purpose::STANDARD.encode(payload.data.as_ref()),
        })
    }

    /// Attest Meta's pre-0.8.41 image address for exactly this payload.
    ///
    /// Before 0.8.41 `put_image` decoded meerkat's base64 and stored it
    /// through [`BinaryBlobStore::put_bytes`], and console image uploads did
    /// the same, so the references those writes left in transcripts carry
    /// Meta's raw-bytes address, `sha256(media_type || 0x00 || bytes)`,
    /// over the media type the store keeps beside the bytes. Meerkat
    /// recomputes its own content address on read-back and asks here before
    /// it refuses such a reference. The adapter recomputes Meta's recipe
    /// over the returned payload and attests only an exact match; every other
    /// reference, and a payload that is not valid base64, stays unattested.
    ///
    /// The attestation binds the reference to the payload's stored media type
    /// and bytes only. The adapter never sees the media type a transcript
    /// block declares for the reference, so checking that declaration against
    /// `payload.media_type` is the caller's side of the gate.
    async fn attest_address(
        &self,
        blob_id: &BlobId,
        payload: &BlobPayload,
    ) -> Result<BlobAddressAttestation, BlobStoreError> {
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(payload.data.as_bytes())
        else {
            return Ok(BlobAddressAttestation::Unattested);
        };
        if compute_blob_id(&payload.media_type, &bytes) == *blob_id {
            Ok(BlobAddressAttestation::StoreAddress)
        } else {
            Ok(BlobAddressAttestation::Unattested)
        }
    }

    async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
        self.inner.delete(blob_id).await
    }

    fn is_persistent(&self) -> bool {
        self.inner.is_persistent()
    }
}

pub struct BinaryBlobStoreAdapter {
    inner: Arc<dyn BlobStore>,
}

impl BinaryBlobStoreAdapter {
    pub fn new(inner: Arc<dyn BlobStore>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl BinaryBlobStore for BinaryBlobStoreAdapter {
    async fn put_bytes(&self, media_type: &str, data: Bytes) -> Result<BlobRef, BlobStoreError> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(data.as_ref());
        self.inner.put_image(media_type, &encoded).await
    }

    /// A meerkat `BlobStore` mints its own (content) address; the caller's
    /// address must be that same id or the write is refused rather than
    /// silently stored under a different identity.
    async fn put_bytes_addressed(
        &self,
        blob_id: BlobId,
        media_type: &str,
        data: Bytes,
    ) -> Result<BlobRef, BlobStoreError> {
        let stored = self.put_bytes(media_type, data).await?;
        if stored.blob_id != blob_id {
            return Err(BlobStoreError::WriteFailed(format!(
                "blob store addressed {} but the caller requested {}",
                stored.blob_id, blob_id
            )));
        }
        Ok(stored)
    }

    async fn get_bytes(&self, blob_id: &BlobId) -> Result<BinaryBlobPayload, BlobStoreError> {
        let payload = self.inner.get(blob_id).await?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(payload.data.as_bytes())
            .map_err(|err| {
                BlobStoreError::ReadFailed(format!("invalid blob base64 payload: {err}"))
            })?;
        Ok(BinaryBlobPayload {
            blob_id: payload.blob_id,
            media_type: payload.media_type,
            size: decoded.len() as u64,
            data: Bytes::from(decoded),
        })
    }

    async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
        self.inner.delete(blob_id).await
    }

    fn is_persistent(&self) -> bool {
        self.inner.is_persistent()
    }
}

/// Store raw image bytes under meerkat's required content address.
///
/// Console uploads and other byte-bearing entry points that place an image
/// into session content must address it the way meerkat does
/// (`content_blob_id(canonical_media_type, base64_text)`), or a later
/// `verify_stored_image_blob` (durable fork preflight, realtime hydration)
/// refuses the reference. Non-image blobs keep [`BinaryBlobStore::put_bytes`].
pub async fn put_meerkat_image_bytes(
    store: &dyn BinaryBlobStore,
    media_type: &str,
    data: Bytes,
) -> Result<BlobRef, BlobStoreError> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(data.as_ref());
    let blob_id = meerkat_core::blob::content_blob_id(media_type, &encoded);
    let canonical_media_type = meerkat_core::image_generation::MediaType::canonical_str(media_type);
    store
        .put_bytes_addressed(blob_id, &canonical_media_type, data)
        .await
}

fn compute_blob_id(media_type: &str, bytes: &[u8]) -> BlobId {
    let mut hasher = Sha256::new();
    hasher.update(media_type.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    BlobId::new(format!("sha256:{:x}", hasher.finalize()))
}

pub fn is_valid_blob_id(blob_id: &BlobId) -> bool {
    is_valid_blob_id_value(blob_id.as_str())
}

pub fn is_valid_blob_id_value(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn storage_key(blob_id: &BlobId) -> &str {
    blob_id
        .as_str()
        .strip_prefix("sha256:")
        .unwrap_or(blob_id.as_str())
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn binary_blob_ids_hash_raw_bytes_not_base64_text() {
        let store = ObjectStoreBlobStore::memory();
        let first = store
            .put_bytes("image/png", Bytes::from_static(b"abc"))
            .await
            .expect("put");
        let second = store
            .put_bytes("image/png", Bytes::from_static(b"abc"))
            .await
            .expect("put same");
        let third = store
            .put_bytes("image/jpeg", Bytes::from_static(b"abc"))
            .await
            .expect("put media");

        assert_eq!(first.blob_id, second.blob_id);
        assert_ne!(first.blob_id, third.blob_id);
        assert_ne!(
            first.blob_id,
            compute_legacy_blob_id("image/png", "YWJj"),
            "raw-byte hashing must not reuse the legacy base64-text hash"
        );
    }

    #[tokio::test]
    async fn base64_adapter_roundtrips_at_json_boundary() {
        let binary: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
        let adapter = Base64BlobStoreAdapter::new(binary);
        let blob = adapter
            .put_image("image/png", "YWJj")
            .await
            .expect("put base64");
        let payload = adapter.get(&blob.blob_id).await.expect("get base64");
        assert_eq!(payload.media_type, "image/png");
        assert_eq!(payload.data, "YWJj");
    }

    /// Base64 of the eight-byte PNG signature: the smallest payload meerkat's
    /// image integrity gate accepts as `image/png`.
    const PNG_SIGNATURE_BASE64: &str = "iVBORw0KGgo=";

    #[tokio::test]
    async fn base64_adapter_put_image_mints_meerkat_content_blob_id() {
        let binary: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
        let adapter = Base64BlobStoreAdapter::new(binary.clone());
        let blob = adapter
            .put_image("image/png", PNG_SIGNATURE_BASE64)
            .await
            .expect("put base64");
        let expected = meerkat_core::blob::content_blob_id("image/png", PNG_SIGNATURE_BASE64);
        assert_eq!(blob.blob_id, expected, "meerkat's required addressing");
        assert_eq!(blob.media_type, "image/png");
        let payload = adapter.get(&expected).await.expect("get by content id");
        assert_eq!(payload.blob_id, expected);
        assert_eq!(payload.data, PNG_SIGNATURE_BASE64);
        let bytes = binary
            .get_bytes(&expected)
            .await
            .expect("binary face by content id");
        assert_eq!(bytes.data.as_ref(), b"\x89PNG\r\n\x1a\n");
    }

    #[tokio::test]
    async fn base64_adapter_canonicalizes_media_type_like_meerkat() {
        let binary: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
        let adapter = Base64BlobStoreAdapter::new(binary);
        let blob = adapter
            .put_image("image/PNG", PNG_SIGNATURE_BASE64)
            .await
            .expect("put base64");
        assert_eq!(
            blob.blob_id,
            meerkat_core::blob::content_blob_id("image/png", PNG_SIGNATURE_BASE64)
        );
        assert_eq!(blob.media_type, "image/png");
        let payload = adapter.get(&blob.blob_id).await.expect("get");
        assert_eq!(payload.media_type, "image/png");
    }

    /// The durable-fork preflight and the realtime user-content path store an
    /// inline image with `ensure_stored_image_blob` and re-verify blob-backed
    /// images with `verify_stored_image_blob`; both recompute meerkat's content
    /// address and refuse a store that minted a different id.
    #[tokio::test]
    async fn meerkat_image_integrity_gates_accept_the_adapter() {
        let binary: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
        let adapter = Base64BlobStoreAdapter::new(binary);
        let stored = meerkat_core::ensure_stored_image_blob(
            &adapter,
            "image/png",
            PNG_SIGNATURE_BASE64,
            1 << 20,
        )
        .await
        .expect("ensure_stored_image_blob through the Meta adapter");
        meerkat_core::verify_stored_image_blob(
            &adapter,
            &stored.blob_ref.blob_id,
            &stored.blob_ref.media_type,
            1 << 20,
        )
        .await
        .expect("verify_stored_image_blob through the Meta adapter");
    }

    #[tokio::test]
    async fn base64_adapter_still_reads_meta_native_blob_ids() {
        let binary: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
        let raw = binary
            .put_bytes("image/png", Bytes::from_static(b"\x89PNG\r\n\x1a\n"))
            .await
            .expect("put raw bytes");
        assert_ne!(
            raw.blob_id,
            meerkat_core::blob::content_blob_id("image/png", PNG_SIGNATURE_BASE64),
            "Meta-native ids keep hashing decoded bytes"
        );
        let adapter = Base64BlobStoreAdapter::new(binary);
        let payload = adapter.get(&raw.blob_id).await.expect("native id readable");
        assert_eq!(payload.blob_id, raw.blob_id);
        assert_eq!(payload.data, PNG_SIGNATURE_BASE64);
    }

    #[tokio::test]
    async fn local_store_reads_legacy_fs_blob_layout() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ObjectStoreBlobStore::local(temp.path().to_path_buf()).expect("local store");
        let legacy_id = compute_legacy_blob_id("image/png", "YWJj");
        let key = storage_key(&legacy_id);
        let legacy_dir = temp.path().join(&key[0..2]);
        tokio::fs::create_dir_all(&legacy_dir)
            .await
            .expect("legacy dir");
        tokio::fs::write(
            legacy_dir.join(format!("{key}.json")),
            serde_json::to_vec(&LegacyStoredBlob {
                media_type: "image/png".to_string(),
                data: "YWJj".to_string(),
            })
            .expect("legacy json"),
        )
        .await
        .expect("legacy write");

        let payload = store.get_bytes(&legacy_id).await.expect("legacy fallback");
        assert_eq!(payload.blob_id, legacy_id);
        assert_eq!(payload.media_type, "image/png");
        assert_eq!(payload.data, Bytes::from_static(b"abc"));
    }

    #[test]
    fn local_store_creates_missing_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("missing").join("blobs");

        let store = ObjectStoreBlobStore::local(root.clone()).expect("local store opens");

        assert!(root.is_dir());
        assert!(store.is_persistent());
    }

    #[tokio::test]
    async fn invalid_blob_ids_do_not_address_object_paths() {
        let store = ObjectStoreBlobStore::memory();
        let invalid = BlobId::from("../not-a-sha");

        let err = store.get_bytes(&invalid).await.expect_err("invalid id");
        assert!(matches!(err, BlobStoreError::NotFound(id) if id == invalid));
        let err = store.delete(&invalid).await.expect_err("invalid delete");
        assert!(matches!(err, BlobStoreError::NotFound(id) if id == invalid));
    }

    /// Decoded bytes of [`PNG_SIGNATURE_BASE64`].
    const PNG_SIGNATURE_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n";

    /// Write an image the way Meta's adapter did before 0.8.41: decoded
    /// bytes through `put_bytes`, addressed by Meta's raw-bytes recipe.
    async fn put_pre_0841_image(binary: &dyn BinaryBlobStore) -> BlobId {
        let stored = binary
            .put_bytes("image/png", Bytes::from_static(PNG_SIGNATURE_BYTES))
            .await
            .expect("put raw bytes");
        assert_ne!(
            stored.blob_id,
            meerkat_core::blob::content_blob_id("image/png", PNG_SIGNATURE_BASE64),
            "the fixture must model a non-content address"
        );
        stored.blob_id
    }

    fn user_image_ref(blob_id: &BlobId) -> meerkat_core::types::Message {
        meerkat_core::types::Message::User(meerkat_core::types::UserMessage::with_blocks(vec![
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: meerkat_core::types::ImageData::Blob {
                    blob_id: blob_id.clone(),
                },
            },
        ]))
    }

    fn only_user_image_blob_id(messages: &[meerkat_core::types::Message]) -> BlobId {
        match messages {
            [meerkat_core::types::Message::User(user)] => match user.content.as_slice() {
                [
                    meerkat_core::types::ContentBlock::Image {
                        data: meerkat_core::types::ImageData::Blob { blob_id },
                        ..
                    },
                ] => blob_id.clone(),
                other => unreachable!("expected one blob-backed image, got {other:?}"),
            },
            other => unreachable!("expected one user message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn adapter_attests_its_pre_0841_raw_bytes_image_address() {
        let binary: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
        let legacy_id = put_pre_0841_image(binary.as_ref()).await;
        let adapter = Base64BlobStoreAdapter::new(binary);
        let payload = adapter.get(&legacy_id).await.expect("legacy id readable");

        assert_eq!(
            adapter
                .attest_address(&legacy_id, &payload)
                .await
                .expect("attest"),
            BlobAddressAttestation::StoreAddress
        );

        // meerkat's accepting gate reports the payload's content address and
        // hands back the payload so a fork can re-home the reference.
        let verification = meerkat_core::verify_stored_image_blob_accepting_store_address(
            &adapter,
            &legacy_id,
            "image/png",
            1 << 20,
        )
        .await
        .expect("an attested legacy reference verifies");
        match verification {
            meerkat_core::StoredImageBlobVerification::StoreAttested { verified, data } => {
                assert_eq!(
                    verified.blob_ref.blob_id,
                    meerkat_core::blob::content_blob_id("image/png", PNG_SIGNATURE_BASE64)
                );
                assert_eq!(data, PNG_SIGNATURE_BASE64);
            }
            other => unreachable!("expected a store-attested verification, got {other:?}"),
        }

        // The strict gate is unchanged: it still refuses the foreign address.
        let strict =
            meerkat_core::verify_stored_image_blob(&adapter, &legacy_id, "image/png", 1 << 20)
                .await
                .expect_err("the strict gate does not consult the store");
        assert!(matches!(
            strict,
            meerkat_core::ImageBlobIntegrityError::BlobIdentityMismatch { .. }
        ));
    }

    #[tokio::test]
    async fn adapter_does_not_attest_a_reference_the_payload_does_not_hash_to() {
        let binary: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
        let legacy_id = put_pre_0841_image(binary.as_ref()).await;
        let adapter = Base64BlobStoreAdapter::new(binary);
        let payload = adapter.get(&legacy_id).await.expect("legacy id readable");

        let different_bytes = BlobPayload {
            data: base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\n\0"),
            ..payload.clone()
        };
        let different_media_type = BlobPayload {
            media_type: "image/jpeg".to_string(),
            ..payload.clone()
        };
        let non_canonical_media_type = BlobPayload {
            media_type: "image/PNG".to_string(),
            ..payload.clone()
        };
        let invalid_base64 = BlobPayload {
            data: "not base64!".to_string(),
            ..payload.clone()
        };
        for mismatched in [
            different_bytes,
            different_media_type,
            non_canonical_media_type,
            invalid_base64,
        ] {
            assert_eq!(
                adapter
                    .attest_address(&legacy_id, &mismatched)
                    .await
                    .expect("attest"),
                BlobAddressAttestation::Unattested,
                "payload {mismatched:?} must not attest {legacy_id}"
            );
        }

        let unrelated_id = compute_blob_id("image/png", b"some other image");
        assert_eq!(
            adapter
                .attest_address(&unrelated_id, &payload)
                .await
                .expect("attest"),
            BlobAddressAttestation::Unattested,
            "the payload attests only its own raw-bytes address"
        );
    }

    /// A reference whose object was replaced by different bytes under the
    /// same legacy address still fails meerkat's gate: the recipe is
    /// recomputed from what the store returns, never looked up.
    #[tokio::test]
    async fn tampered_legacy_object_is_refused_by_the_accepting_gate() {
        let binary: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
        let legacy_id = compute_blob_id("image/png", PNG_SIGNATURE_BYTES);
        binary
            .put_bytes_addressed(
                legacy_id.clone(),
                "image/png",
                Bytes::from_static(b"\x89PNG\r\n\x1a\n\0"),
            )
            .await
            .expect("put tampered bytes under the legacy id");
        let adapter = Base64BlobStoreAdapter::new(binary);

        let error = meerkat_core::verify_stored_image_blob_accepting_store_address(
            &adapter,
            &legacy_id,
            "image/png",
            1 << 20,
        )
        .await
        .expect_err("bytes that do not hash to the reference are refused");
        assert!(matches!(
            error,
            meerkat_core::ImageBlobIntegrityError::BlobIdentityMismatch { expected_blob_id, .. }
                if expected_blob_id == legacy_id
        ));
    }

    #[tokio::test]
    async fn content_addressed_image_is_unaffected_by_attestation() {
        let binary: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
        let adapter = Base64BlobStoreAdapter::new(binary);
        let stored = adapter
            .put_image("image/png", PNG_SIGNATURE_BASE64)
            .await
            .expect("put base64");
        let content_id = meerkat_core::blob::content_blob_id("image/png", PNG_SIGNATURE_BASE64);
        assert_eq!(stored.blob_id, content_id);

        let verification = meerkat_core::verify_stored_image_blob_accepting_store_address(
            &adapter,
            &content_id,
            "image/png",
            1 << 20,
        )
        .await
        .expect("a content-addressed image verifies");
        assert!(matches!(
            verification,
            meerkat_core::StoredImageBlobVerification::ContentAddressed(verified)
                if verified.blob_ref.blob_id == content_id
        ));
        meerkat_core::verify_stored_image_blob(&adapter, &content_id, "image/png", 1 << 20)
            .await
            .expect("the strict gate still accepts the content address");

        // The content address is not Meta's raw-bytes recipe, so the store
        // claims nothing about it; meerkat never needs to ask.
        let payload = adapter.get(&content_id).await.expect("get");
        assert_eq!(
            adapter
                .attest_address(&content_id, &payload)
                .await
                .expect("attest"),
            BlobAddressAttestation::Unattested
        );

        let mut messages = vec![user_image_ref(&content_id)];
        meerkat_core::image_content::preflight_messages_for_durable_fork(&adapter, &mut messages)
            .await
            .expect("fork preflight accepts the content address");
        assert_eq!(
            only_user_image_blob_id(&messages),
            content_id,
            "a content-addressed reference is not re-homed"
        );
    }

    /// Downstream app regression: a transcript holding a pre-0.8.41 image reference
    /// failed every durable fork with "blob identity mismatch". Through the
    /// Meta adapter the fork preflight now re-homes the child's reference to
    /// meerkat's content address and leaves the source's object in place.
    #[tokio::test]
    async fn durable_fork_preflight_rehomes_pre_0841_image_through_the_adapter() {
        let binary: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
        let legacy_id = put_pre_0841_image(binary.as_ref()).await;
        let adapter = Base64BlobStoreAdapter::new(binary.clone());
        let content_id = meerkat_core::blob::content_blob_id("image/png", PNG_SIGNATURE_BASE64);
        let mut messages = vec![user_image_ref(&legacy_id)];

        meerkat_core::image_content::preflight_messages_for_durable_fork(&adapter, &mut messages)
            .await
            .expect("an attested legacy reference must not block the fork");

        assert_eq!(only_user_image_blob_id(&messages), content_id);
        meerkat_core::verify_stored_image_blob(&adapter, &content_id, "image/png", 1 << 20)
            .await
            .expect("the child's re-homed reference verifies under meerkat's strict gate");
        let source_object = binary
            .get_bytes(&legacy_id)
            .await
            .expect("the source's legacy object is kept");
        assert_eq!(source_object.data.as_ref(), PNG_SIGNATURE_BYTES);
    }

    fn compute_legacy_blob_id(media_type: &str, data: &str) -> BlobId {
        let mut hasher = Sha256::new();
        hasher.update(media_type.as_bytes());
        hasher.update([0]);
        hasher.update(data.as_bytes());
        BlobId::new(format!("sha256:{:x}", hasher.finalize()))
    }
}
