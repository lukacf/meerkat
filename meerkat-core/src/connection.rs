//! Realm-scoped connection contracts: backend profiles, auth profiles,
//! provider bindings, and the ingestion wrapper `RealmConfigSection`.
//!
//! This module owns the cross-cutting runtime shapes used by sessions,
//! factories, and surfaces. Provider-runtime-side typed enums
//! (`OpenAiBackendKind`, `AnthropicAuthMethod`, etc.) live in
//! `meerkat-client/src/providers/*` — `meerkat-core` stays generic and
//! carries `backend_kind` / `auth_method` as strings normalized at the
//! provider-runtime boundary.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::auth::{AuthConstraints, AuthMetadataDefaults};
use crate::provider::Provider;

// ---------------------------------------------------------------------
// Runtime shapes (what providers/surfaces consume at runtime)
// ---------------------------------------------------------------------

/// Session-facing reference to a binding inside a realm.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConnectionRef {
    pub realm_id: String,
    pub binding_id: String,
}

impl ConnectionRef {
    /// Parse a `"<realm>:<binding>"` form. Returns `None` for malformed input.
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

impl std::fmt::Display for ConnectionRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.realm_id, self.binding_id)
    }
}

/// Backend profile: where requests go and which backend contract applies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BackendProfile {
    pub id: String,
    pub provider: Provider,
    pub backend_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub options: serde_json::Value,
}

/// Auth profile: how credentials are obtained, stored, refreshed, constrained.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthProfile {
    pub id: String,
    pub provider: Provider,
    pub auth_method: String,
    pub source: CredentialSourceSpec,
    #[serde(default)]
    pub storage: CredentialStorageSpec,
    #[serde(default)]
    pub constraints: AuthConstraints,
    #[serde(default)]
    pub metadata_defaults: AuthMetadataDefaults,
}

/// Where credentials come from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialSourceSpec {
    InlineSecret {
        secret: String,
    },
    Env {
        env: String,
    },
    ManagedStore {
        profile: String,
    },
    ExternalResolver {
        handle: String,
    },
    PlatformDefault,
    /// External command that prints a bearer token on stdout. Reference:
    /// Codex `external_bearer.rs:17-157`. The runner lives in
    /// `meerkat-client/src/auth_store/command.rs`.
    Command {
        program: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        /// Timeout for the subprocess in milliseconds.
        #[serde(default = "default_command_timeout_ms")]
        timeout_ms: u64,
        /// Optional cached-token lifetime. `None` disables caching.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_interval_ms: Option<u64>,
    },
    /// Read credentials from an inherited file descriptor (Claude Code
    /// pattern for sandboxed host-injected tokens).
    FileDescriptor {
        fd: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope_override: Option<String>,
    },
}

fn default_command_timeout_ms() -> u64 {
    30_000
}

/// How credentials are stored by the host.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialStorageSpec {
    Keyring,
    File {
        path: PathBuf,
    },
    #[default]
    Auto,
    Ephemeral,
    HostManaged,
}

/// Policy overrides carried on a binding.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BindingPolicy {
    #[serde(default)]
    pub allow_auth_override: bool,
    #[serde(default)]
    pub require_metadata_account: bool,
    #[serde(default)]
    pub require_metadata_workspace: bool,
}

/// A binding is what sessions actually refer to: one backend + one auth
/// profile, plus policy and an optional default model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderBinding {
    pub id: String,
    pub backend_profile: String,
    pub auth_profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub policy: BindingPolicy,
}

/// Realm-scoped set of backends, auth profiles, and bindings.
///
/// Produced by [`RealmConnectionSet::from_config`] from a
/// [`RealmConfigSection`] ingested from TOML.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealmConnectionSet {
    pub realm_id: String,
    pub backends: BTreeMap<String, BackendProfile>,
    pub auth_profiles: BTreeMap<String, AuthProfile>,
    pub bindings: BTreeMap<String, ProviderBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_binding: Option<String>,
}

impl RealmConnectionSet {
    /// Validate and materialize a realm connection set from its config
    /// section. Normalizes provider strings into the typed
    /// [`Provider`] enum and verifies that every binding references
    /// existing backend and auth profiles whose providers agree.
    pub fn from_config(
        realm_id: &str,
        section: &RealmConfigSection,
    ) -> Result<Self, ProviderBindingError> {
        let mut backends: BTreeMap<String, BackendProfile> = BTreeMap::new();
        for (id, cfg) in &section.backend {
            let provider = Provider::parse_strict(&cfg.provider)
                .ok_or_else(|| ProviderBindingError::UnknownProviderName(cfg.provider.clone()))?;
            let backend = BackendProfile {
                id: id.clone(),
                provider,
                backend_kind: cfg.backend_kind.clone(),
                base_url: cfg.base_url.clone(),
                options: cfg.options.clone(),
            };
            // id uniqueness within a single BTreeMap key space is
            // guaranteed by the map itself; no extra check needed.
            backends.insert(id.clone(), backend);
        }

        let mut auth_profiles: BTreeMap<String, AuthProfile> = BTreeMap::new();
        for (id, cfg) in &section.auth {
            let provider = Provider::parse_strict(&cfg.provider)
                .ok_or_else(|| ProviderBindingError::UnknownProviderName(cfg.provider.clone()))?;
            let profile = AuthProfile {
                id: id.clone(),
                provider,
                auth_method: cfg.auth_method.clone(),
                source: cfg.source.clone(),
                storage: cfg.storage.clone().unwrap_or_default(),
                constraints: cfg.constraints.clone(),
                metadata_defaults: cfg.metadata_defaults.clone(),
            };
            auth_profiles.insert(id.clone(), profile);
        }

        let mut bindings: BTreeMap<String, ProviderBinding> = BTreeMap::new();
        for (id, cfg) in &section.binding {
            let backend = backends
                .get(&cfg.backend_profile)
                .ok_or_else(|| ProviderBindingError::UnknownBackend(cfg.backend_profile.clone()))?;
            let auth = auth_profiles
                .get(&cfg.auth_profile)
                .ok_or_else(|| ProviderBindingError::UnknownAuth(cfg.auth_profile.clone()))?;
            if backend.provider != auth.provider {
                return Err(ProviderBindingError::ProviderMismatch {
                    binding: id.clone(),
                    backend: backend.provider,
                    auth: auth.provider,
                });
            }
            let binding = ProviderBinding {
                id: id.clone(),
                backend_profile: cfg.backend_profile.clone(),
                auth_profile: cfg.auth_profile.clone(),
                default_model: cfg.default_model.clone(),
                policy: cfg.policy.clone(),
            };
            bindings.insert(id.clone(), binding);
        }

        Ok(Self {
            realm_id: realm_id.to_string(),
            backends,
            auth_profiles,
            bindings,
            default_binding: section.default_binding.clone(),
        })
    }

    /// Resolve a binding by id. Returns the binding plus its referenced
    /// backend and auth profiles.
    pub fn lookup_binding(
        &self,
        id: &str,
    ) -> Result<(&ProviderBinding, &BackendProfile, &AuthProfile), ProviderBindingError> {
        let binding = self
            .bindings
            .get(id)
            .ok_or_else(|| ProviderBindingError::UnknownBinding(id.to_string()))?;
        let backend = self
            .backends
            .get(&binding.backend_profile)
            .ok_or_else(|| ProviderBindingError::UnknownBackend(binding.backend_profile.clone()))?;
        let auth = self
            .auth_profiles
            .get(&binding.auth_profile)
            .ok_or_else(|| ProviderBindingError::UnknownAuth(binding.auth_profile.clone()))?;
        Ok((binding, backend, auth))
    }
}

/// Validation / reference-resolution errors for a realm connection set.
///
/// The plan originally listed a `DuplicateId(String)` variant; it's been
/// omitted because `RealmConfigSection` uses `BTreeMap<String, ...>` for
/// backends/auth/bindings, so duplicate ids within one category are
/// impossible at ingestion time. Cross-category id sharing is harmless
/// (lookups are category-keyed). If a future code path constructs a
/// `RealmConfigSection` programmatically and needs duplicate detection,
/// add the variant back alongside the check.
#[derive(Debug, Clone, Error, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderBindingError {
    #[error("unknown binding: {0}")]
    UnknownBinding(String),
    #[error("unknown backend: {0}")]
    UnknownBackend(String),
    #[error("unknown auth: {0}")]
    UnknownAuth(String),
    #[error("provider mismatch on binding {binding}: backend={backend:?} auth={auth:?}")]
    ProviderMismatch {
        binding: String,
        backend: Provider,
        auth: Provider,
    },
    #[error("unknown provider name: {0}")]
    UnknownProviderName(String),
}

// ---------------------------------------------------------------------
// Ingestion shapes (what TOML / config files deserialize into)
// ---------------------------------------------------------------------

/// Ingestion wrapper for `[realm.<id>.*]` TOML tables.
///
/// The singular nouns `backend`/`auth`/`binding` match TOML dotted-key
/// notation (`[realm.dev.backend.openai_default]`) so that one `.backend.X`
/// table becomes one entry in the `backend` map.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealmConfigSection {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub backend: BTreeMap<String, BackendProfileConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub auth: BTreeMap<String, AuthProfileConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub binding: BTreeMap<String, ProviderBindingConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_binding: Option<String>,
}

/// Serialized backend profile (pre-normalization).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BackendProfileConfig {
    pub provider: String,
    pub backend_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub options: serde_json::Value,
}

/// Serialized auth profile (pre-normalization).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthProfileConfig {
    pub provider: String,
    pub auth_method: String,
    pub source: CredentialSourceSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<CredentialStorageSpec>,
    #[serde(default)]
    pub constraints: AuthConstraints,
    #[serde(default)]
    pub metadata_defaults: AuthMetadataDefaults,
}

/// Serialized binding (pre-normalization).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderBindingConfig {
    pub backend_profile: String,
    pub auth_profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub policy: BindingPolicy,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn connection_ref_parse_display_roundtrip() {
        let c = ConnectionRef::parse("dev:default_openai").expect("valid");
        assert_eq!(c.realm_id, "dev");
        assert_eq!(c.binding_id, "default_openai");
        assert_eq!(c.to_string(), "dev:default_openai");
    }

    #[test]
    fn connection_ref_parse_rejects_malformed() {
        assert!(ConnectionRef::parse("no_colon").is_none());
        assert!(ConnectionRef::parse(":foo").is_none());
        assert!(ConnectionRef::parse("dev:").is_none());
        assert!(ConnectionRef::parse("").is_none());
    }

    #[test]
    fn credential_source_spec_serde() {
        for src in [
            CredentialSourceSpec::InlineSecret {
                secret: "sk-x".into(),
            },
            CredentialSourceSpec::Env {
                env: "OPENAI_API_KEY".into(),
            },
            CredentialSourceSpec::ManagedStore {
                profile: "default".into(),
            },
            CredentialSourceSpec::ExternalResolver {
                handle: "desktop".into(),
            },
            CredentialSourceSpec::PlatformDefault,
        ] {
            let s = serde_json::to_string(&src).unwrap();
            let back: CredentialSourceSpec = serde_json::from_str(&s).unwrap();
            assert_eq!(back, src);
        }
    }

    #[test]
    fn credential_source_spec_rejects_unknown_kind() {
        let bad = r#"{"kind":"nonexistent","foo":"bar"}"#;
        let err = serde_json::from_str::<CredentialSourceSpec>(bad).unwrap_err();
        assert!(
            err.to_string().contains("nonexistent") || err.to_string().contains("unknown variant"),
            "serde error should mention unknown variant: {err}",
        );
    }

    #[test]
    fn credential_storage_spec_serde_roundtrip_all_variants() {
        for storage in [
            CredentialStorageSpec::Keyring,
            CredentialStorageSpec::File {
                path: std::path::PathBuf::from("/tmp/meerkat-secret.json"),
            },
            CredentialStorageSpec::Auto,
            CredentialStorageSpec::Ephemeral,
            CredentialStorageSpec::HostManaged,
        ] {
            let s = serde_json::to_string(&storage).unwrap();
            let back: CredentialStorageSpec = serde_json::from_str(&s).unwrap();
            assert_eq!(back, storage);
        }
        assert_eq!(
            CredentialStorageSpec::default(),
            CredentialStorageSpec::Auto
        );
    }

    #[test]
    fn from_config_empty_section_yields_empty_set() {
        let section = RealmConfigSection::default();
        let set = RealmConnectionSet::from_config("dev", &section).expect("empty section is valid");
        assert_eq!(set.realm_id, "dev");
        assert!(set.backends.is_empty());
        assert!(set.auth_profiles.is_empty());
        assert!(set.bindings.is_empty());
        assert_eq!(set.default_binding, None);
    }

    #[test]
    fn lookup_binding_returns_unknown_binding() {
        let set = RealmConnectionSet::from_config("dev", &RealmConfigSection::default())
            .expect("empty section valid");
        let err = set
            .lookup_binding("missing")
            .expect_err("empty set has no bindings");
        assert_eq!(err, ProviderBindingError::UnknownBinding("missing".into()));
    }

    #[test]
    fn realm_config_section_serde_empty() {
        let section = RealmConfigSection::default();
        let s = serde_json::to_string(&section).unwrap();
        // All maps empty + no default_binding → empty object.
        assert_eq!(s, "{}");
    }

    #[test]
    fn realm_config_section_serde_populated() {
        // `default_binding` appears BEFORE any section header so that TOML
        // treats it as a top-level field rather than a key inside the last
        // subsection.
        let toml_input = r#"
default_binding = "default_openai"

[backend.openai_default]
provider = "openai"
backend_kind = "openai_api"
base_url = "https://api.openai.com"

[auth.openai_api_key]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "OPENAI_API_KEY" }

[binding.default_openai]
backend_profile = "openai_default"
auth_profile = "openai_api_key"
default_model = "gpt-5.1"
"#;
        let section: RealmConfigSection = toml::from_str(toml_input).unwrap();
        assert_eq!(section.backend.len(), 1);
        assert_eq!(section.auth.len(), 1);
        assert_eq!(section.binding.len(), 1);
        assert_eq!(section.default_binding.as_deref(), Some("default_openai"));
        assert_eq!(
            section.backend["openai_default"].base_url.as_deref(),
            Some("https://api.openai.com"),
        );
    }
}
