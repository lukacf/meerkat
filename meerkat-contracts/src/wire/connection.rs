//! Wire-facing projections of the realm connection types
//! (`ConnectionRef`, `BackendProfile`, `AuthProfile`, `ProviderBinding`,
//! `RealmConnectionSet`, `AuthStatus`).
//!
//! `meerkat-core` owns the domain types; this module re-projects them
//! with wire-friendly field shapes and adds `#[schemars]` attributes
//! for SDK codegen. The projection is lossy by design: `Provider` is
//! string-typed on the wire, DateTime is ISO-8601 string, and sensitive
//! secret material never crosses a wire boundary (the wire projection
//! only carries IDs + non-secret metadata).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Wire projection of [`meerkat_core::ConnectionRef`].
///
/// Pure structural shape — no `"realm:binding"` string form. Wave-b deleted
/// `parse` and `Display` on both the core type and the wire projection so
/// the colon-joined form cannot travel across wire boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireConnectionRef {
    pub realm: meerkat_core::connection::RealmId,
    pub binding: meerkat_core::connection::BindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<meerkat_core::connection::ProfileId>,
}

impl From<meerkat_core::ConnectionRef> for WireConnectionRef {
    fn from(value: meerkat_core::ConnectionRef) -> Self {
        Self {
            realm: value.realm,
            binding: value.binding,
            profile: value.profile,
        }
    }
}

impl From<WireConnectionRef> for meerkat_core::ConnectionRef {
    fn from(value: WireConnectionRef) -> Self {
        Self {
            realm: value.realm,
            binding: value.binding,
            profile: value.profile,
        }
    }
}

/// Request payload for `auth/profile/list` and `realm/get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealmIdParams {
    pub realm_id: String,
}

/// Request payload for binding-scoped auth methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BindingIdParams {
    pub realm_id: String,
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

/// Request payload for `auth/profile/create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreateProfileParams {
    pub realm_id: String,
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub auth_method: String,
    pub secret: String,
}

/// Request payload for `auth/login/start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LoginStartParams {
    pub provider: String,
    pub redirect_uri: String,
}

/// Request payload for `auth/login/complete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LoginCompleteParams {
    pub provider: String,
    pub code: String,
    pub state: String,
    pub redirect_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

/// Request payload for `auth/login/device_start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeviceStartParams {
    pub provider: String,
}

/// Request payload for `auth/login/device_complete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeviceCompleteParams {
    pub provider: String,
    pub device_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

/// Request payload for `auth/login/provision_api_key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProvisionApiKeyParams {
    /// Access token acquired from a prior Console-OAuth flow.
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

/// Wire projection of [`meerkat_core::BackendProfile`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireBackendProfile {
    pub id: String,
    /// Provider as a normalized string: `"openai"` / `"anthropic"` /
    /// `"gemini"` / `"self_hosted"`. Wire types don't carry the strict
    /// `Provider` enum so consumers aren't forced to update their
    /// schema when we add new providers.
    pub provider: String,
    pub backend_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub options: serde_json::Value,
}

impl From<&meerkat_core::BackendProfile> for WireBackendProfile {
    fn from(value: &meerkat_core::BackendProfile) -> Self {
        Self {
            id: value.id.clone(),
            provider: value.provider.as_str().to_string(),
            backend_kind: value.backend_kind.clone(),
            base_url: value.base_url.clone(),
            options: value.options.clone(),
        }
    }
}

/// Wire projection of [`meerkat_core::AuthProfile`]. Sensitive credential
/// material is NOT wire-projected; callers that inspect profile metadata use
/// the server-side `auth.profile.get` RPC or the
/// `GET /auth/bindings/{binding_id}` REST path; both return typed redacted
/// shapes. `source_kind` is a
/// discriminator for the credential-source variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthProfile {
    pub id: String,
    pub provider: String,
    pub auth_method: String,
    /// Discriminator: "inline_secret" / "managed_store" / "env" /
    /// "external_resolver" / "platform_default" / "command" /
    /// "file_descriptor".
    pub source_kind: String,
}

impl From<&meerkat_core::AuthProfile> for WireAuthProfile {
    fn from(value: &meerkat_core::AuthProfile) -> Self {
        let source_kind = match &value.source {
            meerkat_core::CredentialSourceSpec::InlineSecret { .. } => "inline_secret",
            meerkat_core::CredentialSourceSpec::ManagedStore => "managed_store",
            meerkat_core::CredentialSourceSpec::Env { .. } => "env",
            meerkat_core::CredentialSourceSpec::ExternalResolver { .. } => "external_resolver",
            meerkat_core::CredentialSourceSpec::PlatformDefault => "platform_default",
            meerkat_core::CredentialSourceSpec::Command { .. } => "command",
            meerkat_core::CredentialSourceSpec::FileDescriptor { .. } => "file_descriptor",
        };
        Self {
            id: value.id.clone(),
            provider: value.provider.as_str().to_string(),
            auth_method: value.auth_method.clone(),
            source_kind: source_kind.to_string(),
        }
    }
}

/// Wire projection of [`meerkat_core::ProviderBinding`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireProviderBinding {
    pub id: String,
    pub backend_profile: String,
    pub auth_profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub allow_auth_override: bool,
    #[serde(default)]
    pub require_metadata_account: bool,
    #[serde(default)]
    pub require_metadata_workspace: bool,
}

impl From<&meerkat_core::ProviderBinding> for WireProviderBinding {
    fn from(value: &meerkat_core::ProviderBinding) -> Self {
        Self {
            id: value.id.clone(),
            backend_profile: value.backend_profile.clone(),
            auth_profile: value.auth_profile.clone(),
            default_model: value.default_model.clone(),
            allow_auth_override: value.policy.allow_auth_override,
            require_metadata_account: value.policy.require_metadata_account,
            require_metadata_workspace: value.policy.require_metadata_workspace,
        }
    }
}

/// Wire projection of [`meerkat_core::RealmConnectionSet`]. Returned
/// from the `realm/get` / `GET /realm/:id` endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRealmConnectionSet {
    pub realm_id: String,
    pub backends: BTreeMap<String, WireBackendProfile>,
    pub auth_profiles: BTreeMap<String, WireAuthProfile>,
    pub bindings: BTreeMap<String, WireProviderBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_binding: Option<String>,
}

impl From<&meerkat_core::RealmConnectionSet> for WireRealmConnectionSet {
    fn from(value: &meerkat_core::RealmConnectionSet) -> Self {
        Self {
            realm_id: value.realm_id.clone(),
            backends: value
                .backends
                .iter()
                .map(|(k, v)| (k.clone(), WireBackendProfile::from(v)))
                .collect(),
            auth_profiles: value
                .auth_profiles
                .iter()
                .map(|(k, v)| (k.clone(), WireAuthProfile::from(v)))
                .collect(),
            bindings: value
                .bindings
                .iter()
                .map(|(k, v)| (k.clone(), WireProviderBinding::from(v)))
                .collect(),
            default_binding: value.default_binding.clone(),
        }
    }
}

// --- Auth status projection -------------------------------------------

/// Stable wire kind for auth errors. Mirrors `meerkat_core::AuthErrorKind`
/// on the wire as a normalized string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireAuthError {
    MissingSecret,
    UnsupportedCombination { backend: String, auth: String },
    MissingRequiredMetadata { field: String },
    WorkspaceMismatch,
    Expired,
    RefreshFailed { detail: String },
    InteractiveLoginRequired,
    HostOwnedUnavailable,
    Io { detail: String },
    Other { detail: String },
}

impl From<meerkat_core::AuthError> for WireAuthError {
    fn from(value: meerkat_core::AuthError) -> Self {
        match value {
            meerkat_core::AuthError::MissingSecret => Self::MissingSecret,
            meerkat_core::AuthError::UnsupportedCombination { backend, auth } => {
                Self::UnsupportedCombination { backend, auth }
            }
            meerkat_core::AuthError::MissingRequiredMetadata(field) => {
                Self::MissingRequiredMetadata { field }
            }
            meerkat_core::AuthError::WorkspaceMismatch => Self::WorkspaceMismatch,
            meerkat_core::AuthError::Expired => Self::Expired,
            meerkat_core::AuthError::RefreshFailed(detail) => Self::RefreshFailed { detail },
            meerkat_core::AuthError::InteractiveLoginRequired => Self::InteractiveLoginRequired,
            meerkat_core::AuthError::HostOwnedUnavailable => Self::HostOwnedUnavailable,
            meerkat_core::AuthError::Io(detail) => Self::Io { detail },
            meerkat_core::AuthError::Other(detail) => Self::Other { detail },
        }
    }
}

/// Wire projection of the auth-profile status. Returned from
/// `auth.status.get` / `GET /auth/bindings/{binding_id}/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthStatus {
    pub profile_id: String,
    pub provider: String,
    pub auth_method: String,
    /// High-level health projected from typed auth-lease lifecycle truth.
    pub state: meerkat_core::AuthStatusPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub last_refresh_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Most recent auth error, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<WireAuthError>,
}

// --- Auth REST response envelopes ------------------------------------

/// Identifies a binding inside a realm on the wire. Shared by every
/// auth REST response that returns a `{realm_id, binding_id, connection_ref}`
/// trio. Built from a typed [`meerkat_core::ConnectionRef`] so the three
/// fields always agree; the `realm_id`/`binding_id` strings carry the
/// slug form for wire consumers that key by string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireBindingIdentity {
    pub realm_id: String,
    pub binding_id: String,
    pub connection_ref: WireConnectionRef,
}

impl From<&meerkat_core::ConnectionRef> for WireBindingIdentity {
    fn from(cref: &meerkat_core::ConnectionRef) -> Self {
        Self {
            realm_id: cref.realm.to_string(),
            binding_id: cref.binding.to_string(),
            connection_ref: WireConnectionRef::from(cref.clone()),
        }
    }
}

/// `POST /auth/profiles` (create) success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthProfileCreated {
    #[serde(flatten)]
    pub identity: WireBindingIdentity,
    pub profile_id: String,
    pub provider: String,
    pub auth_method: String,
    pub stored: bool,
}

/// `GET /auth/bindings/{binding_id}` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthProfileDetail {
    pub connection_ref: WireConnectionRef,
    pub binding_id: String,
    pub profile_id: String,
    pub auth_profile: WireAuthProfile,
}

/// `DELETE /auth/bindings/{binding_id}` /
/// `POST /auth/bindings/{binding_id}/logout` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthProfileCleared {
    #[serde(flatten)]
    pub identity: WireBindingIdentity,
    pub profile_id: String,
    pub cleared: bool,
}

/// `POST /auth/login/start` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireLoginStart {
    pub authorize_url: String,
    pub state: String,
    pub redirect_uri: String,
    pub provider: String,
}

/// `POST /auth/login/complete` / ready leg of device-code success body.
///
/// The optional `state` field distinguishes the flat `POST
/// /auth/login/complete` response (no `state` set) from the device-code
/// ready leg (`state = "ready"`) which is part of the pending/slow_down/
/// access_denied/expired/ready tagged protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireLoginReady {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(flatten)]
    pub identity: WireBindingIdentity,
    pub profile_id: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub has_refresh_token: bool,
    pub scopes: Vec<String>,
}

/// `POST /auth/login/device/start` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireDeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
    pub provider: String,
}

/// `auth/login/device_complete` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WireDeviceCompleteResult {
    Pending,
    SlowDown,
    AccessDenied,
    Expired,
    Ready {
        #[serde(flatten)]
        identity: Box<WireBindingIdentity>,
        profile_id: String,
        provider: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_at: Option<String>,
        has_refresh_token: bool,
        scopes: Vec<String>,
    },
}

/// `auth/login/provision_api_key` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireProvisionApiKeyResult {
    #[serde(flatten)]
    pub identity: WireBindingIdentity,
    pub profile_id: String,
    pub provider: String,
    pub auth_mode: String,
    pub has_api_key: bool,
    pub scopes: Vec<String>,
}

/// Realm summary entry returned by `GET /realms`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRealmSummary {
    pub realm_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_binding: Option<String>,
    pub backend_count: usize,
    pub auth_profile_count: usize,
    pub binding_count: usize,
}

/// `GET /realms` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRealmList {
    pub realms: Vec<WireRealmSummary>,
}

/// `GET /auth/profiles` success body — realm-scoped lists of backend,
/// auth, and binding profiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthProfilesList {
    pub realm_id: String,
    pub auth_profiles: Vec<WireAuthProfile>,
    pub backend_profiles: Vec<WireBackendProfile>,
    pub bindings: Vec<WireProviderBinding>,
}

/// `GET /auth/bindings/{binding_id}/status` success body. Richer than
/// [`WireAuthStatus`] — also carries `realm_id` / `binding_id` /
/// `connection_ref` / `has_refresh_token` so the caller can key by
/// binding directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthStatusDetail {
    #[serde(flatten)]
    pub identity: WireBindingIdentity,
    pub profile_id: String,
    pub provider: String,
    pub auth_method: String,
    pub state: meerkat_core::AuthStatusPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_refresh_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub has_refresh_token: bool,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn connection_ref_roundtrip() {
        let r = meerkat_core::ConnectionRef {
            realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::connection::BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let w: WireConnectionRef = r.clone().into();
        assert_eq!(w.realm.as_str(), "dev");
        assert_eq!(w.binding.as_str(), "default_openai");
        assert!(w.profile.is_none());
        let back: meerkat_core::ConnectionRef = w.into();
        assert_eq!(back, r);
    }

    #[test]
    fn connection_ref_wire_json_has_no_string_form() {
        let w = WireConnectionRef {
            realm: meerkat_core::connection::RealmId::parse("prod").unwrap(),
            binding: meerkat_core::connection::BindingId::parse("openai_main").unwrap(),
            profile: Some(meerkat_core::connection::ProfileId::parse("ci").unwrap()),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains("\"realm\":\"prod\""));
        assert!(json.contains("\"binding\":\"openai_main\""));
        assert!(json.contains("\"profile\":\"ci\""));
        // No colon-joined form anywhere.
        assert!(!json.contains("prod:openai_main"));
    }

    #[test]
    fn backend_profile_projects_provider_as_string() {
        let bp = meerkat_core::BackendProfile {
            id: "openai_api".into(),
            provider: meerkat_core::Provider::OpenAI,
            backend_kind: "openai_api".into(),
            base_url: None,
            options: serde_json::Value::Null,
        };
        let w: WireBackendProfile = (&bp).into();
        assert_eq!(w.provider, "openai");
    }

    #[test]
    fn auth_error_projects_to_tagged_variants() {
        use meerkat_core::AuthError;
        let e = AuthError::RefreshFailed("timeout".into());
        let w: WireAuthError = e.into();
        let s = serde_json::to_string(&w).unwrap();
        assert!(s.contains("\"kind\":\"refresh_failed\""));
        assert!(s.contains("\"detail\":\"timeout\""));
    }

    #[test]
    fn device_complete_result_serializes_terminal_and_ready_shapes() {
        let pending = serde_json::to_value(WireDeviceCompleteResult::Pending).unwrap();
        assert_eq!(pending, serde_json::json!({ "state": "pending" }));

        let cref = meerkat_core::ConnectionRef {
            realm: meerkat_core::connection::RealmId::parse("prod").unwrap(),
            binding: meerkat_core::connection::BindingId::parse("anthropic_main").unwrap(),
            profile: None,
        };
        let ready = serde_json::to_value(WireDeviceCompleteResult::Ready {
            identity: Box::new(WireBindingIdentity::from(&cref)),
            profile_id: "console".to_string(),
            provider: "anthropic".to_string(),
            expires_at: Some("2026-04-29T00:00:00Z".to_string()),
            has_refresh_token: true,
            scopes: vec!["org:create_api_key".to_string()],
        })
        .unwrap();

        assert_eq!(ready["state"], "ready");
        assert_eq!(ready["realm_id"], "prod");
        assert_eq!(ready["binding_id"], "anthropic_main");
        assert_eq!(ready["connection_ref"]["realm"], "prod");
        assert_eq!(ready["connection_ref"]["binding"], "anthropic_main");
        assert_eq!(ready["profile_id"], "console");
        assert_eq!(ready["has_refresh_token"], true);
    }

    #[test]
    fn provision_api_key_result_serializes_binding_identity() {
        let cref = meerkat_core::ConnectionRef {
            realm: meerkat_core::connection::RealmId::parse("prod").unwrap(),
            binding: meerkat_core::connection::BindingId::parse("anthropic_main").unwrap(),
            profile: Some(meerkat_core::connection::ProfileId::parse("console").unwrap()),
        };
        let value = serde_json::to_value(WireProvisionApiKeyResult {
            identity: WireBindingIdentity::from(&cref),
            profile_id: "console".to_string(),
            provider: "anthropic".to_string(),
            auth_mode: "oauth_to_api_key".to_string(),
            has_api_key: true,
            scopes: vec!["user:profile".to_string()],
        })
        .unwrap();

        assert_eq!(value["realm_id"], "prod");
        assert_eq!(value["binding_id"], "anthropic_main");
        assert_eq!(value["connection_ref"]["profile"], "console");
        assert_eq!(value["auth_mode"], "oauth_to_api_key");
        assert_eq!(value["has_api_key"], true);
    }

    #[test]
    fn auth_status_serde_roundtrip() {
        let status = WireAuthStatus {
            profile_id: "p".into(),
            provider: "openai".into(),
            auth_method: "managed_chatgpt_oauth".into(),
            state: meerkat_core::AuthStatusPhase::Valid,
            expires_at: Some(chrono::Utc::now()),
            last_refresh_at: None,
            account_id: Some("acct_x".into()),
            last_error: None,
        };
        let s = serde_json::to_string(&status).unwrap();
        let back: WireAuthStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, status);
    }
}
