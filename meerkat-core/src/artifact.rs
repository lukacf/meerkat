//! Stable artifact records and handles.
//!
//! Artifacts are durable records that describe generated outputs. Their bytes
//! are transported through existing blob storage when possible; filesystem
//! locations and other storage mechanics stay opaque and are not canonical
//! artifact identity.

use crate::blob::{BlobPayload, BlobRef};
use crate::surface_metadata::SurfaceMetadata;
use crate::time_compat::SystemTime;
use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Stable realm-local artifact identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ArtifactId(String);

impl ArtifactId {
    pub fn new(value: impl Into<String>) -> Result<Self, ArtifactError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.contains('/')
            || trimmed.contains('\\')
            || trimmed == "."
            || trimmed == ".."
            || trimmed.contains("..")
        {
            return Err(ArtifactError::InvalidArtifactId { value });
        }
        if !trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '@'))
        {
            return Err(ArtifactError::InvalidArtifactId { value });
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn new_unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ArtifactId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Public artifact handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ArtifactHandle {
    pub artifact_id: ArtifactId,
}

impl ArtifactHandle {
    pub fn new(artifact_id: ArtifactId) -> Self {
        Self { artifact_id }
    }
}

/// Product-neutral artifact classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Text,
    Log,
    CommandOutput,
    Diff,
    Patch,
    GeneratedFile,
    TestReport,
    Screenshot,
    Image,
    Json,
    Binary,
    Other(String),
}

impl ArtifactType {
    fn accepts_media_type(&self, media_type: &str) -> bool {
        match self {
            Self::Text | Self::Log | Self::CommandOutput => media_type.starts_with("text/"),
            Self::Diff | Self::Patch => {
                media_type.starts_with("text/") || media_type == "application/patch"
            }
            Self::GeneratedFile | Self::TestReport | Self::Binary | Self::Other(_) => true,
            Self::Screenshot | Self::Image => media_type.starts_with("image/"),
            Self::Json => media_type == "application/json" || media_type.ends_with("+json"),
        }
    }
}

/// Opaque content handle for an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactContentHandle {
    Blob(BlobRef),
    Opaque { handle: String, media_type: String },
}

impl ArtifactContentHandle {
    pub fn media_type(&self) -> &str {
        match self {
            Self::Blob(blob_ref) => &blob_ref.media_type,
            Self::Opaque { media_type, .. } => media_type,
        }
    }

    pub fn opaque_id(&self) -> String {
        match self {
            Self::Blob(blob_ref) => format!("blob:{}", blob_ref.blob_id.as_str()),
            Self::Opaque { handle, .. } => handle.clone(),
        }
    }
}

/// Ownership references for artifact projection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct ArtifactOwner {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Stable artifact record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct ArtifactRecord {
    pub artifact_id: ArtifactId,
    pub handle: ArtifactHandle,
    pub artifact_type: ArtifactType,
    pub title: String,
    pub media_type: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    pub content_handle: ArtifactContentHandle,
    #[serde(default)]
    pub owner: ArtifactOwner,
    #[serde(default)]
    pub metadata: SurfaceMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub created_at: SystemTime,
    #[serde(default)]
    pub provenance: BTreeMap<String, String>,
}

impl ArtifactRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        artifact_id: ArtifactId,
        artifact_type: ArtifactType,
        title: String,
        media_type: String,
        size_bytes: u64,
        hash: Option<String>,
        content_handle: ArtifactContentHandle,
    ) -> Result<Self, ArtifactError> {
        validate_non_empty("title", &title)?;
        validate_media_type(&media_type)?;
        if content_handle.media_type() != media_type {
            return Err(ArtifactError::MediaTypeMismatch {
                artifact_type,
                media_type,
            });
        }
        if !artifact_type.accepts_media_type(&media_type) {
            return Err(ArtifactError::MediaTypeMismatch {
                artifact_type,
                media_type,
            });
        }
        Ok(Self {
            handle: ArtifactHandle::new(artifact_id.clone()),
            artifact_id,
            artifact_type,
            title,
            media_type,
            size_bytes,
            hash,
            content_handle,
            owner: ArtifactOwner::default(),
            metadata: SurfaceMetadata::default(),
            producer: None,
            created_at: SystemTime::now(),
            provenance: BTreeMap::new(),
        })
    }

    pub fn validate_public_metadata(&self) -> Result<(), crate::SurfaceMetadataError> {
        self.metadata.validate_public()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactListFilter {
    pub session_id: Option<String>,
    pub label_equals: BTreeMap<String, String>,
}

impl ArtifactListFilter {
    pub fn matches(&self, record: &ArtifactRecord) -> bool {
        if let Some(session_id) = self.session_id.as_ref()
            && record.owner.session_id.as_ref() != Some(session_id)
        {
            return false;
        }
        self.label_equals
            .iter()
            .all(|(key, value)| record.metadata.labels.get(key) == Some(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ArtifactPayload {
    pub artifact_id: ArtifactId,
    pub media_type: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// Base64-encoded bytes.
    pub data: String,
}

impl ArtifactPayload {
    pub fn from_record_and_blob(
        record: &ArtifactRecord,
        payload: BlobPayload,
    ) -> Result<Self, ArtifactError> {
        if payload.media_type != record.media_type {
            return Err(ArtifactError::MediaTypeMismatch {
                artifact_type: record.artifact_type.clone(),
                media_type: payload.media_type,
            });
        }
        Ok(Self {
            artifact_id: record.artifact_id.clone(),
            media_type: record.media_type.clone(),
            size_bytes: record.size_bytes,
            hash: record.hash.clone(),
            data: payload.data,
        })
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ArtifactError {
    #[error("invalid artifact id: {value}")]
    InvalidArtifactId { value: String },
    #[error("{field} must not be empty")]
    EmptyField { field: &'static str },
    #[error("invalid media type: {media_type}")]
    InvalidMediaType { media_type: String },
    #[error("artifact type {artifact_type:?} does not accept media type {media_type}")]
    MediaTypeMismatch {
        artifact_type: ArtifactType,
        media_type: String,
    },
    #[error("artifact not found: {0}")]
    NotFound(ArtifactId),
    #[error("artifact store read failed: {0}")]
    ReadFailed(String),
    #[error("artifact store write failed: {0}")]
    WriteFailed(String),
    #[error("artifact content is not blob-backed: {0}")]
    UnsupportedContentHandle(String),
    #[error("artifact store internal error: {0}")]
    Internal(String),
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait ArtifactStore: Send + Sync {
    async fn put(&self, record: ArtifactRecord) -> Result<ArtifactHandle, ArtifactError>;
    async fn get(&self, artifact_id: &ArtifactId) -> Result<ArtifactRecord, ArtifactError>;
    async fn list(&self, filter: ArtifactListFilter) -> Result<Vec<ArtifactRecord>, ArtifactError>;
    async fn delete(&self, artifact_id: &ArtifactId) -> Result<(), ArtifactError>;
    fn is_persistent(&self) -> bool;
}

fn validate_non_empty(field: &'static str, value: &str) -> Result<(), ArtifactError> {
    if value.trim().is_empty() {
        return Err(ArtifactError::EmptyField { field });
    }
    Ok(())
}

fn validate_media_type(media_type: &str) -> Result<(), ArtifactError> {
    let parts: Vec<&str> = media_type.split('/').collect();
    if parts.len() != 2 || parts.iter().any(|part| part.trim().is_empty()) {
        return Err(ArtifactError::InvalidMediaType {
            media_type: media_type.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{BlobId, BlobRef, SurfaceMetadata};
    use std::collections::BTreeMap;

    fn blob_ref() -> BlobRef {
        BlobRef {
            blob_id: BlobId::new("sha256:test-artifact"),
            media_type: "image/png".to_string(),
        }
    }

    #[test]
    fn artifact_record_keeps_stable_id_separate_from_blob_handle() {
        let record = ArtifactRecord::new(
            ArtifactId::new("artifact-1").unwrap(),
            ArtifactType::Screenshot,
            "Screenshot".to_string(),
            "image/png".to_string(),
            12,
            Some("sha256:payload".to_string()),
            ArtifactContentHandle::Blob(blob_ref()),
        )
        .unwrap();

        assert_eq!(record.artifact_id.as_str(), "artifact-1");
        assert_eq!(record.handle.artifact_id.as_str(), "artifact-1");
        assert_eq!(
            record.content_handle.opaque_id(),
            "blob:sha256:test-artifact"
        );
    }

    #[test]
    fn artifact_id_rejects_path_like_values() {
        for raw in ["../artifact", "/tmp/artifact", "artifact/one", ""] {
            assert!(matches!(
                ArtifactId::new(raw),
                Err(ArtifactError::InvalidArtifactId { .. })
            ));
        }
    }

    #[test]
    fn artifact_record_rejects_media_type_mismatch() {
        let err = ArtifactRecord::new(
            ArtifactId::new("artifact-1").unwrap(),
            ArtifactType::Image,
            "Image".to_string(),
            "text/plain".to_string(),
            12,
            None,
            ArtifactContentHandle::Blob(blob_ref()),
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ArtifactError::MediaTypeMismatch {
                artifact_type: ArtifactType::Image,
                ..
            }
        ));
    }

    #[test]
    fn artifact_metadata_rejects_reserved_meerkat_keys() {
        let metadata = SurfaceMetadata {
            labels: BTreeMap::from([("meerkat.runtime_id".to_string(), "spoof".to_string())]),
            app_context: None,
        };
        let mut record = ArtifactRecord::new(
            ArtifactId::new("artifact-1").unwrap(),
            ArtifactType::Binary,
            "Binary".to_string(),
            "application/octet-stream".to_string(),
            12,
            None,
            ArtifactContentHandle::Blob(BlobRef {
                blob_id: BlobId::new("sha256:test-binary"),
                media_type: "application/octet-stream".to_string(),
            }),
        )
        .unwrap();
        record.metadata = metadata;

        assert!(matches!(
            record.validate_public_metadata(),
            Err(crate::SurfaceMetadataError::ReservedLabelKey { .. })
        ));
    }

    #[test]
    fn artifact_record_serialization_does_not_leak_paths() {
        let record = ArtifactRecord::new(
            ArtifactId::new("artifact-1").unwrap(),
            ArtifactType::Patch,
            "Patch".to_string(),
            "text/x-diff".to_string(),
            12,
            None,
            ArtifactContentHandle::Opaque {
                handle: "store-object-1".to_string(),
                media_type: "text/x-diff".to_string(),
            },
        )
        .unwrap();

        let encoded = serde_json::to_string(&record).unwrap();
        assert!(!encoded.contains("/tmp"));
        assert!(!encoded.contains("path"));
        assert!(encoded.contains("store-object-1"));
    }
}
