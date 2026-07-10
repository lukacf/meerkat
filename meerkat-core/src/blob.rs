use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Validated content-addressed image payload metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedImageBlob {
    pub blob_ref: BlobRef,
    pub encoded_bytes: usize,
    pub decoded_bytes: usize,
}

/// Fail-closed image-blob integrity error used before durable receipts.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ImageBlobIntegrityError {
    #[error("unsupported image media type: {media_type}")]
    UnsupportedMediaType { media_type: String },
    #[error(
        "image payload exceeds the encoded limit of {max_encoded_bytes} bytes (actual {actual_encoded_bytes})"
    )]
    EncodedTooLarge {
        max_encoded_bytes: usize,
        actual_encoded_bytes: usize,
    },
    #[error("image payload contains invalid base64: {detail}")]
    InvalidBase64 { detail: String },
    #[error(
        "image payload exceeds the decoded limit of {max_decoded_bytes} bytes (actual {actual_decoded_bytes})"
    )]
    DecodedTooLarge {
        max_decoded_bytes: usize,
        actual_decoded_bytes: usize,
    },
    #[error("image bytes do not match declared media type {media_type}")]
    SignatureMismatch { media_type: String },
    #[error(
        "blob media type mismatch: expected canonical {expected_media_type}, found {actual_media_type}"
    )]
    MediaTypeMismatch {
        expected_media_type: String,
        actual_media_type: String,
    },
    #[error("blob identity mismatch: expected {expected_blob_id}, found {actual_blob_id}")]
    BlobIdentityMismatch {
        expected_blob_id: BlobId,
        actual_blob_id: BlobId,
    },
    #[error(transparent)]
    Store(#[from] BlobStoreError),
}

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

    /// Whether this id is the canonical filesystem-safe content-addressed
    /// form: `sha256:` followed by exactly 64 lowercase hexadecimal digits.
    #[must_use]
    pub fn is_canonical_sha256(&self) -> bool {
        self.0.strip_prefix("sha256:").is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
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
///
/// The media type is canonicalized through
/// [`MediaType::canonical_str`](crate::image_generation::MediaType::canonical_str)
/// before hashing, so cosmetic string differences (e.g. `image/PNG` vs
/// `image/png`, or a trailing `; charset=...`) never mint divergent blob ids for
/// identical bytes.
pub fn content_blob_id(media_type: &str, data: &str) -> BlobId {
    use sha2::{Digest, Sha256};
    let canonical_media_type = crate::image_generation::MediaType::canonical_str(media_type);
    let mut hasher = Sha256::new();
    hasher.update(canonical_media_type.as_bytes());
    hasher.update([0]);
    hasher.update(data.as_bytes());
    BlobId::new(format!("sha256:{:x}", hasher.finalize()))
}

/// Validate one base64 image payload under an explicit decoded-byte bound.
///
/// This is the canonical receipt-side gate: it canonicalizes MIME, rejects
/// malformed/oversized base64, checks the supported file signature, and mints
/// the content-addressed identity from the exact encoded representation that
/// is stored durably.
pub fn validate_image_blob_payload(
    media_type: &str,
    data: &str,
    max_decoded_bytes: usize,
) -> Result<VerifiedImageBlob, ImageBlobIntegrityError> {
    let canonical_media_type = crate::image_generation::MediaType::parse(media_type)
        .map_err(|_| ImageBlobIntegrityError::UnsupportedMediaType {
            media_type: crate::image_generation::MediaType::canonical_str(media_type),
        })?
        .as_str()
        .to_string();
    if !matches!(canonical_media_type.as_str(), "image/png" | "image/jpeg") {
        return Err(ImageBlobIntegrityError::UnsupportedMediaType {
            media_type: canonical_media_type,
        });
    }
    let max_encoded_bytes = max_decoded_bytes.div_ceil(3).saturating_mul(4);
    if data.len() > max_encoded_bytes {
        return Err(ImageBlobIntegrityError::EncodedTooLarge {
            max_encoded_bytes,
            actual_encoded_bytes: data.len(),
        });
    }
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| ImageBlobIntegrityError::InvalidBase64 {
            detail: error.to_string(),
        })?;
    if decoded.len() > max_decoded_bytes {
        return Err(ImageBlobIntegrityError::DecodedTooLarge {
            max_decoded_bytes,
            actual_decoded_bytes: decoded.len(),
        });
    }
    let signature_matches = match canonical_media_type.as_str() {
        "image/png" => decoded.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => decoded.starts_with(&[0xff, 0xd8, 0xff]),
        _ => false,
    };
    if !signature_matches {
        return Err(ImageBlobIntegrityError::SignatureMismatch {
            media_type: canonical_media_type,
        });
    }
    let blob_id = content_blob_id(&canonical_media_type, data);
    Ok(VerifiedImageBlob {
        blob_ref: BlobRef {
            blob_id,
            media_type: canonical_media_type,
        },
        encoded_bytes: data.len(),
        decoded_bytes: decoded.len(),
    })
}

/// Verify that a referenced durable image exists and exactly matches its
/// canonical media type, bounded payload, signature, and content address.
pub async fn verify_stored_image_blob(
    blob_store: &dyn BlobStore,
    expected_blob_id: &BlobId,
    expected_media_type: &str,
    max_decoded_bytes: usize,
) -> Result<VerifiedImageBlob, ImageBlobIntegrityError> {
    if !expected_blob_id.is_canonical_sha256() {
        return Err(ImageBlobIntegrityError::Store(BlobStoreError::InvalidId(
            expected_blob_id.clone(),
        )));
    }
    let canonical_media_type = crate::image_generation::MediaType::parse(expected_media_type)
        .map_err(|_| ImageBlobIntegrityError::UnsupportedMediaType {
            media_type: crate::image_generation::MediaType::canonical_str(expected_media_type),
        })?
        .as_str()
        .to_string();
    let max_encoded_bytes = max_decoded_bytes.div_ceil(3).saturating_mul(4);
    let payload = blob_store
        .get_with_encoded_limit(expected_blob_id, max_encoded_bytes)
        .await?;
    if payload.blob_id != *expected_blob_id {
        return Err(ImageBlobIntegrityError::BlobIdentityMismatch {
            expected_blob_id: expected_blob_id.clone(),
            actual_blob_id: payload.blob_id,
        });
    }
    if payload.media_type != canonical_media_type {
        return Err(ImageBlobIntegrityError::MediaTypeMismatch {
            expected_media_type: canonical_media_type,
            actual_media_type: payload.media_type,
        });
    }
    let verified =
        validate_image_blob_payload(&payload.media_type, &payload.data, max_decoded_bytes)?;
    if verified.blob_ref.blob_id != *expected_blob_id {
        return Err(ImageBlobIntegrityError::BlobIdentityMismatch {
            expected_blob_id: expected_blob_id.clone(),
            actual_blob_id: verified.blob_ref.blob_id,
        });
    }
    Ok(verified)
}

/// Store (or repair) an inline image and verify the durable object before its
/// reference can become receipt authority.
pub async fn ensure_stored_image_blob(
    blob_store: &dyn BlobStore,
    media_type: &str,
    data: &str,
    max_decoded_bytes: usize,
) -> Result<VerifiedImageBlob, ImageBlobIntegrityError> {
    let expected = validate_image_blob_payload(media_type, data, max_decoded_bytes)?;
    let stored = blob_store
        .put_image(&expected.blob_ref.media_type, data)
        .await?;
    if stored.blob_id != expected.blob_ref.blob_id {
        return Err(ImageBlobIntegrityError::BlobIdentityMismatch {
            expected_blob_id: expected.blob_ref.blob_id,
            actual_blob_id: stored.blob_id,
        });
    }
    if stored.media_type != expected.blob_ref.media_type {
        return Err(ImageBlobIntegrityError::MediaTypeMismatch {
            expected_media_type: expected.blob_ref.media_type,
            actual_media_type: stored.media_type,
        });
    }
    verify_stored_image_blob(
        blob_store,
        &stored.blob_id,
        &stored.media_type,
        max_decoded_bytes,
    )
    .await
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
    #[error("invalid non-canonical blob id: {0}")]
    InvalidId(BlobId),
    #[error("blob not found: {0}")]
    NotFound(BlobId),
    #[error("blob {blob_id} exceeds the bounded read limit of {max_encoded_bytes} encoded bytes")]
    ReadLimitExceeded {
        blob_id: BlobId,
        max_encoded_bytes: usize,
        /// Exact encoded payload length when the store can inspect it without
        /// materializing a copy. Filesystem metadata preflight may not know it.
        actual_encoded_bytes: Option<usize>,
    },
    #[error("blob store read failed: {0}")]
    ReadFailed(String),
    #[error("blob {blob_id} is corrupt: {detail}")]
    Corrupt { blob_id: BlobId, detail: String },
    #[error("blob store write failed: {0}")]
    WriteFailed(String),
    #[error("blob store delete failed: {0}")]
    DeleteFailed(String),
    #[error("blob store unsupported: {0}")]
    Unsupported(String),
    #[error("blob store internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    struct NeverReadBlobStore;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl BlobStore for NeverReadBlobStore {
        async fn put_image(
            &self,
            _media_type: &str,
            _data: &str,
        ) -> Result<BlobRef, BlobStoreError> {
            unreachable!("invalid-id verification must not call the blob store")
        }

        async fn get(&self, _blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
            unreachable!("invalid-id verification must not call the blob store")
        }

        async fn delete(&self, _blob_id: &BlobId) -> Result<(), BlobStoreError> {
            unreachable!("invalid-id verification must not call the blob store")
        }

        fn is_persistent(&self) -> bool {
            false
        }
    }

    #[test]
    fn content_blob_id_canonicalizes_media_type_for_identity() {
        let data = "AAAA";
        // Cosmetic media-type differences must not mint divergent blob ids for
        // identical bytes.
        assert_eq!(
            content_blob_id("image/PNG", data),
            content_blob_id("image/png", data),
            "case differences in the media type must not change blob identity"
        );
        assert_eq!(
            content_blob_id("image/png; charset=binary", data),
            content_blob_id("image/png", data),
            "media-type parameters must not change blob identity"
        );
        assert_eq!(
            content_blob_id("  image/png  ", data),
            content_blob_id("image/png", data),
            "surrounding whitespace must not change blob identity"
        );
    }

    #[test]
    fn content_blob_id_distinguishes_distinct_media_types() {
        let data = "AAAA";
        assert_ne!(
            content_blob_id("image/png", data),
            content_blob_id("image/jpeg", data),
            "genuinely different media types must mint different blob ids"
        );
    }

    #[test]
    fn image_blob_validation_rejects_malformed_base64_before_identity() {
        let error = validate_image_blob_payload("image/png", "not base64!", 1024)
            .expect_err("malformed base64 must not mint a durable identity");
        assert!(matches!(
            error,
            ImageBlobIntegrityError::InvalidBase64 { .. }
        ));
    }

    #[test]
    fn image_blob_validation_rejects_signature_mismatch() {
        use base64::Engine as _;
        let jpeg_bytes_declared_as_png =
            base64::engine::general_purpose::STANDARD.encode([0xff, 0xd8, 0xff, 0x00]);
        let error = validate_image_blob_payload("image/png", &jpeg_bytes_declared_as_png, 1024)
            .expect_err("declared MIME must match the decoded file signature");
        assert!(matches!(
            error,
            ImageBlobIntegrityError::SignatureMismatch { media_type }
                if media_type == "image/png"
        ));
    }

    #[test]
    fn image_blob_validation_is_bounded_before_and_after_decode() {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\n");
        let encoded_error = validate_image_blob_payload("image/png", &encoded, 1)
            .expect_err("encoded representation must be bounded");
        assert!(matches!(
            encoded_error,
            ImageBlobIntegrityError::EncodedTooLarge { .. }
        ));

        // Four base64 characters fit the conservative encoded bound for two
        // decoded bytes, but decode to three and must still fail the exact
        // decoded-byte check.
        let decoded_error = validate_image_blob_payload("image/jpeg", "/9j/", 2)
            .expect_err("decoded representation must be bounded");
        assert!(matches!(
            decoded_error,
            ImageBlobIntegrityError::DecodedTooLarge {
                max_decoded_bytes: 2,
                actual_decoded_bytes: 3,
            }
        ));
    }

    #[test]
    fn image_blob_validation_returns_canonical_mime_and_content_address() {
        let verified =
            validate_image_blob_payload(" Image/PNG; charset=binary ", "iVBORw0KGgo=", 8)
                .expect("minimal PNG signature is a valid bounded fixture");
        assert_eq!(verified.blob_ref.media_type, "image/png");
        assert_eq!(verified.decoded_bytes, 8);
        assert_eq!(
            verified.blob_ref.blob_id,
            content_blob_id("image/png", "iVBORw0KGgo=")
        );
    }

    #[tokio::test]
    async fn stored_image_verification_rejects_noncanonical_id_before_store_read() {
        let invalid = BlobId::new("sha256:../../outside");
        let error = verify_stored_image_blob(&NeverReadBlobStore, &invalid, "image/png", 1024)
            .await
            .expect_err("path-like ids must fail before any store lookup");
        assert!(matches!(
            error,
            ImageBlobIntegrityError::Store(BlobStoreError::InvalidId(ref rejected))
                if rejected == &invalid
        ));
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait BlobStore: Send + Sync {
    async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError>;

    async fn put_artifact(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
        self.put_image(media_type, data).await
    }

    async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError>;

    /// Read a blob only when its encoded payload fits `max_encoded_bytes`.
    ///
    /// The default preserves compatibility for external/test stores, but it
    /// can only validate after [`Self::get`] has materialized the payload.
    /// Production stores should override this method so the size check occurs
    /// before cloning an in-memory value or reading an on-disk object in full.
    async fn get_with_encoded_limit(
        &self,
        blob_id: &BlobId,
        max_encoded_bytes: usize,
    ) -> Result<BlobPayload, BlobStoreError> {
        let payload = self.get(blob_id).await?;
        if payload.data.len() > max_encoded_bytes {
            return Err(BlobStoreError::ReadLimitExceeded {
                blob_id: blob_id.clone(),
                max_encoded_bytes,
                actual_encoded_bytes: Some(payload.data.len()),
            });
        }
        Ok(payload)
    }

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
