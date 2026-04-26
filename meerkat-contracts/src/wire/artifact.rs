use meerkat_core::{ArtifactId, ArtifactListFilter, ArtifactPayload, ArtifactRecord};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct ArtifactListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub label_equals: BTreeMap<String, String>,
}

impl ArtifactListParams {
    pub fn into_filter(self) -> ArtifactListFilter {
        ArtifactListFilter {
            session_id: self.session_id,
            label_equals: self.label_equals,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct ArtifactIdParams {
    pub artifact_id: ArtifactId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct ArtifactDownloadParams {
    pub artifact_id: ArtifactId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_media_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct ArtifactListResult {
    pub artifacts: Vec<ArtifactRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct ArtifactDownloadResult {
    pub record: ArtifactRecord,
    pub payload: ArtifactPayload,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::{ArtifactContentHandle, ArtifactType, BlobId, BlobRef};
    use serde_json::json;

    #[test]
    fn artifact_id_params_use_typed_artifact_id() {
        let params: ArtifactIdParams =
            serde_json::from_value(json!({"artifact_id": "artifact-1"})).unwrap();

        assert_eq!(params.artifact_id.as_str(), "artifact-1");
    }

    #[test]
    fn artifact_id_params_reject_path_like_values() {
        let err = serde_json::from_value::<ArtifactIdParams>(json!({"artifact_id": "/tmp/report"}))
            .unwrap_err();

        assert!(err.to_string().contains("invalid artifact id"));
    }

    #[test]
    fn artifact_list_params_project_to_core_filter() {
        let params: ArtifactListParams = serde_json::from_value(json!({
            "session_id": "session-a",
            "label_equals": {"client.thread_id": "thread-a"}
        }))
        .unwrap();

        let filter = params.into_filter();
        assert_eq!(filter.session_id.as_deref(), Some("session-a"));
        assert_eq!(
            filter
                .label_equals
                .get("client.thread_id")
                .map(String::as_str),
            Some("thread-a")
        );
    }

    #[test]
    fn artifact_record_wire_round_trips_without_paths() {
        let record = ArtifactRecord::new(
            ArtifactId::new("artifact-1").unwrap(),
            ArtifactType::Json,
            "Report".to_string(),
            "application/json".to_string(),
            2,
            None,
            ArtifactContentHandle::Blob(BlobRef {
                blob_id: BlobId::new("sha256:report"),
                media_type: "application/json".to_string(),
            }),
        )
        .unwrap();

        let encoded = serde_json::to_string(&record).unwrap();
        assert!(encoded.contains("artifact-1"));
        assert!(!encoded.contains("/tmp"));
        assert!(!encoded.contains("path"));
    }
}
