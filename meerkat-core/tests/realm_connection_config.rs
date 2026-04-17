#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! T1 top-down integration test for provider auth v2 (Phase 1).
//!
//! Exercises the full ingestion chain: canonical TOML → `Config` serde →
//! `Config.realm["dev"]` → `RealmConnectionSet::from_config` →
//! `RealmConnectionSet::lookup_binding`. Originally written during the
//! T1+T2 scaffolding phase with red-at-assertion reds; flipped green once
//! Phase 1 leaf slices (from_config + lookup_binding) landed.
//!
//! See /Users/luka/.claude/plans/yes-make-a-plan-shimmying-bengio.md.

use meerkat_core::provider::Provider;
use meerkat_core::{
    BackendProfile, Config, CredentialSourceSpec, ProviderBinding, ProviderBindingError,
    RealmConfigSection, RealmConnectionSet,
};

/// Canonical TOML fixture used across the happy-path and negative tests.
/// Intentionally mixes new `[realm.*]` tables with flat `[providers]`
/// config to prove the legacy path is unaffected.
const CANONICAL_TOML: &str = r#"
max_tokens = 4096

[providers]
api_keys = { openai = "flat-key" }

[realm.dev.backend.openai_default]
provider = "openai"
backend_kind = "openai_api"
base_url = "https://api.openai.com"

[realm.dev.backend.anthropic_default]
provider = "anthropic"
backend_kind = "anthropic_api"
base_url = "https://api.anthropic.com"

[realm.dev.auth.openai_api_key]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "OPENAI_API_KEY" }
storage = { kind = "host_managed" }

[realm.dev.auth.anthropic_api_key]
provider = "anthropic"
auth_method = "api_key"
source = { kind = "env", env = "ANTHROPIC_API_KEY" }
storage = { kind = "host_managed" }

[realm.dev.binding.default_openai]
backend_profile = "openai_default"
auth_profile = "openai_api_key"
default_model = "gpt-5.1"

[realm.dev.binding.default_anthropic]
backend_profile = "anthropic_default"
auth_profile = "anthropic_api_key"
default_model = "claude-opus-4-7"

[realm.dev]
default_binding = "default_openai"
"#;

fn load_config() -> Config {
    toml::from_str::<Config>(CANONICAL_TOML).expect("canonical TOML parses into Config")
}

fn dev_section(config: &Config) -> &RealmConfigSection {
    config
        .realm
        .get("dev")
        .expect("realm.dev section present after TOML ingestion")
}

#[test]
fn canonical_toml_populates_config_realm_without_regressing_flat_provider() {
    // Proves that the C1 contract boundary — TOML ingest → `Config.realm` —
    // works at the serde layer regardless of whether from_config is
    // implemented. Flat [provider]/[providers] tables still parse.
    let config = load_config();

    // Flat legacy path untouched: top-level fields and flat [providers]
    // still parse alongside the new [realm.*] tables.
    assert_eq!(config.max_tokens, 4096);
    assert_eq!(
        config
            .providers
            .api_keys
            .as_ref()
            .and_then(|m| m.get("openai")),
        Some(&"flat-key".to_string()),
    );

    // New realm path populated.
    let section = dev_section(&config);
    assert_eq!(section.backend.len(), 2, "two backends in fixture");
    assert_eq!(section.auth.len(), 2, "two auth profiles in fixture");
    assert_eq!(section.binding.len(), 2, "two bindings in fixture");
    assert_eq!(
        section.default_binding.as_deref(),
        Some("default_openai"),
        "realm default binding",
    );

    // Spot-check one backend and one auth to confirm dotted-key nesting.
    let backend = section
        .backend
        .get("openai_default")
        .expect("openai_default backend");
    assert_eq!(backend.provider, "openai");
    assert_eq!(backend.backend_kind, "openai_api");
    assert_eq!(backend.base_url.as_deref(), Some("https://api.openai.com"));

    let auth = section
        .auth
        .get("openai_api_key")
        .expect("openai_api_key auth profile");
    assert_eq!(auth.provider, "openai");
    assert_eq!(auth.auth_method, "api_key");
    assert_eq!(
        auth.source,
        CredentialSourceSpec::Env {
            env: "OPENAI_API_KEY".into()
        }
    );
}

#[test]
fn from_config_happy_path_resolves_binding() {
    // T1 top-down happy-path assertion.
    let config = load_config();
    let section = dev_section(&config);

    let set = RealmConnectionSet::from_config("dev", section)
        .expect("Phase 1 leaves L1.9/L1.10 should make this return Ok");

    let (binding, backend, auth) = set
        .lookup_binding("default_openai")
        .expect("default_openai is a valid binding in the fixture");
    check_openai_default(binding, backend);
    assert_eq!(auth.provider, Provider::OpenAI);
    assert_eq!(auth.auth_method, "api_key");

    let (anthropic_binding, anthropic_backend, anthropic_auth) = set
        .lookup_binding("default_anthropic")
        .expect("default_anthropic is a valid binding in the fixture");
    assert_eq!(
        anthropic_binding.default_model.as_deref(),
        Some("claude-opus-4-7")
    );
    assert_eq!(anthropic_backend.provider, Provider::Anthropic);
    assert_eq!(anthropic_auth.provider, Provider::Anthropic);
}

fn check_openai_default(binding: &ProviderBinding, backend: &BackendProfile) {
    assert_eq!(binding.id, "default_openai");
    assert_eq!(binding.backend_profile, "openai_default");
    assert_eq!(binding.auth_profile, "openai_api_key");
    assert_eq!(binding.default_model.as_deref(), Some("gpt-5.1"));
    assert_eq!(backend.provider, Provider::OpenAI);
    assert_eq!(backend.backend_kind, "openai_api");
    assert_eq!(backend.base_url.as_deref(), Some("https://api.openai.com"));
}

#[test]
fn from_config_rejects_unknown_provider_name() {
    // K1/C1 negative: string provider names are normalized at from_config
    // time. An unknown name surfaces a typed error.
    let mut section = RealmConfigSection::default();
    section.backend.insert(
        "bad".into(),
        meerkat_core::BackendProfileConfig {
            provider: "notaprovider".into(),
            backend_kind: "openai_api".into(),
            base_url: None,
            options: serde_json::Value::Null,
        },
    );

    let result = RealmConnectionSet::from_config("dev", &section);
    assert_eq!(
        result,
        Err(ProviderBindingError::UnknownProviderName(
            "notaprovider".into()
        )),
        "Phase 1 L1.10 should surface UnknownProviderName for non-canonical provider strings",
    );
}

#[test]
fn from_config_rejects_binding_pointing_at_missing_backend() {
    let mut section = RealmConfigSection::default();
    section.auth.insert(
        "openai_api_key".into(),
        meerkat_core::AuthProfileConfig {
            provider: "openai".into(),
            auth_method: "api_key".into(),
            source: CredentialSourceSpec::Env {
                env: "OPENAI_API_KEY".into(),
            },
            storage: None,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        "default_openai".into(),
        meerkat_core::ProviderBindingConfig {
            backend_profile: "does_not_exist".into(),
            auth_profile: "openai_api_key".into(),
            default_model: None,
            policy: Default::default(),
        },
    );

    let result = RealmConnectionSet::from_config("dev", &section);
    assert_eq!(
        result,
        Err(ProviderBindingError::UnknownBackend(
            "does_not_exist".into()
        )),
        "Phase 1 L1.10 should reject bindings referencing missing backends",
    );
}

#[test]
fn from_config_rejects_binding_pointing_at_missing_auth() {
    let mut section = RealmConfigSection::default();
    section.backend.insert(
        "openai_default".into(),
        meerkat_core::BackendProfileConfig {
            provider: "openai".into(),
            backend_kind: "openai_api".into(),
            base_url: None,
            options: serde_json::Value::Null,
        },
    );
    section.binding.insert(
        "default_openai".into(),
        meerkat_core::ProviderBindingConfig {
            backend_profile: "openai_default".into(),
            auth_profile: "does_not_exist".into(),
            default_model: None,
            policy: Default::default(),
        },
    );

    let result = RealmConnectionSet::from_config("dev", &section);
    assert_eq!(
        result,
        Err(ProviderBindingError::UnknownAuth("does_not_exist".into())),
        "Phase 1 L1.10 should reject bindings referencing missing auth profiles",
    );
}

#[test]
fn from_config_detects_provider_mismatch() {
    // Binding points to an openai backend and an anthropic auth profile.
    let mut section = RealmConfigSection::default();
    section.backend.insert(
        "openai_default".into(),
        meerkat_core::BackendProfileConfig {
            provider: "openai".into(),
            backend_kind: "openai_api".into(),
            base_url: None,
            options: serde_json::Value::Null,
        },
    );
    section.auth.insert(
        "anthropic_api_key".into(),
        meerkat_core::AuthProfileConfig {
            provider: "anthropic".into(),
            auth_method: "api_key".into(),
            source: CredentialSourceSpec::Env {
                env: "ANTHROPIC_API_KEY".into(),
            },
            storage: None,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        "mismatched".into(),
        meerkat_core::ProviderBindingConfig {
            backend_profile: "openai_default".into(),
            auth_profile: "anthropic_api_key".into(),
            default_model: None,
            policy: Default::default(),
        },
    );

    let result = RealmConnectionSet::from_config("dev", &section);
    assert_eq!(
        result,
        Err(ProviderBindingError::ProviderMismatch {
            binding: "mismatched".into(),
            backend: Provider::OpenAI,
            auth: Provider::Anthropic,
        }),
        "Phase 1 L1.10 should detect backend/auth provider mismatch",
    );
}

#[test]
fn lookup_binding_rejects_unknown_id_after_from_config_succeeds() {
    // Two-stage negative test: requires L1.10 to return Ok, then L1.9 to
    // return UnknownBinding. Fails at the first unwrap during scaffolding.
    let config = load_config();
    let section = dev_section(&config);
    let set = RealmConnectionSet::from_config("dev", section).expect("happy path depends on L1.10");
    let result = set.lookup_binding("not_a_binding");
    assert_eq!(
        result.err(),
        Some(ProviderBindingError::UnknownBinding("not_a_binding".into())),
        "Phase 1 L1.9 should surface UnknownBinding",
    );
}

#[test]
fn default_config_has_empty_realm_map() {
    // Coexistence: an untouched flat config has no realms. Proves
    // Config.realm defaults to empty without breaking the legacy path.
    let config = Config::default();
    assert!(config.realm.is_empty());
}
