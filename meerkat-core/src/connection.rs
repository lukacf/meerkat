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

/// Error returned when a realm/binding/profile slug fails validation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdentityError {
    #[error("identity slug is empty")]
    Empty,
    #[error(
        "identity slug contains invalid character {0:?}; must be ASCII alphanumeric or one of '-', '_', '.'"
    )]
    InvalidChar(char),
}

fn validate_slug(raw: &str) -> Result<(), IdentityError> {
    if raw.is_empty() {
        return Err(IdentityError::Empty);
    }
    for ch in raw.chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.') {
            return Err(IdentityError::InvalidChar(ch));
        }
    }
    Ok(())
}

macro_rules! slug_newtype {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn parse(raw: impl Into<String>) -> Result<Self, IdentityError> {
                let raw = raw.into();
                validate_slug(&raw)?;
                Ok(Self(raw))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdentityError;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                Self::parse(s)
            }
        }

        impl From<$name> for String {
            fn from(v: $name) -> String {
                v.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

slug_newtype!(RealmId, "Opaque slug identifying a realm.");
slug_newtype!(
    BindingId,
    "Opaque slug identifying a binding inside a realm."
);
slug_newtype!(
    ProfileId,
    "Opaque slug identifying an auth profile override on a connection."
);

/// Session-facing reference to a binding inside a realm.
///
/// `ConnectionRef` is purely structural — it does NOT carry a `"realm:binding"`
/// string form. Wave-b deleted `parse` and `Display` so that no code path
/// accidentally ferries the opaque join through the runtime. CLI input that
/// arrives as `"realm:binding[:profile]"` must be split at the CLI boundary
/// and constructed field-by-field.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConnectionRef {
    pub realm: RealmId,
    pub binding: BindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileId>,
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

/// Auth profile: how credentials are obtained, refreshed, constrained.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthProfile {
    pub id: String,
    pub provider: Provider,
    pub auth_method: String,
    pub source: CredentialSourceSpec,
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
        /// Ordered fallback env var names consulted when `env` is
        /// unset. Used for providers with multiple well-known names
        /// (e.g. Gemini falls back to `GOOGLE_API_KEY` when
        /// `GEMINI_API_KEY` is absent). The resolver's RKAT_*-prefix
        /// precedence applies to each name in turn.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fallback: Vec<String>,
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

    /// Synthesize a default [`RealmConnectionSet`] for a given provider,
    /// sourcing credentials from a well-known env var. Used by surface
    /// factories when no explicit realm config exists but the user has
    /// set `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `GEMINI_API_KEY` in
    /// the environment — the synthesized realm is consumed by the same
    /// `ProviderRuntimeRegistry` path as explicit realms, so env-var auth
    /// and realm-config auth share one resolution pipeline.
    ///
    /// Returns a realm with id `"env_default"` containing one binding
    /// `"default"` pointing at:
    /// - BackendProfile `"default"` with the provider's default
    ///   backend_kind and base_url=None (provider client uses its default).
    /// - AuthProfile `"default"` with `source = Env { env: <ENV_VAR> }`,
    ///   `auth_method = "api_key"`.
    ///
    /// The ENV_VAR name is per-provider:
    /// - Anthropic: `ANTHROPIC_API_KEY`
    /// - OpenAI:   `OPENAI_API_KEY`
    /// - Google:   `GEMINI_API_KEY`
    ///
    /// Callers should also honor `RKAT_*`-prefixed overrides via
    /// `ResolverEnvironment::with_process_env()`; that lookup is applied
    /// inside the registry's resolve path when it reads the env source.
    pub fn synthesize_env_default(provider: Provider) -> Self {
        Self::synthesize_default(provider, None)
    }

    /// Synthesize a default realm with an inline secret instead of an env
    /// lookup. Used when callers have already read the api key from a
    /// config file (legacy credential-map path).
    pub fn synthesize_inline_default(provider: Provider, secret: String) -> Self {
        Self::synthesize_default(provider, Some(secret))
    }

    fn synthesize_default(provider: Provider, inline_secret: Option<String>) -> Self {
        let (backend_kind, env_var, fallback) = match provider {
            Provider::Anthropic => ("anthropic_api", "ANTHROPIC_API_KEY", vec![]),
            Provider::OpenAI => ("openai_api", "OPENAI_API_KEY", vec![]),
            Provider::Gemini => (
                "google_genai",
                "GEMINI_API_KEY",
                vec!["GOOGLE_API_KEY".to_string()],
            ),
            Provider::SelfHosted => ("self_hosted", "RKAT_SELF_HOSTED_API_KEY", vec![]),
            Provider::Other => ("other_api", "RKAT_OTHER_API_KEY", vec![]),
        };
        let backend = BackendProfile {
            id: "default".to_string(),
            provider,
            backend_kind: backend_kind.to_string(),
            base_url: None,
            options: serde_json::Value::Null,
        };
        let source = match inline_secret {
            Some(secret) => CredentialSourceSpec::InlineSecret { secret },
            None => CredentialSourceSpec::Env {
                env: env_var.to_string(),
                fallback,
            },
        };
        let auth = AuthProfile {
            id: "default".to_string(),
            provider,
            auth_method: "api_key".to_string(),
            source,
            constraints: AuthConstraints::default(),
            metadata_defaults: AuthMetadataDefaults::default(),
        };
        let binding = ProviderBinding {
            id: "default".to_string(),
            backend_profile: "default".to_string(),
            auth_profile: "default".to_string(),
            default_model: None,
            policy: BindingPolicy::default(),
        };
        let mut backends = BTreeMap::new();
        backends.insert("default".to_string(), backend);
        let mut auth_profiles = BTreeMap::new();
        auth_profiles.insert("default".to_string(), auth);
        let mut bindings = BTreeMap::new();
        bindings.insert("default".to_string(), binding);
        Self {
            realm_id: "env_default".to_string(),
            backends,
            auth_profiles,
            bindings,
            default_binding: Some("default".to_string()),
        }
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

    /// Resolve a typed connection reference. `ConnectionRef.profile`, when
    /// present, overrides the binding's configured auth profile while keeping
    /// the binding's backend and policy authoritative.
    pub fn lookup_connection_ref(
        &self,
        connection_ref: &ConnectionRef,
    ) -> Result<(&ProviderBinding, &BackendProfile, &AuthProfile), ProviderBindingError> {
        let binding = self
            .bindings
            .get(connection_ref.binding.as_str())
            .ok_or_else(|| {
                ProviderBindingError::UnknownBinding(connection_ref.binding.to_string())
            })?;
        let backend = self
            .backends
            .get(&binding.backend_profile)
            .ok_or_else(|| ProviderBindingError::UnknownBackend(binding.backend_profile.clone()))?;
        let auth_profile_id = connection_ref
            .profile
            .as_ref()
            .map(ProfileId::as_str)
            .unwrap_or(binding.auth_profile.as_str());
        let auth = self
            .auth_profiles
            .get(auth_profile_id)
            .ok_or_else(|| ProviderBindingError::UnknownAuth(auth_profile_id.to_string()))?;
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

impl RealmConfigSection {
    /// Programmatic constructor for a realm populated from per-provider
    /// inline api keys. Used by surfaces (notably the WASM browser
    /// runtime) that receive credentials as plain strings at bootstrap
    /// and need to translate them into the realm-based config shape
    /// consumed by `AgentFactory::build_agent`.
    ///
    /// For each (provider, secret) pair, emits:
    ///   - a `BackendProfileConfig { provider, backend_kind: "<p>_api" }`
    ///   - an `AuthProfileConfig` with `CredentialSourceSpec::InlineSecret`
    ///   - a `ProviderBindingConfig` wiring the two
    ///
    /// The first provider in the input list becomes the
    /// `default_binding` so that build_agent's connection_ref-less
    /// code path can resolve through this realm. Plan §6.10 replacement
    /// for the deleted `ProviderSettings.api_keys` map.
    pub fn from_inline_api_keys(entries: &[(&str, &str)]) -> Self {
        let mut backend = BTreeMap::new();
        let mut auth = BTreeMap::new();
        let mut binding = BTreeMap::new();
        let mut default_binding: Option<String> = None;

        for (idx, (provider, secret)) in entries.iter().enumerate() {
            let id = format!("default_{provider}");
            let backend_kind = match *provider {
                "anthropic" => "anthropic_api",
                "openai" => "openai_api",
                "gemini" | "google" => "google_genai",
                other => other,
            };
            backend.insert(
                id.clone(),
                BackendProfileConfig {
                    provider: provider.to_string(),
                    backend_kind: backend_kind.to_string(),
                    base_url: None,
                    options: serde_json::Value::Null,
                },
            );
            auth.insert(
                id.clone(),
                AuthProfileConfig {
                    provider: provider.to_string(),
                    auth_method: "api_key".to_string(),
                    source: CredentialSourceSpec::InlineSecret {
                        secret: (*secret).to_string(),
                    },
                    constraints: AuthConstraints::default(),
                    metadata_defaults: AuthMetadataDefaults::default(),
                },
            );
            binding.insert(
                id.clone(),
                ProviderBindingConfig {
                    backend_profile: id.clone(),
                    auth_profile: id.clone(),
                    default_model: None,
                    policy: BindingPolicy::default(),
                },
            );
            if idx == 0 {
                default_binding = Some(id);
            }
        }

        Self {
            backend,
            auth,
            binding,
            default_binding,
        }
    }
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
    fn connection_ref_is_purely_structural() {
        let c = ConnectionRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        assert_eq!(c.realm.as_str(), "dev");
        assert_eq!(c.binding.as_str(), "default_openai");
        assert!(c.profile.is_none());
    }

    #[test]
    fn connection_ref_serde_roundtrip_with_profile() {
        let c = ConnectionRef {
            realm: RealmId::parse("prod").unwrap(),
            binding: BindingId::parse("gpt5").unwrap(),
            profile: Some(ProfileId::parse("override").unwrap()),
        };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"realm\":\"prod\""));
        assert!(s.contains("\"binding\":\"gpt5\""));
        assert!(s.contains("\"profile\":\"override\""));
        let back: ConnectionRef = serde_json::from_str(&s).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn connection_ref_profile_overrides_binding_auth_profile() {
        let toml = r#"
realm_id = "prod"
default_binding = "primary"

[backend.openai_default]
provider = "openai"
backend_kind = "openai_api"
base_url = "https://api.openai.com/v1"

[auth.default_profile]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "OPENAI_API_KEY" }

[auth.override_profile]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "OVERRIDE_OPENAI_API_KEY" }

[binding.primary]
backend_profile = "openai_default"
auth_profile = "default_profile"
"#;
        let section: RealmConfigSection = toml::from_str(toml).unwrap();
        let realm = RealmConnectionSet::from_config("prod", &section).unwrap();
        let connection_ref = ConnectionRef {
            realm: RealmId::parse("prod").unwrap(),
            binding: BindingId::parse("primary").unwrap(),
            profile: Some(ProfileId::parse("override_profile").unwrap()),
        };

        let (_binding, _backend, auth) = realm.lookup_connection_ref(&connection_ref).unwrap();
        assert_eq!(auth.id, "override_profile");
    }

    #[test]
    fn identity_slugs_reject_invalid_characters() {
        assert!(RealmId::parse("").is_err());
        assert!(BindingId::parse("bad space").is_err());
        assert!(ProfileId::parse("bad:colon").is_err());
        assert!(RealmId::parse("dev").is_ok());
        assert!(BindingId::parse("openai_default.v1").is_ok());
    }

    #[test]
    fn credential_source_spec_serde() {
        for src in [
            CredentialSourceSpec::InlineSecret {
                secret: "sk-x".into(),
            },
            CredentialSourceSpec::Env {
                env: "OPENAI_API_KEY".into(),
                fallback: Vec::new(),
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
