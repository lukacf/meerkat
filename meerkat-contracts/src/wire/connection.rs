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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireConnectionRef {
    pub realm_id: String,
    pub binding_id: String,
}

impl From<meerkat_core::ConnectionRef> for WireConnectionRef {
    fn from(value: meerkat_core::ConnectionRef) -> Self {
        Self {
            realm_id: value.realm_id,
            binding_id: value.binding_id,
        }
    }
}

impl From<WireConnectionRef> for meerkat_core::ConnectionRef {
    fn from(value: WireConnectionRef) -> Self {
        Self {
            realm_id: value.realm_id,
            binding_id: value.binding_id,
        }
    }
}

impl WireConnectionRef {
    pub fn parse(raw: &str) -> Option<Self> {
        let (realm, binding) = raw.split_once(':')?;
        if realm.is_empty() || binding.is_empty() {
            return None;
        }
        Some(Self {
            realm_id: realm.to_string(),
            binding_id: binding.to_string(),
        })
    }
}

impl std::fmt::Display for WireConnectionRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.realm_id, self.binding_id)
    }
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
/// material is NOT wire-projected — callers that want to read secret
/// material have to go through the server-side
/// `auth.profile.get` / `/auth/profiles/:id` endpoints which return
/// typed redacted shapes. `source_kind` is a discriminator for the
/// credential-source variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthProfile {
    pub id: String,
    pub provider: String,
    pub auth_method: String,
    /// Discriminator: "inline_secret" / "env" / "managed_store" /
    /// "external_resolver" / "platform_default" / "command" /
    /// "file_descriptor".
    pub source_kind: String,
    /// Discriminator for the storage variant:
    /// "keyring" / "file" / "auto" / "ephemeral" / "host_managed".
    pub storage_kind: String,
}

impl From<&meerkat_core::AuthProfile> for WireAuthProfile {
    fn from(value: &meerkat_core::AuthProfile) -> Self {
        let source_kind = match &value.source {
            meerkat_core::CredentialSourceSpec::InlineSecret { .. } => "inline_secret",
            meerkat_core::CredentialSourceSpec::Env { .. } => "env",
            meerkat_core::CredentialSourceSpec::ManagedStore { .. } => "managed_store",
            meerkat_core::CredentialSourceSpec::ExternalResolver { .. } => "external_resolver",
            meerkat_core::CredentialSourceSpec::PlatformDefault => "platform_default",
            meerkat_core::CredentialSourceSpec::Command { .. } => "command",
            meerkat_core::CredentialSourceSpec::FileDescriptor { .. } => "file_descriptor",
        };
        let storage_kind = match &value.storage {
            meerkat_core::CredentialStorageSpec::Keyring => "keyring",
            meerkat_core::CredentialStorageSpec::File { .. } => "file",
            meerkat_core::CredentialStorageSpec::Auto => "auto",
            meerkat_core::CredentialStorageSpec::Ephemeral => "ephemeral",
            meerkat_core::CredentialStorageSpec::HostManaged => "host_managed",
        };
        Self {
            id: value.id.clone(),
            provider: value.provider.as_str().to_string(),
            auth_method: value.auth_method.clone(),
            source_kind: source_kind.to_string(),
            storage_kind: storage_kind.to_string(),
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
/// `auth.status.get` / `GET /auth/status/:id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthStatus {
    pub profile_id: String,
    pub provider: String,
    pub auth_method: String,
    /// High-level health: "valid" / "expiring" / "expired" /
    /// "reauth_required" / "refresh_failed" / "unknown".
    pub state: String,
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn connection_ref_roundtrip() {
        let r = meerkat_core::ConnectionRef {
            realm_id: "dev".into(),
            binding_id: "default_openai".into(),
        };
        let w: WireConnectionRef = r.clone().into();
        assert_eq!(w.realm_id, "dev");
        assert_eq!(w.binding_id, "default_openai");
        let back: meerkat_core::ConnectionRef = w.into();
        assert_eq!(back, r);
    }

    #[test]
    fn connection_ref_parse_and_display() {
        let w = WireConnectionRef::parse("dev:default_openai").unwrap();
        assert_eq!(w.realm_id, "dev");
        assert_eq!(w.binding_id, "default_openai");
        assert_eq!(w.to_string(), "dev:default_openai");
        assert!(WireConnectionRef::parse("no-colon").is_none());
        assert!(WireConnectionRef::parse(":empty-realm").is_none());
        assert!(WireConnectionRef::parse("empty-binding:").is_none());
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
    fn auth_profile_discriminators_are_snake_case() {
        use meerkat_core::{AuthProfile, CredentialSourceSpec, CredentialStorageSpec, Provider};
        let cases = [
            (
                CredentialSourceSpec::InlineSecret { secret: "x".into() },
                CredentialStorageSpec::Keyring,
                "inline_secret",
                "keyring",
            ),
            (
                CredentialSourceSpec::ExternalResolver { handle: "x".into() },
                CredentialStorageSpec::Ephemeral,
                "external_resolver",
                "ephemeral",
            ),
            (
                CredentialSourceSpec::PlatformDefault,
                CredentialStorageSpec::Auto,
                "platform_default",
                "auto",
            ),
            (
                CredentialSourceSpec::Command {
                    program: "/bin/sh".into(),
                    args: Vec::new(),
                    cwd: None,
                    env: Default::default(),
                    timeout_ms: 30_000,
                    refresh_interval_ms: None,
                },
                CredentialStorageSpec::HostManaged,
                "command",
                "host_managed",
            ),
            (
                CredentialSourceSpec::FileDescriptor {
                    fd: 3,
                    scope_override: None,
                },
                CredentialStorageSpec::File {
                    path: "/tmp/x".into(),
                },
                "file_descriptor",
                "file",
            ),
        ];
        for (source, storage, expected_source, expected_storage) in cases {
            let ap = AuthProfile {
                id: "a".into(),
                provider: Provider::OpenAI,
                auth_method: "api_key".into(),
                source,
                storage,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            };
            let w: WireAuthProfile = (&ap).into();
            assert_eq!(w.source_kind, expected_source);
            assert_eq!(w.storage_kind, expected_storage);
        }
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
    fn auth_status_serde_roundtrip() {
        let status = WireAuthStatus {
            profile_id: "p".into(),
            provider: "openai".into(),
            auth_method: "managed_chatgpt_oauth".into(),
            state: "valid".into(),
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
