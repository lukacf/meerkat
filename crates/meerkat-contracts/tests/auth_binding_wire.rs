//! Phase 4c T12 contract proof — wire-type round-trips.
//!
//! Plan §Top-down integration tests T12 (crates/meerkat-contracts/tests/
//! auth_binding_wire.rs) asserts that every wire projection the SDK
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
    WireAuthBindingRef, WireAuthError, WireAuthProfile, WireAuthStatus, WireAuthStatusDetail,
    WireBackendProfile, WireBindingIdentity, WireProviderBinding, WireRealmConnectionSet,
};

fn sample_auth_binding() -> meerkat_core::AuthBindingRef {
    meerkat_core::AuthBindingRef {
        realm: meerkat_core::connection::RealmId::parse("dev").expect("valid realm"),
        binding: meerkat_core::connection::BindingId::parse("default_openai")
            .expect("valid binding"),
        profile: None,
        origin: meerkat_core::connection::BindingOrigin::Configured,
    }
}

fn sample_backend_profile() -> meerkat_core::BackendProfile {
    meerkat_core::BackendProfile {
        id: "openai_api".to_string(),
        provider: meerkat_core::Provider::OpenAI,
        backend_kind: "openai_api".to_string(),
        base_url: Some("https://api.openai.com".to_string()),
        options: serde_json::json!({"region": "us-east-1"}),
        server: None,
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
        credential_account: None,
        default_model: Some("gpt-5.2".to_string()),
        policy: Default::default(),
        provider_default: false,
    }
}

#[test]
fn auth_binding_serde_roundtrip() {
    let domain = sample_auth_binding();
    let wire: WireAuthBindingRef = domain.clone().into();
    let s = serde_json::to_string(&wire).unwrap();
    assert!(s.contains("\"realm\":\"dev\""));
    assert!(s.contains("\"binding\":\"default_openai\""));
    let back: WireAuthBindingRef = serde_json::from_str(&s).unwrap();
    let redomain: meerkat_core::AuthBindingRef = back.into();
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
        realm_id: meerkat_core::connection::RealmId::parse("dev").expect("valid realm id"),
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
    let auth_binding = sample_auth_binding();
    let status = WireAuthStatusDetail {
        identity: WireBindingIdentity::from(&auth_binding),
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
    assert_eq!(value["auth_binding"]["realm"], "dev");
    assert_eq!(value["profile_id"], "prod_env_key");
    assert_eq!(value["has_refresh_token"], true);
    let back: WireAuthStatusDetail = serde_json::from_value(value).unwrap();
    assert_eq!(back.identity.realm_id, status.identity.realm_id);
    assert_eq!(back.identity.binding_id, status.identity.binding_id);
    assert_eq!(back.identity.auth_binding, status.identity.auth_binding);
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
        "auth_binding": {
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
    fn auth_binding_schema_has_realm_and_binding() {
        let s = schema_json::<WireAuthBindingRef>();
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
        assert!(props.get("auth_binding").is_some());
        assert!(props.get("profile_id").is_some());
        assert!(props.get("has_refresh_token").is_some());
    }
}

// --- MCP server target on auth/login/* and auth/status/get -------------

mod mcp_login_target {
    use meerkat_contracts::{
        AuthStatusParams, LoginCompleteParams, LoginStartParams, WireLoginCompleteTarget,
        WireLoginReady, WireLoginReadyTarget, WireLoginStart, WireLoginStartTarget,
        WireLoginTarget, WireMcpAuthTarget, WireMcpLoginReady, WireMcpLoginStart,
        WireOAuthProvider,
    };
    use serde_json::json;

    #[test]
    fn provider_login_json_keeps_its_flat_shape() {
        let legacy = json!({
            "provider": "openai",
            "redirect_uri": "http://127.0.0.1:1/callback",
            "realm_id": "dev",
            "binding_id": "default_openai",
        });
        let parsed: LoginStartParams = serde_json::from_value(legacy.clone()).unwrap();
        let WireLoginTarget::Provider(target) = &parsed.target else {
            panic!("legacy JSON must stay a provider target");
        };
        assert_eq!(target.provider, WireOAuthProvider::OpenAi);
        assert_eq!(serde_json::to_value(&parsed).unwrap(), legacy);

        let complete: LoginCompleteParams = serde_json::from_value(json!({
            "provider": "anthropic",
            "code": "c",
            "state": "s",
            "redirect_uri": "http://127.0.0.1:1/callback",
            "realm_id": "dev",
            "binding_id": "b",
            "profile_id": "p",
        }))
        .unwrap();
        assert!(matches!(
            complete.target,
            WireLoginCompleteTarget::Provider(_)
        ));
    }

    #[test]
    fn mcp_login_target_parses_and_mixed_targets_are_refused() {
        let start: LoginStartParams = serde_json::from_value(json!({
            "mcp": {
                "server_name": "glean",
                "server_url": "https://glean.example/mcp",
                "oauth_account": "subject-7",
            },
            "redirect_uri": "http://127.0.0.1:1/mcp/oauth/callback",
        }))
        .unwrap();
        let WireLoginTarget::Mcp(target) = &start.target else {
            panic!("expected an MCP target");
        };
        assert_eq!(target.mcp.oauth_account.as_deref(), Some("subject-7"));

        for mixed in [
            json!({
                "provider": "openai",
                "realm_id": "dev",
                "binding_id": "default_openai",
                "mcp": {"server_name": "glean", "server_url": "https://glean.example/mcp"},
                "redirect_uri": "http://127.0.0.1:1/callback",
            }),
            json!({
                "mcp": {"server_name": "glean", "server_url": "https://glean.example/mcp"},
                "realm_id": "dev",
                "redirect_uri": "http://127.0.0.1:1/callback",
            }),
            json!({
                "mcp": {"server_name": "glean", "server_url": "u", "authorize_url": "x"},
                "redirect_uri": "http://127.0.0.1:1/callback",
            }),
        ] {
            assert!(
                serde_json::from_value::<LoginStartParams>(mixed.clone()).is_err(),
                "ambiguous target must be refused: {mixed}"
            );
        }

        let complete: LoginCompleteParams = serde_json::from_value(json!({
            "mcp": {"server_name": "glean", "server_url": "https://glean.example/mcp"},
            "client_id": "client-123",
            "resource_metadata_url": "https://glean.example/.well-known/oauth-protected-resource/mcp",
            "code": "fixture-code",
            "state": "fixture-state",
            "redirect_uri": "http://127.0.0.1:1/mcp/oauth/callback",
        }))
        .unwrap();
        assert!(matches!(complete.target, WireLoginCompleteTarget::Mcp(_)));

        let status: AuthStatusParams = serde_json::from_value(json!({
            "mcp": {"server_name": "glean", "server_url": "https://glean.example/mcp"},
        }))
        .unwrap();
        assert!(matches!(status, AuthStatusParams::Mcp(_)));
        let binding: AuthStatusParams =
            serde_json::from_value(json!({"realm_id": "dev", "binding_id": "b"})).unwrap();
        assert!(matches!(binding, AuthStatusParams::Binding(_)));
    }

    #[test]
    fn login_debug_redacts_bearer_material() {
        let complete: LoginCompleteParams = serde_json::from_value(json!({
            "mcp": {"server_name": "glean", "server_url": "https://glean.example/mcp"},
            "client_id": "client-123",
            "code": "code-canary",
            "state": "state-canary",
            "redirect_uri": "http://127.0.0.1:1/mcp/oauth/callback",
        }))
        .unwrap();
        let start = WireLoginStart {
            authorize_url: "https://idp.example/authorize?state=state-canary".into(),
            state: "state-canary".into(),
            redirect_uri: "http://127.0.0.1:1/mcp/oauth/callback".into(),
            target: WireLoginStartTarget::Mcp(WireMcpLoginStart {
                mcp: WireMcpAuthTarget {
                    server_name: "glean".into(),
                    server_url: "https://glean.example/mcp".into(),
                    oauth_account: None,
                },
                client_id: "client-123".into(),
                resource_metadata_url: "https://glean.example/.well-known/x".into(),
            }),
        };
        let rendered = format!("{complete:?} {start:?}");
        for canary in ["code-canary", "state-canary", "idp.example/authorize"] {
            assert!(!rendered.contains(canary), "{canary} leaked: {rendered}");
        }
    }

    #[test]
    fn mcp_login_ready_is_secret_free_and_distinct_from_provider_ready() {
        let ready = WireLoginReady {
            state: None,
            target: WireLoginReadyTarget::Mcp(WireMcpLoginReady {
                mcp: WireMcpAuthTarget {
                    server_name: "glean".into(),
                    server_url: "https://glean.example/mcp".into(),
                    oauth_account: Some("subject-7".into()),
                },
                account_id: Some("subject-7".into()),
            }),
            expires_at: None,
            has_refresh_token: true,
            scopes: vec!["openid".into()],
        };
        let value = serde_json::to_value(&ready).unwrap();
        assert_eq!(value["mcp"]["server_name"], "glean");
        assert_eq!(value["account_id"], "subject-7");
        assert!(value.get("provider").is_none());
        let back: WireLoginReady = serde_json::from_value(value).unwrap();
        assert!(matches!(back.target, WireLoginReadyTarget::Mcp(_)));
    }
}
