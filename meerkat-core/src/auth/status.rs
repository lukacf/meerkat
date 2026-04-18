//! Auth status projection — poll surface for CLI/REST/RPC/MCP/WASM.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::error::AuthErrorKind;
use crate::provider::Provider;

/// Status snapshot of a resolved (or unresolved) auth profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthStatus {
    pub profile_id: String,
    pub provider: Provider,
    pub backend_kind: String,
    pub auth_method: String,
    pub source_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    #[serde(default)]
    pub needs_reauth: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<AuthErrorSummary>,
}

/// Compact projection of the last auth error. Used in AuthStatus poll surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthErrorSummary {
    pub kind: AuthErrorKind,
    pub message: String,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub occurred_at: DateTime<Utc>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn auth_status_roundtrip() {
        let status = AuthStatus {
            profile_id: "openai_api_key".into(),
            provider: Provider::OpenAI,
            backend_kind: "openai_api".into(),
            auth_method: "api_key".into(),
            source_label: "env:OPENAI_API_KEY".into(),
            expires_at: None,
            account_id: None,
            workspace_id: None,
            organization_id: Some("org-abc".into()),
            needs_reauth: false,
            last_error: None,
        };
        let s = serde_json::to_string(&status).unwrap();
        let back: AuthStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, status);
    }

    #[test]
    fn auth_error_summary_roundtrip() {
        let summary = AuthErrorSummary {
            kind: AuthErrorKind::MissingSecret,
            message: "env var not set".into(),
            occurred_at: Utc::now(),
        };
        let s = serde_json::to_string(&summary).unwrap();
        let back: AuthErrorSummary = serde_json::from_str(&s).unwrap();
        assert_eq!(back.kind, summary.kind);
        assert_eq!(back.message, summary.message);
    }

    #[cfg(feature = "schema")]
    #[test]
    fn auth_status_emits_json_schema() {
        // Proves that the JsonSchema derive works without panicking and
        // produces a non-trivial schema object. L1.5 requirement.
        let schema = schemars::schema_for!(AuthStatus);
        let json = serde_json::to_value(&schema).unwrap();
        // The schema must have a top-level object with properties.
        assert!(json.is_object());
        let props = json
            .get("properties")
            .expect("AuthStatus schema has properties");
        assert!(props.get("profile_id").is_some());
        assert!(props.get("provider").is_some());
        assert!(props.get("needs_reauth").is_some());
    }
}
