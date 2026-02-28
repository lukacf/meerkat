//! Wire session types.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use meerkat_core::{SessionId, SessionInfo, SessionSummary};

/// Canonical session info for wire protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireSessionInfo {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
    pub is_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_assistant_text: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl From<SessionInfo> for WireSessionInfo {
    fn from(info: SessionInfo) -> Self {
        Self {
            session_id: info.session_id,
            session_ref: None,
            created_at: info
                .created_at
                .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            updated_at: info
                .updated_at
                .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            message_count: info.message_count,
            is_active: info.is_active,
            last_assistant_text: info.last_assistant_text,
            labels: info.labels,
        }
    }
}

/// Canonical session summary for wire protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireSessionSummary {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
    pub total_tokens: u64,
    pub is_active: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl From<SessionSummary> for WireSessionSummary {
    fn from(summary: SessionSummary) -> Self {
        Self {
            session_id: summary.session_id,
            session_ref: None,
            created_at: summary
                .created_at
                .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            updated_at: summary
                .updated_at
                .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            message_count: summary.message_count,
            total_tokens: summary.total_tokens,
            is_active: summary.is_active,
            labels: summary.labels,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::time_compat::SystemTime;

    #[test]
    fn test_wire_session_summary_labels_roundtrip() {
        let mut labels = BTreeMap::new();
        labels.insert("env".to_string(), "prod".to_string());
        labels.insert("team".to_string(), "infra".to_string());

        let wire = WireSessionSummary {
            session_id: SessionId::new(),
            session_ref: None,
            created_at: 1000,
            updated_at: 2000,
            message_count: 5,
            total_tokens: 100,
            is_active: true,
            labels: labels.clone(),
        };
        let json = serde_json::to_string(&wire).unwrap();
        let parsed: WireSessionSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.labels, labels);
    }

    #[test]
    fn test_wire_session_summary_empty_labels_omitted() {
        let wire = WireSessionSummary {
            session_id: SessionId::new(),
            session_ref: None,
            created_at: 1000,
            updated_at: 2000,
            message_count: 0,
            total_tokens: 0,
            is_active: false,
            labels: BTreeMap::new(),
        };
        let json = serde_json::to_string(&wire).unwrap();
        assert!(
            !json.contains("\"labels\""),
            "empty labels should be omitted from JSON"
        );
    }

    #[test]
    fn test_wire_session_info_labels_roundtrip() {
        let mut labels = BTreeMap::new();
        labels.insert("role".to_string(), "orchestrator".to_string());

        let wire = WireSessionInfo {
            session_id: SessionId::new(),
            session_ref: None,
            created_at: 1000,
            updated_at: 2000,
            message_count: 3,
            is_active: true,
            last_assistant_text: None,
            labels: labels.clone(),
        };
        let json = serde_json::to_string(&wire).unwrap();
        let parsed: WireSessionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.labels, labels);
    }

    #[test]
    fn test_wire_session_info_from_session_info_maps_labels() {
        let mut labels = BTreeMap::new();
        labels.insert("env".to_string(), "staging".to_string());

        let info = SessionInfo {
            session_id: SessionId::new(),
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            message_count: 2,
            is_active: false,
            last_assistant_text: Some("hello".to_string()),
            labels: labels.clone(),
        };
        let wire: WireSessionInfo = info.into();
        assert_eq!(wire.labels, labels);
    }

    #[test]
    fn test_wire_session_summary_from_session_summary_maps_labels() {
        let mut labels = BTreeMap::new();
        labels.insert("project".to_string(), "meerkat".to_string());

        let summary = SessionSummary {
            session_id: SessionId::new(),
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            message_count: 10,
            total_tokens: 500,
            is_active: true,
            labels: labels.clone(),
        };
        let wire: WireSessionSummary = summary.into();
        assert_eq!(wire.labels, labels);
    }

    #[test]
    fn test_wire_session_info_backward_compat_no_labels() {
        let json = r#"{
            "session_id": "019405c8-1234-7000-8000-000000000001",
            "created_at": 1000,
            "updated_at": 2000,
            "message_count": 0,
            "is_active": false
        }"#;
        let parsed: WireSessionInfo = serde_json::from_str(json).unwrap();
        assert!(parsed.labels.is_empty());
    }

    #[test]
    fn test_wire_session_summary_backward_compat_no_labels() {
        let json = r#"{
            "session_id": "019405c8-1234-7000-8000-000000000001",
            "created_at": 1000,
            "updated_at": 2000,
            "message_count": 0,
            "total_tokens": 0,
            "is_active": false
        }"#;
        let parsed: WireSessionSummary = serde_json::from_str(json).unwrap();
        assert!(parsed.labels.is_empty());
    }
}
