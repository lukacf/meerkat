//! Auth status projection — poll surface for CLI/REST/RPC/MCP/WASM.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::error::AuthErrorKind;
use super::token_store::PersistedTokens;
use crate::handles::{AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, AuthLeasePhase};
use crate::provider::Provider;

/// Public auth status phase shared by REST, RPC, CLI, and generated SDK wire
/// payloads.
///
/// This is the compatibility projection of typed auth lease truth. Surfaces
/// should map to this enum first and only then emit [`Self::as_public_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AuthStatusPhase {
    Valid,
    Expiring,
    Expired,
    ReauthRequired,
    RefreshFailed,
    Unknown,
}

impl AuthStatusPhase {
    pub const fn as_public_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Expiring => "expiring",
            Self::Expired => "expired",
            Self::ReauthRequired => "reauth_required",
            Self::RefreshFailed => "refresh_failed",
            Self::Unknown => "unknown",
        }
    }

    pub const fn from_lease_phase(phase: Option<AuthLeasePhase>) -> Self {
        match phase {
            Some(AuthLeasePhase::Valid) => Self::Valid,
            Some(AuthLeasePhase::Expiring | AuthLeasePhase::Refreshing) => Self::Expiring,
            Some(AuthLeasePhase::ReauthRequired) => Self::ReauthRequired,
            Some(AuthLeasePhase::Released) | None => Self::Unknown,
        }
    }

    pub fn from_persisted_tokens(now: DateTime<Utc>, tokens: Option<&PersistedTokens>) -> Self {
        match tokens {
            Some(tokens) => Self::from_expires_at(now, tokens.expires_at),
            None => Self::Unknown,
        }
    }

    pub fn from_expires_at(now: DateTime<Utc>, expires_at: Option<DateTime<Utc>>) -> Self {
        match expires_at {
            Some(expires_at) if expires_at <= now => Self::Expired,
            Some(expires_at)
                if expires_at - now
                    < chrono::Duration::seconds(AUTH_LEASE_TTL_REFRESH_WINDOW_SECS as i64) =>
            {
                Self::Expiring
            }
            Some(_) | None => Self::Valid,
        }
    }
}

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
    use chrono::{Duration, TimeZone};

    fn tokens_with_expiry(expires_at: Option<DateTime<Utc>>) -> PersistedTokens {
        PersistedTokens {
            auth_mode: crate::auth::PersistedAuthMode::ApiKey,
            primary_secret: Some("secret".into()),
            refresh_token: None,
            id_token: None,
            expires_at,
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }

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

    #[test]
    fn auth_status_phase_maps_persisted_token_expiry_to_public_values() {
        let now = Utc.with_ymd_and_hms(2026, 4, 28, 12, 0, 0).unwrap();

        let valid = tokens_with_expiry(Some(now + Duration::minutes(10)));
        assert_eq!(
            AuthStatusPhase::from_persisted_tokens(now, Some(&valid)).as_public_str(),
            "valid"
        );

        let expired = tokens_with_expiry(Some(now - Duration::seconds(1)));
        assert_eq!(
            AuthStatusPhase::from_persisted_tokens(now, Some(&expired)).as_public_str(),
            "expired"
        );

        assert_eq!(
            AuthStatusPhase::from_persisted_tokens(now, None).as_public_str(),
            "unknown"
        );
    }

    #[test]
    fn auth_status_phase_maps_typed_lease_phase_to_public_values() {
        assert_eq!(
            AuthStatusPhase::from_lease_phase(Some(AuthLeasePhase::Valid)).as_public_str(),
            "valid"
        );
        assert_eq!(
            AuthStatusPhase::from_lease_phase(Some(AuthLeasePhase::Expiring)).as_public_str(),
            "expiring"
        );
        assert_eq!(
            AuthStatusPhase::from_lease_phase(None).as_public_str(),
            "unknown"
        );
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
