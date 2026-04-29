//! Auth status projection — poll surface for CLI/REST/RPC/MCP/WASM.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::error::AuthErrorKind;
use crate::handles::{AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, AuthLeasePhase, AuthLeaseSnapshot};
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

    pub fn from_lease_snapshot(now: DateTime<Utc>, snapshot: &AuthLeaseSnapshot) -> Self {
        match snapshot.phase {
            Some(AuthLeasePhase::Valid) => Self::from_lease_expires_at(now, snapshot.expires_at),
            Some(AuthLeasePhase::Expiring | AuthLeasePhase::Refreshing) => {
                match snapshot.expires_at {
                    Some(expires_at) if epoch_secs_expired(now, expires_at) => Self::Expired,
                    _ => Self::Expiring,
                }
            }
            Some(AuthLeasePhase::ReauthRequired) => Self::ReauthRequired,
            Some(AuthLeasePhase::Released) | None => Self::Unknown,
        }
    }

    pub fn from_lease_expires_at(now: DateTime<Utc>, expires_at: Option<u64>) -> Self {
        match expires_at {
            Some(expires_at) if epoch_secs_expired(now, expires_at) => Self::Expired,
            Some(expires_at)
                if epoch_secs_until(now, expires_at)
                    < AUTH_LEASE_TTL_REFRESH_WINDOW_SECS as i64 =>
            {
                Self::Expiring
            }
            Some(_) | None => Self::Valid,
        }
    }
}

fn epoch_secs_expired(now: DateTime<Utc>, expires_at: u64) -> bool {
    epoch_secs_until(now, expires_at) <= 0
}

fn epoch_secs_until(now: DateTime<Utc>, expires_at: u64) -> i64 {
    let expires_at = i64::try_from(expires_at).unwrap_or(i64::MAX);
    expires_at.saturating_sub(now.timestamp())
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
    fn auth_status_phase_maps_lease_snapshot_expiry_to_public_values() {
        let now = Utc.with_ymd_and_hms(2026, 4, 28, 12, 0, 0).unwrap();

        let valid = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some((now + Duration::minutes(10)).timestamp() as u64),
        };
        assert_eq!(
            AuthStatusPhase::from_lease_snapshot(now, &valid).as_public_str(),
            "valid"
        );

        let expired = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some((now - Duration::seconds(1)).timestamp() as u64),
        };
        assert_eq!(
            AuthStatusPhase::from_lease_snapshot(now, &expired).as_public_str(),
            "expired"
        );

        let stale_token_without_machine = AuthLeaseSnapshot {
            phase: None,
            expires_at: Some((now + Duration::minutes(10)).timestamp() as u64),
        };
        assert_eq!(
            AuthStatusPhase::from_lease_snapshot(now, &stale_token_without_machine).as_public_str(),
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
            AuthStatusPhase::from_lease_phase(Some(AuthLeasePhase::ReauthRequired)).as_public_str(),
            "reauth_required"
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
