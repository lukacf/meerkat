//! Phase 4c T12 contract proof — wire-type round-trips.
//!
//! Plan §Top-down integration tests T12 (meerkat-contracts/tests/
//! connection_ref_wire.rs) asserts that every wire projection the SDK
//! codegen consumes round-trips through `serde_json`, and — under the
//! `schema` feature — emits a non-trivial JsonSchema. Unit tests in
//! `src/wire/connection.rs` already exercise the Rust↔wire From/Into
//! direction; this file is the cross-boundary proof that the JSON shape
//! the SDKs see is stable and the schemars derivation compiles.
//!
//! Dogma §17: "Surfaces are skins, not authorities". SDK types must be
//! faithful projections of domain truth, validated here before codegen.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_contracts::wire::{
    WireAuthError, WireAuthProfile, WireAuthStatus, WireBackendProfile, WireConnectionRef,
    WireProviderBinding, WireRealmConnectionSet,
};

fn sample_connection_ref() -> meerkat_core::ConnectionRef {
    meerkat_core::ConnectionRef {
        realm_id: "dev".to_string(),
        binding_id: "default_openai".to_string(),
    }
}

fn sample_backend_profile() -> meerkat_core::BackendProfile {
    meerkat_core::BackendProfile {
        id: "openai_api".to_string(),
        provider: meerkat_core::Provider::OpenAI,
        backend_kind: "openai_api".to_string(),
        base_url: Some("https://api.openai.com".to_string()),
        options: serde_json::json!({"region": "us-east-1"}),
    }
}

fn sample_auth_profile() -> meerkat_core::AuthProfile {
    meerkat_core::AuthProfile {
        id: "prod_env_key".to_string(),
        provider: meerkat_core::Provider::OpenAI,
        auth_method: "api_key".to_string(),
        source: meerkat_core::CredentialSourceSpec::Env {
            env: "OPENAI_API_KEY".to_string(),
            fallback: Vec::new(),
        },
        storage: meerkat_core::CredentialStorageSpec::Ephemeral,
        constraints: Default::default(),
        metadata_defaults: Default::default(),
    }
}

fn sample_provider_binding() -> meerkat_core::ProviderBinding {
    meerkat_core::ProviderBinding {
        id: "default".to_string(),
        backend_profile: "openai_api".to_string(),
        auth_profile: "prod_env_key".to_string(),
        default_model: Some("gpt-5.2".to_string()),
        policy: Default::default(),
    }
}

#[test]
fn connection_ref_serde_roundtrip() {
    let domain = sample_connection_ref();
    let wire: WireConnectionRef = domain.clone().into();
    let s = serde_json::to_string(&wire).unwrap();
    assert!(s.contains("\"realm_id\":\"dev\""));
    assert!(s.contains("\"binding_id\":\"default_openai\""));
    let back: WireConnectionRef = serde_json::from_str(&s).unwrap();
    let redomain: meerkat_core::ConnectionRef = back.into();
    assert_eq!(redomain, domain);
}

#[test]
fn backend_profile_wire_carries_provider_as_string() {
    let bp = sample_backend_profile();
    let wire: WireBackendProfile = (&bp).into();
    let s = serde_json::to_string(&wire).unwrap();
    assert!(s.contains("\"provider\":\"openai\""));
    assert!(s.contains("\"backend_kind\":\"openai_api\""));
    let back: WireBackendProfile = serde_json::from_str(&s).unwrap();
    assert_eq!(back, wire);
}

#[test]
fn auth_profile_wire_carries_source_and_storage_discriminator() {
    let ap = sample_auth_profile();
    let wire: WireAuthProfile = (&ap).into();
    let s = serde_json::to_string(&wire).unwrap();
    assert!(s.contains("\"source_kind\":\"env\""));
    assert!(s.contains("\"storage_kind\":\"ephemeral\""));
    let back: WireAuthProfile = serde_json::from_str(&s).unwrap();
    assert_eq!(back, wire);
}

#[test]
fn provider_binding_wire_flattens_policy_flags() {
    let pb = sample_provider_binding();
    let wire: WireProviderBinding = (&pb).into();
    let s = serde_json::to_string(&wire).unwrap();
    assert!(s.contains("\"default_model\":\"gpt-5.2\""));
    let back: WireProviderBinding = serde_json::from_str(&s).unwrap();
    assert_eq!(back, wire);
}

#[test]
fn realm_connection_set_roundtrips_with_populated_maps() {
    let mut backends = std::collections::BTreeMap::new();
    backends.insert("openai_api".to_string(), sample_backend_profile());
    let mut auth_profiles = std::collections::BTreeMap::new();
    auth_profiles.insert("prod_env_key".to_string(), sample_auth_profile());
    let mut bindings = std::collections::BTreeMap::new();
    bindings.insert("default".to_string(), sample_provider_binding());

    let realm = meerkat_core::RealmConnectionSet {
        realm_id: "dev".to_string(),
        backends,
        auth_profiles,
        bindings,
        default_binding: Some("default".to_string()),
    };
    let wire: WireRealmConnectionSet = (&realm).into();
    let s = serde_json::to_string(&wire).unwrap();
    let back: WireRealmConnectionSet = serde_json::from_str(&s).unwrap();
    assert_eq!(back, wire);
    assert!(back.backends.contains_key("openai_api"));
    assert!(back.auth_profiles.contains_key("prod_env_key"));
    assert!(back.bindings.contains_key("default"));
    assert_eq!(back.default_binding.as_deref(), Some("default"));
}

#[test]
fn auth_error_tagged_discriminator_matches_domain() {
    let cases: Vec<(meerkat_core::AuthError, &str)> = vec![
        (meerkat_core::AuthError::MissingSecret, "missing_secret"),
        (meerkat_core::AuthError::Expired, "expired"),
        (
            meerkat_core::AuthError::RefreshFailed("timeout".into()),
            "refresh_failed",
        ),
        (
            meerkat_core::AuthError::InteractiveLoginRequired,
            "interactive_login_required",
        ),
        (
            meerkat_core::AuthError::WorkspaceMismatch,
            "workspace_mismatch",
        ),
    ];
    for (domain, expected_tag) in cases {
        let wire: WireAuthError = domain.into();
        let s = serde_json::to_string(&wire).unwrap();
        assert!(
            s.contains(&format!("\"kind\":\"{expected_tag}\"")),
            "AuthError discriminator missing tag '{expected_tag}': {s}"
        );
        let back: WireAuthError = serde_json::from_str(&s).unwrap();
        assert_eq!(back, wire);
    }
}

#[test]
fn auth_status_wire_with_error_roundtrips() {
    let status = WireAuthStatus {
        binding_id: "p1".to_string(),
        provider: "openai".to_string(),
        auth_method: "managed_chatgpt_oauth".to_string(),
        state: "reauth_required".to_string(),
        expires_at: None,
        last_refresh_at: None,
        account_id: Some("acct_42".to_string()),
        last_error: Some(WireAuthError::InteractiveLoginRequired),
    };
    let s = serde_json::to_string(&status).unwrap();
    assert!(s.contains("\"state\":\"reauth_required\""));
    assert!(s.contains("\"kind\":\"interactive_login_required\""));
    let back: WireAuthStatus = serde_json::from_str(&s).unwrap();
    assert_eq!(back, status);
}

#[cfg(feature = "schema")]
mod schema_emission {
    use super::*;

    fn schema_json<T: schemars::JsonSchema>() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(T)).unwrap()
    }

    #[test]
    fn connection_ref_schema_has_realm_and_binding() {
        let s = schema_json::<WireConnectionRef>();
        let props = s.pointer("/properties").expect("schema has properties");
        assert!(props.get("realm_id").is_some());
        assert!(props.get("binding_id").is_some());
    }

    #[test]
    fn auth_profile_schema_has_discriminator_fields() {
        let s = schema_json::<WireAuthProfile>();
        let props = s.pointer("/properties").expect("schema has properties");
        assert!(props.get("source_kind").is_some());
        assert!(props.get("storage_kind").is_some());
        assert!(props.get("auth_method").is_some());
    }

    #[test]
    fn realm_connection_set_schema_nests_backends_and_bindings() {
        let s = schema_json::<WireRealmConnectionSet>();
        let props = s.pointer("/properties").expect("schema has properties");
        assert!(props.get("backends").is_some());
        assert!(props.get("auth_profiles").is_some());
        assert!(props.get("bindings").is_some());
    }

    #[test]
    fn auth_status_schema_has_state_and_optional_error() {
        let s = schema_json::<WireAuthStatus>();
        let props = s.pointer("/properties").expect("schema has properties");
        assert!(props.get("state").is_some());
        assert!(props.get("last_error").is_some());
    }
}
