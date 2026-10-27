#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use meerkat_meta::{
    AuthPolicy, AuthProvider, BigQueryNaming, ConsoleAccessRequest, ConsolePolicy,
    ConsoleRestJsonRequest, DecisionPolicyError, JwtValidationConfig, JwtValidationError,
    RuntimeDecisionInputs, RuntimeOpsPolicy, TrustedOidcRuntimeConfig,
    build_runtime_decision_state, enforce_console_route_access, handle_console_ingress_json,
    handle_console_rest_json_route, parse_jwks_json, parse_oidc_discovery_json,
    select_jwk_for_token, validate_jwt_locally,
};
use serde_json::{Value, json};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

fn trusted_toml() -> String {
    r#"
[[modules]]
id = "router"
command = "router-bin"
args = ["--mode", "fast"]
restart_policy = "always"

[[modules]]
id = "delivery"
command = "delivery-bin"
args = ["--sink", "test"]
restart_policy = "on_failure"
"#
    .to_string()
}

fn release_json() -> String {
    include_str!("../assets/release-targets.json").to_string()
}

fn decision_state() -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "phase7_dataset".to_string(),
            table: "phase7_table".to_string(),
        },
        trusted_meta_toml: trusted_toml(),
        auth: AuthPolicy {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: vec![
                "alice@example.com".to_string(),
                "svc:delivery-bot".to_string(),
            ],
        },
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth: true,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: release_json(),
    })
    .expect("decision state should build")
}

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.localhost","jwks_uri":"https://trusted.meta.localhost/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"},{"kid":"kid-next","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtbmV4dC1zZWNyZXQ"}]}"#.to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}

fn sign_hs256(payload: Value, secret: &str, kid: &str) -> String {
    let header = json!({"alg":"HS256","typ":"JWT","kid":kid});
    let header_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("encode header"));
    let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("encode claims"));
    let signing_input = format!("{header_b64}.{payload_b64}");
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac init");
    mac.update(signing_input.as_bytes());
    let signature = mac.finalize().into_bytes();
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature);
    format!("{signing_input}.{signature_b64}")
}

fn b64_json(value: Value) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).expect("serialize json"))
}

#[test]
fn phase7_auth_001_provider_support_model_and_dec_002_default_behavior() {
    let auth = AuthPolicy {
        default_provider: AuthProvider::GoogleOAuth,
        email_allowlist: vec!["alice@example.com".to_string()],
    };
    let console = ConsolePolicy {
        require_app_auth: true,
        ..ConsolePolicy::default()
    };

    let google_ok = enforce_console_route_access(
        &auth,
        &console,
        &ConsoleAccessRequest {
            provider: AuthProvider::GoogleOAuth,
            email: "alice@example.com".to_string(),
        },
    );
    let github_mismatch = enforce_console_route_access(
        &auth,
        &console,
        &ConsoleAccessRequest {
            provider: AuthProvider::GitHubOAuth,
            email: "alice@example.com".to_string(),
        },
    );
    let oidc_mismatch = enforce_console_route_access(
        &auth,
        &console,
        &ConsoleAccessRequest {
            provider: AuthProvider::GenericOidc,
            email: "alice@example.com".to_string(),
        },
    );
    let mismatch_with_test_provider = enforce_console_route_access(
        &auth,
        &console,
        &ConsoleAccessRequest {
            provider: AuthProvider::TestProvider,
            email: "alice@example.com".to_string(),
        },
    );

    assert_eq!(google_ok, Ok(()));
    assert_eq!(
        github_mismatch,
        Err(DecisionPolicyError::AuthProviderMismatch)
    );
    assert_eq!(
        oidc_mismatch,
        Err(DecisionPolicyError::AuthProviderMismatch)
    );
    assert_eq!(
        mismatch_with_test_provider,
        Err(DecisionPolicyError::AuthProviderMismatch)
    );
}

#[test]
fn phase7_auth_002_allowlist_and_service_identity_path() {
    let auth = AuthPolicy {
        default_provider: AuthProvider::GitHubOAuth,
        email_allowlist: vec![
            "alice@example.com".to_string(),
            "svc:delivery-bot".to_string(),
        ],
    };
    let console = ConsolePolicy {
        require_app_auth: true,
        ..ConsolePolicy::default()
    };

    let user_ok = enforce_console_route_access(
        &auth,
        &console,
        &ConsoleAccessRequest {
            provider: AuthProvider::GitHubOAuth,
            email: "alice@example.com".to_string(),
        },
    );
    let user_denied = enforce_console_route_access(
        &auth,
        &console,
        &ConsoleAccessRequest {
            provider: AuthProvider::GitHubOAuth,
            email: "mallory@example.com".to_string(),
        },
    );
    let service_ok = enforce_console_route_access(
        &auth,
        &console,
        &ConsoleAccessRequest {
            provider: AuthProvider::ServiceIdentity,
            email: "svc:delivery-bot".to_string(),
        },
    );
    let service_bad_format = enforce_console_route_access(
        &auth,
        &console,
        &ConsoleAccessRequest {
            provider: AuthProvider::ServiceIdentity,
            email: "delivery-bot".to_string(),
        },
    );
    let service_denied = enforce_console_route_access(
        &auth,
        &console,
        &ConsoleAccessRequest {
            provider: AuthProvider::ServiceIdentity,
            email: "svc:unknown".to_string(),
        },
    );

    assert_eq!(user_ok, Ok(()));
    assert_eq!(user_denied, Err(DecisionPolicyError::EmailNotAllowlisted));
    assert_eq!(service_ok, Ok(()));
    assert_eq!(
        service_bad_format,
        Err(DecisionPolicyError::InvalidServiceIdentity)
    );
    assert_eq!(
        service_denied,
        Err(DecisionPolicyError::ServiceIdentityNotAllowlisted)
    );
}

#[test]
fn phase7_auth_002_generic_oidc_allow_path_when_configured_default() {
    let auth = AuthPolicy {
        default_provider: AuthProvider::GenericOidc,
        email_allowlist: vec!["alice@example.com".to_string()],
    };
    let console = ConsolePolicy {
        require_app_auth: true,
        ..ConsolePolicy::default()
    };
    let allow = enforce_console_route_access(
        &auth,
        &console,
        &ConsoleAccessRequest {
            provider: AuthProvider::GenericOidc,
            email: "alice@example.com".to_string(),
        },
    );
    let mismatch = enforce_console_route_access(
        &auth,
        &console,
        &ConsoleAccessRequest {
            provider: AuthProvider::GoogleOAuth,
            email: "alice@example.com".to_string(),
        },
    );
    assert_eq!(allow, Ok(()));
    assert_eq!(mismatch, Err(DecisionPolicyError::AuthProviderMismatch));
}

#[test]
fn phase7_auth_003_local_jwt_validation_without_ipc() {
    let secret = "phase7-secret";
    let token = sign_hs256(
        json!({
            "sub":"user-123",
            "email":"alice@example.com",
            "iss":"https://issuer.example",
            "aud":"meerkat-console",
            "provider":"google_oauth",
            "exp":2_000_000_000_u64,
            "nbf":1_700_000_000_u64
        }),
        secret,
        "kid-a",
    );
    let config = JwtValidationConfig {
        shared_secret: secret.to_string(),
        issuer: Some("https://issuer.example".to_string()),
        audience: Some("meerkat-console".to_string()),
        now_epoch_seconds: 1_800_000_000,
        leeway_seconds: 30,
    };

    let validated = validate_jwt_locally(&token, &config).expect("token should validate");
    assert_eq!(validated.subject.as_deref(), Some("user-123"));
    assert_eq!(validated.email.as_deref(), Some("alice@example.com"));
    assert_eq!(validated.provider.as_deref(), Some("google_oauth"));

    let wrong_secret = JwtValidationConfig {
        shared_secret: "wrong".to_string(),
        ..config.clone()
    };
    assert_eq!(
        validate_jwt_locally(&token, &wrong_secret),
        Err(JwtValidationError::InvalidSignature)
    );
}

#[test]
fn phase7_oidc_discovery_and_jwks_contract_parse_and_selection() {
    let discovery = parse_oidc_discovery_json(
        r#"{"issuer":"https://issuer.example","jwks_uri":"https://issuer.example/keys"}"#,
    )
    .expect("discovery should parse");
    let jwks = parse_jwks_json(
        r#"{"keys":[
            {"kid":"k1","kty":"oct","alg":"HS256","k":"czNjcjN0"},
            {"kid":"k2","kty":"oct","alg":"HS256","k":"b3RoZXI"}
        ]}"#,
    )
    .expect("jwks should parse");
    let selected = select_jwk_for_token(&jwks, Some("k2"), "HS256").expect("kid should resolve");

    assert_eq!(discovery.issuer, "https://issuer.example");
    assert_eq!(selected.kid.as_deref(), Some("k2"));
}

#[test]
fn phase7_runtime_token_auth_middleware_flow() {
    let state = decision_state();

    let service_token = sign_hs256(
        json!({
            "sub":"svc:delivery-bot",
            "actor_type":"service",
            "provider":"generic_oidc",
            "iss":"https://trusted.meta.localhost",
            "aud":"meerkat-console",
            "exp":4_000_000_000_u64
        }),
        "phase7-trusted-current-secret",
        "kid-current",
    );
    let service_allowed = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: format!("/console/modules?auth_token={service_token}"),
            auth: None,
        },
    );

    let bad_token = sign_hs256(
        json!({
            "sub":"svc:delivery-bot",
            "actor_type":"service",
            "provider":"generic_oidc",
            "iss":"https://trusted.meta.localhost",
            "aud":"wrong-aud",
            "exp":4_000_000_000_u64
        }),
        "phase7-trusted-current-secret",
        "kid-current",
    );
    let service_denied = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: format!("/console/modules?auth_token={bad_token}"),
            auth: None,
        },
    );

    assert_eq!(service_allowed.status, 200);
    assert_eq!(
        service_denied.body,
        json!({"error":"unauthorized","reason":"invalid_token"})
    );
}

#[test]
fn phase7_ic_001_caller_supplied_jwks_is_ignored() {
    let state = decision_state();
    let attacker_token = sign_hs256(
        json!({
            "sub":"alice@example.com",
            "email":"alice@example.com",
            "provider":"google_oauth",
            "iss":"https://attacker.example",
            "aud":"attacker-console",
            "exp":4_000_000_000_u64
        }),
        "attacker-secret",
        "attacker-kid",
    );
    let injected_discovery = b64_json(
        json!({"issuer":"https://attacker.example","jwks_uri":"https://attacker.example/jwks"}),
    );
    let injected_jwks = b64_json(
        json!({"keys":[{"kid":"attacker-kid","kty":"oct","alg":"HS256","k":URL_SAFE_NO_PAD.encode("attacker-secret".as_bytes())}]}),
    );
    let denied = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: format!(
                "/console/modules?auth_token={attacker_token}&oidc_discovery_b64={injected_discovery}&jwks_b64={injected_jwks}&audience=attacker-console"
            ),
            auth: None,
        },
    );

    assert_eq!(
        denied.body,
        json!({"error":"unauthorized","reason":"jwks_key_not_found"})
    );
}

#[test]
fn phase7_sc_001_key_rotation_new_key_passes_stale_key_fails() {
    let state = decision_state();
    let next_key_token = sign_hs256(
        json!({
            "sub":"user-rot",
            "email":"alice@example.com",
            "provider":"google_oauth",
            "iss":"https://trusted.meta.localhost",
            "aud":"meerkat-console",
            "exp":4_000_000_000_u64
        }),
        "phase7-trusted-next-secret",
        "kid-next",
    );
    let stale_key_token = sign_hs256(
        json!({
            "sub":"user-rot",
            "email":"alice@example.com",
            "provider":"google_oauth",
            "iss":"https://trusted.meta.localhost",
            "aud":"meerkat-console",
            "exp":4_000_000_000_u64
        }),
        "phase7-trusted-old-secret",
        "kid-old",
    );

    let rotated_ok = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: format!("/console/modules?auth_token={next_key_token}"),
            auth: None,
        },
    );
    let stale_denied = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: format!("/console/modules?auth_token={stale_key_token}"),
            auth: None,
        },
    );

    assert_eq!(rotated_ok.status, 200);
    assert_eq!(
        stale_denied.body,
        json!({"error":"unauthorized","reason":"jwks_key_not_found"})
    );
}

#[test]
fn phase7_config_flow_trusted_oidc_values_control_auth_outcome() {
    let mut strict = decision_state();
    strict.trusted_oidc = TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://strict.issuer.localhost","jwks_uri":"https://strict.issuer.localhost/jwks"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"strict-kid","kty":"oct","alg":"HS256","k":"c3RyaWN0LXNlY3JldA"}]}"#.to_string(),
        audience: "strict-aud".to_string(),
        require_verified_email: false,
    };
    let strict_token = sign_hs256(
        json!({
            "sub":"u1",
            "email":"alice@example.com",
            "provider":"google_oauth",
            "iss":"https://strict.issuer.localhost",
            "aud":"strict-aud",
            "exp":4_000_000_000_u64
        }),
        "strict-secret",
        "strict-kid",
    );
    let strict_ok = handle_console_rest_json_route(
        &strict,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: format!("/console/modules?auth_token={strict_token}"),
            auth: None,
        },
    );

    let default_state = decision_state();
    let denied_on_default = handle_console_rest_json_route(
        &default_state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: format!("/console/modules?auth_token={strict_token}"),
            auth: None,
        },
    );

    assert_eq!(strict_ok.status, 200);
    assert_eq!(
        denied_on_default.body,
        json!({"error":"unauthorized","reason":"jwks_key_not_found"})
    );
}

#[test]
fn phase7_non_test_ingress_caller_routes_console_auth() {
    let state = decision_state();
    let token = sign_hs256(
        json!({
            "sub":"ingress-user",
            "email":"alice@example.com",
            "provider":"google_oauth",
            "iss":"https://trusted.meta.localhost",
            "aud":"meerkat-console",
            "exp":4_000_000_000_u64
        }),
        "phase7-trusted-current-secret",
        "kid-current",
    );
    let ingress_request = json!({
        "method":"GET",
        "path": format!("/console/modules?auth_token={token}"),
        "auth": null
    });
    let ingress_response_json = handle_console_ingress_json(&state, &ingress_request.to_string());
    let ingress_response: serde_json::Value =
        serde_json::from_str(&ingress_response_json).expect("valid ingress response");

    assert_eq!(ingress_response["status"], 200);
    assert!(ingress_response["body"]["modules"].is_array());
}
