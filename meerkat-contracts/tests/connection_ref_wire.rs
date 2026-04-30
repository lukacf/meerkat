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
    WireAuthError, WireAuthProfile, WireAuthStatus, WireAuthStatusDetail, WireBackendProfile,
    WireBindingIdentity, WireConnectionRef, WireProviderBinding, WireRealmConnectionSet,
};

fn sample_connection_ref() -> meerkat_core::ConnectionRef {
    meerkat_core::ConnectionRef {
        realm: meerkat_core::connection::RealmId::parse("dev").expect("valid realm"),
        binding: meerkat_core::connection::BindingId::parse("default_openai")
            .expect("valid binding"),
        profile: None,
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
    assert!(s.contains("\"realm\":\"dev\""));
    assert!(s.contains("\"binding\":\"default_openai\""));
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
fn auth_profile_wire_carries_source_discriminator() {
    let ap = sample_auth_profile();
    let wire: WireAuthProfile = (&ap).into();
    let s = serde_json::to_string(&wire).unwrap();
    assert!(s.contains("\"source_kind\":\"env\""));
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
        profile_id: "p1".to_string(),
        provider: "openai".to_string(),
        auth_method: "managed_chatgpt_oauth".to_string(),
        state: meerkat_core::AuthStatusPhase::ReauthRequired,
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

#[test]
fn auth_status_detail_wire_flattens_binding_identity() {
    let connection_ref = sample_connection_ref();
    let status = WireAuthStatusDetail {
        identity: WireBindingIdentity::from(&connection_ref),
        profile_id: "prod_env_key".to_string(),
        provider: "openai".to_string(),
        auth_method: "api_key".to_string(),
        state: meerkat_core::AuthStatusPhase::Valid,
        expires_at: None,
        last_refresh_at: Some("2026-04-28T00:00:00Z".to_string()),
        account_id: Some("acct_42".to_string()),
        has_refresh_token: true,
    };
    let value = serde_json::to_value(&status).unwrap();
    assert_eq!(value["realm_id"], "dev");
    assert_eq!(value["binding_id"], "default_openai");
    assert_eq!(value["connection_ref"]["realm"], "dev");
    assert_eq!(value["profile_id"], "prod_env_key");
    assert_eq!(value["has_refresh_token"], true);
    let back: WireAuthStatusDetail = serde_json::from_value(value).unwrap();
    assert_eq!(back.identity.realm_id, status.identity.realm_id);
    assert_eq!(back.identity.binding_id, status.identity.binding_id);
    assert_eq!(back.identity.connection_ref, status.identity.connection_ref);
    assert_eq!(back.profile_id, status.profile_id);
    assert_eq!(back.has_refresh_token, status.has_refresh_token);
}

#[test]
fn auth_status_wire_rejects_unknown_lifecycle_state() {
    let status = serde_json::json!({
        "profile_id": "p1",
        "provider": "openai",
        "auth_method": "managed_chatgpt_oauth",
        "state": "credential_present"
    });
    assert!(
        serde_json::from_value::<WireAuthStatus>(status).is_err(),
        "WireAuthStatus state must be typed lifecycle truth, not an arbitrary string"
    );

    let detail = serde_json::json!({
        "realm_id": "dev",
        "binding_id": "default_openai",
        "connection_ref": {
            "realm": "dev",
            "binding": "default_openai"
        },
        "profile_id": "p1",
        "provider": "openai",
        "auth_method": "managed_chatgpt_oauth",
        "state": "credential_present",
        "has_refresh_token": false
    });
    assert!(
        serde_json::from_value::<WireAuthStatusDetail>(detail).is_err(),
        "WireAuthStatusDetail state must reject string defaults outside AuthStatusPhase"
    );
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
        assert!(props.get("realm").is_some());
        assert!(props.get("binding").is_some());
    }

    #[test]
    fn auth_profile_schema_has_discriminator_fields() {
        let s = schema_json::<WireAuthProfile>();
        let props = s.pointer("/properties").expect("schema has properties");
        assert!(props.get("source_kind").is_some());
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

    #[test]
    fn auth_status_detail_schema_has_binding_identity_fields() {
        let s = schema_json::<WireAuthStatusDetail>();
        let props = s.pointer("/properties").expect("schema has properties");
        assert!(props.get("realm_id").is_some());
        assert!(props.get("binding_id").is_some());
        assert!(props.get("connection_ref").is_some());
        assert!(props.get("profile_id").is_some());
        assert!(props.get("has_refresh_token").is_some());
    }
}
