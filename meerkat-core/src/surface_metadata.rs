//! Shared surface/runtime metadata contracts.
//!
//! Surface metadata is caller-owned annotation used for filtering and UI
//! projection. It is never semantic authority. Meerkat-owned labels and
//! metadata keys are reserved so public callers cannot spoof runtime facts.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Label prefix reserved for Meerkat-owned runtime facts.
pub const MEERKAT_METADATA_PREFIX: &str = "meerkat.";

/// Legacy mob discovery labels that are stamped by the mob runtime.
pub const RESERVED_MOB_LABEL_KEYS: [&str; 3] = ["mob_id", "role", "meerkat_id"];

/// Opaque caller-owned metadata shared across public surfaces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct SurfaceMetadata {
    /// Caller-owned labels for filtering and projection.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Caller-owned opaque application context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<serde_json::Value>,
}

impl SurfaceMetadata {
    /// Build metadata from the existing optional create-surface fields.
    #[must_use]
    pub fn from_optional_parts(
        labels: Option<BTreeMap<String, String>>,
        app_context: Option<serde_json::Value>,
    ) -> Self {
        Self {
            labels: labels.unwrap_or_default(),
            app_context,
        }
    }

    /// Return whether this metadata is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty() && self.app_context.is_none()
    }

    /// Validate caller-supplied metadata against Meerkat-owned keys.
    pub fn validate_public(&self) -> Result<(), SurfaceMetadataError> {
        validate_public_labels(Some(&self.labels))?;
        validate_public_app_context(self.app_context.as_ref())
    }
}

/// Runtime-carried metadata projection.
///
/// This type intentionally wraps surface metadata instead of adding semantic
/// meaning. Runtime-owned facts belong in typed runtime state, not in labels.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct RuntimeMetadata {
    #[serde(default, skip_serializing_if = "SurfaceMetadata::is_empty")]
    pub surface: SurfaceMetadata,
}

impl RuntimeMetadata {
    /// Build a runtime projection from caller-owned surface metadata.
    #[must_use]
    pub fn from_surface(surface: SurfaceMetadata) -> Self {
        Self { surface }
    }

    /// Return whether this runtime projection carries no caller-owned metadata.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.surface.is_empty()
    }
}

impl From<SurfaceMetadata> for RuntimeMetadata {
    fn from(surface: SurfaceMetadata) -> Self {
        Self::from_surface(surface)
    }
}

/// Metadata validation failures for public surfaces.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SurfaceMetadataError {
    /// A caller attempted to set a Meerkat-owned label key.
    #[error("metadata label key '{key}' is reserved for Meerkat-owned runtime facts")]
    ReservedLabelKey { key: String },
    /// A caller attempted to set a Meerkat-owned top-level app-context key.
    #[error("app_context key '{key}' is reserved for Meerkat-owned runtime facts")]
    ReservedAppContextKey { key: String },
}

/// Check whether a metadata key is reserved for Meerkat-owned facts.
#[must_use]
pub fn is_reserved_meerkat_metadata_key(key: &str) -> bool {
    key == "meerkat" || key.starts_with(MEERKAT_METADATA_PREFIX)
}

/// Check whether a label key is reserved for Meerkat-owned facts.
#[must_use]
pub fn is_reserved_meerkat_label_key(key: &str) -> bool {
    RESERVED_MOB_LABEL_KEYS.contains(&key) || is_reserved_meerkat_metadata_key(key)
}

/// Validate caller-supplied labels.
pub fn validate_public_labels(
    labels: Option<&BTreeMap<String, String>>,
) -> Result<(), SurfaceMetadataError> {
    let Some(labels) = labels else {
        return Ok(());
    };

    for key in labels.keys() {
        if is_reserved_meerkat_label_key(key) {
            return Err(SurfaceMetadataError::ReservedLabelKey { key: key.clone() });
        }
    }

    Ok(())
}

/// Validate top-level caller-supplied app context keys.
pub fn validate_public_app_context(
    app_context: Option<&serde_json::Value>,
) -> Result<(), SurfaceMetadataError> {
    let Some(serde_json::Value::Object(map)) = app_context else {
        return Ok(());
    };

    for key in map.keys() {
        if is_reserved_meerkat_metadata_key(key) {
            return Err(SurfaceMetadataError::ReservedAppContextKey { key: key.clone() });
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn surface_metadata_omits_empty_fields() {
        let encoded = serde_json::to_value(SurfaceMetadata::default()).unwrap();
        assert_eq!(encoded, json!({}));
    }

    #[test]
    fn surface_metadata_round_trips_existing_labels_and_app_context_shape() {
        let metadata = SurfaceMetadata::from_optional_parts(
            Some(BTreeMap::from([(
                "client.thread_id".into(),
                "thread-1".into(),
            )])),
            Some(json!({"client_ref": {"view": "compact"}})),
        );

        let encoded = serde_json::to_value(&metadata).unwrap();
        assert_eq!(
            encoded,
            json!({
                "labels": { "client.thread_id": "thread-1" },
                "app_context": { "client_ref": { "view": "compact" } }
            })
        );
        assert_eq!(
            serde_json::from_value::<SurfaceMetadata>(encoded).unwrap(),
            metadata
        );
    }

    #[test]
    fn public_validation_rejects_meerkat_owned_label_keys() {
        for key in ["mob_id", "role", "meerkat_id", "meerkat.runtime_id"] {
            let metadata = SurfaceMetadata::from_optional_parts(
                Some(BTreeMap::from([(key.to_string(), "spoof".to_string())])),
                None,
            );
            assert!(matches!(
                metadata.validate_public(),
                Err(SurfaceMetadataError::ReservedLabelKey { .. })
            ));
        }
    }

    #[test]
    fn public_validation_rejects_meerkat_owned_app_context_keys() {
        let metadata = SurfaceMetadata::from_optional_parts(
            None,
            Some(json!({
                "meerkat.runtime_id": "spoof",
                "client_ref": "ok"
            })),
        );

        assert!(matches!(
            metadata.validate_public(),
            Err(SurfaceMetadataError::ReservedAppContextKey { .. })
        ));
    }
}
