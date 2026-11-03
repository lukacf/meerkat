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
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use meerkat_meta::{
    AuthPolicy, AuthProvider, BigQueryNaming, ConsolePolicy, ConsoleRestJsonRequest,
    RuntimeDecisionInputs, RuntimeOpsPolicy, TrustedOidcRuntimeConfig,
    build_runtime_decision_state, handle_console_rest_json_route,
};
use serde_json::{Value, json};

const RSA_A_PRIVATE_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDOsyz7rroGyhPq
7T2/q4WF4CLX6wzJeXVNBOYnrdqBYRhVt04wxSDQbQThdKIuUB3KoMmdscaI+qnL
z+pJaTqif0+wHmz21CB6iTYlRIzhEi5TfOxLT9fl8X/lZ4qvA1YeQH6tXqi9rCS1
R5tLMhA/ZUYGr8xcgRuVYzb58slWUUCcw0S3W5h54jEgKU653MeQz47lj3R9l+bk
VqgoO44MXkCQe6S3kwXksplF4Hnnw2n2wu58VxcsxM1hJ42ufLUN1gSCkrUuTr2d
giUc07ezm48vIetY8ZilYyjXHg8PJlVPo63nLV2+N2chZ3lw9Y3PjW7+b1d/Efcq
UwiIEa3xAgMBAAECggEAA7i+Kpnux4iD60ryPa087jgm6HRW+pmxPv3DlxtOP94b
rg9q3P3vpVERMW4ELYlLwAY9GxXEWVsKC51mvoOihqJ8MNObaqZPH2WxD+K4FqVZ
KC+roX+Ch6VdhCflG1mYB1tp7H0z1JZw4sKzTRtNp5aPODeaGmBIutvadY2limRL
0di4hEaxJcXwb75dy//goum3JlXEM7kdEslQ8ZMyrHTH+55b3TTqXhYLWututDJO
9/2h2rETiRO+Wy2zCo0OKNR/QRCYphzXqFGDoNmgH7UPQT/AuRtKcolL9q/7Hg9r
HHrhdMZWmXGjUtNU43gPrsgg7lGtRicQ1SQi8yHW8QKBgQD5lvBfnTUcme/yzXNU
MxERm6YNGi6ZuyM6MrLfuu2BeZoOALZoZOwoCTFzOm6pwirA9lkwAGYvJ7NDnbYV
7kmu7rf7lC9wo0d6eJli1ldCbdvXIQZxe60LMFvqrB+mUWU6ubQBqXNLciKSe1GH
xgvbbR1eUitddBBcU43roUCsXwKBgQDUAjxA30/USgP83425y9sxcnuADEOqtSdE
lHvsP0frrAz/Rl4Uf5utejjPDk+d5SjlP6HAJkLdI5kJkDsuzTIAgXhEeGZd/UEF
R13sSNFKdha5SjNYs5xL+dtPhv4EgHVZ9PYx9OBj6tYuzI9/YPGw5mI+IpaBdM3B
2EuaBeLHrwKBgEzC+1KsyvTs8zs9rMasngdIU52b+9EUGRWBGjptBzbW62Z7GZ6p
y2fUy/ygcACN0xBds7hrpwHBuASHsMS18Lt4d+VMAfsmfIlSJfqb6WJo30AezBiC
7QmP6fUW0vUX+4ZALviD4Q3HIJLkkoKrimIGAQ5NP0ESvSVoHTHm+jkJAoGBALEW
ZOnzHiU+5fHVcfad9ytoawxcMjFnO7OnK5P8j8ClZ/3a8z7AEHNpQgaB97L19aD3
884ip3s7/trkJOtE7t1JSAI5Z5hesG8OW7/AW0GNPhHrjtQqwwUbYTsekROFkYBg
gzzbRItxXxKcP8iwW3HeHnW0Qm9D95JRb2TqQbF1AoGBAOc0xLIYp+g8ewSt9qcB
W3DO9TAs2etiMaAcH/sgl0YGpq7b5lHn3AZAglfP0bMvEVLVK+AWuxkTUVAxn9Re
/zPZlurmdwU2Yn1ZrL4C0Q1Hh1FCRxWEEPM0uuYZDKxkbR10NfP84aJtLt8r8VSd
wM/t64cVs2yoe53gOBgSqxWJ
-----END PRIVATE KEY-----
"#;

const RSA_B_PRIVATE_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDQHPbWBOSzob4B
QXNNM2dbAGl5WlL1VWLrrUmH+rlWiTHXCMmIy0Y9IHsxpl3Ldx2Gvn8sWk1iUJ0F
u9euvtkoXuXW1Zu653bP8+ZBqOib+4T+Ey4LydDNSboxb/9/pzkZuTtD6wRwVHJx
oDpuYKTOdpTLMThBWk1G7m3ZjG5EtLYvObRpPoDBbe65DRLNGdKZxJgZSEMB4413
25zJ60PcRtrEJcJjuCR/e63MfsDi4+IgbOFIuuvXXRBEjWQvZ5HAIO4h5Hbh67BP
1DQiwflZNe3F18CS4SucH12rgixBF5X2GKNwRgRqPp3GfPx0YHmGeSsAGOE1hOU0
3lbW9HAbAgMBAAECggEAAsBnSRQee/uG+hhF8H7d/neGbXrSvvimiqwrXTdk7O56
cLfmhj79ykAcMN9cvRxxkP8CynDVNhgPw2wk4WQXle+PRWRknzeBPCWi5TpY/Pr5
2qwhPzmnX5d6dT3gWG07FYp77J12XQ/YxYTTUPNJKoup0vfvIPoTLH1piWdQa+sb
n4EBzUQLJ1vScdwDC4EO4/SP82HDY1RN5ktReoacCUtw+YDZdOY2F4hAPflmWKEd
a2UyNry+TGDFeEX4WQeENhx3BzkNLEo0r5ok65WB4D54rdXtJ9CY9vSWPMZjElPf
/V/qVfcKRwEJyYLgFHeuckNOf7yA6kksrwLZb3/TNQKBgQD7CIID929HxCadjg7A
RNYmXzSTVbAF++ttqZVvPui7PquEVrLdCIxuslMKxYDcDNRxNY4UQ3pUe60EQ9ZR
hgZD9uLAqrLOMT6Z+dFPv1T6VDab1D2DXIXdDC3NLaA7H+Ivruu83vANkGzW/oZn
zUG3tZ27Uvecj1rsUgYTpu5rxwKBgQDUOxCG2ryiMgWjsLGJLyiF9xFzIL5FRLGY
BXUSjmGlvrPtahpsTKT90TGRFipDXmsDdBGVpRlJeTYTFIcHI7ujR1gioGJwAyPc
okkNiPfdj3TtsspAPkJCbvyWJVxyngxjGiYLFw9IO16dszv7g0tk6nxf24N1KeeB
xLMzR0lRDQKBgQCtodrcB39O8luLSsDlODevXtasue4AlZjnxw53Xdn3+YcFCDq7
K7iGsI1DvAw/KBihHVvipDGu0cSAWLOau8sFo3R/sxHuEJ2uPt8J+9s5Mpp6+jh5
7bshg9UCP/a+LnVyadjgUItVtnmx02b/0TcNbG9nLCHchkNrhehyG1p57QKBgQCk
BzJyx8RbJ4YsSXgtqwEK6TXXYUsthjYsZKtjOCBIVegCaqsZYPN0KKbCl/r6LpNP
C/o7SmsM2l3syUTDQ97WB2IbARKTuBmTgOotR9sqpqGcxT6EAJp9dgJKmX2mKHky
bxdQIvZwwVITWF/XuFYhHQobnDEx8L05EqndzA7iUQKBgHZY96QFRhkqDXlPZLCm
g5IDWgpKMzuY210z0zV031PhCg+OOv2qRrHAlwpiR3kygAhIuuKCD18K5kDu/bhb
TwyxevcS2PuB+6qvjcEuflcVTF8A4EhTdusyUHQa9lLKADqeOylhRK2PsHvpFelV
kZxouHmozn2ZLK6iVesu5xs6
-----END PRIVATE KEY-----
"#;

const EC_A_PRIVATE_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg5mB0FxcGYqKI+UOt
epRCRycET6hPWU6dF6U42AoZWgqhRANCAAR3QYsGfw/kJje2YZd9tO6RGlEfpEiu
jC+6hONZGcfM66hMEgN9QMh5P4Jv/UpDvLy2PhLsJC2fMP5oLdOCwdik
-----END PRIVATE KEY-----
"#;

const RSA_A_N: &str = "zrMs-666BsoT6u09v6uFheAi1-sMyXl1TQTmJ63agWEYVbdOMMUg0G0E4XSiLlAdyqDJnbHGiPqpy8_qSWk6on9PsB5s9tQgeok2JUSM4RIuU3zsS0_X5fF_5WeKrwNWHkB-rV6ovawktUebSzIQP2VGBq_MXIEblWM2-fLJVlFAnMNEt1uYeeIxIClOudzHkM-O5Y90fZfm5FaoKDuODF5AkHukt5MF5LKZReB558Np9sLufFcXLMTNYSeNrny1DdYEgpK1Lk69nYIlHNO3s5uPLyHrWPGYpWMo1x4PDyZVT6Ot5y1dvjdnIWd5cPWNz41u_m9XfxH3KlMIiBGt8Q";
const RSA_B_N: &str = "0Bz21gTks6G-AUFzTTNnWwBpeVpS9VVi661Jh_q5Vokx1wjJiMtGPSB7MaZdy3cdhr5_LFpNYlCdBbvXrr7ZKF7l1tWbuud2z_PmQajom_uE_hMuC8nQzUm6MW__f6c5Gbk7Q-sEcFRycaA6bmCkznaUyzE4QVpNRu5t2YxuRLS2Lzm0aT6AwW3uuQ0SzRnSmcSYGUhDAeONd9ucyetD3EbaxCXCY7gkf3utzH7A4uPiIGzhSLrr110QRI1kL2eRwCDuIeR24euwT9Q0IsH5WTXtxdfAkuErnB9dq4IsQReV9hijcEYEaj6dxnz8dGB5hnkrABjhNYTlNN5W1vRwGw";
const RSA_E: &str = "AQAB";
const EC_A_X: &str = "d0GLBn8P5CY3tmGXfbTukRpRH6RIrowvuoTjWRnHzOs";
const EC_A_Y: &str = "qEwSA31AyHk_gm_9SkO8vLY-EuwkLZ8w_mgt04LB2KQ";

const DEV_HS_SECRET: &str = "phase-e-dev-shared-secret";
const DEV_ISSUER: &str = "http://localhost:7443";
const DEV_JWKS_URI: &str = "http://localhost:7443/.well-known/jwks.json";
const LOCAL_SUFFIX_ISSUER: &str = "https://oidc.prod.local";
const LOCAL_SUFFIX_JWKS_URI: &str = "https://oidc.prod.local/.well-known/jwks.json";
const PROD_ISSUER: &str = "https://oidc.prod.example";
const PROD_JWKS_URI: &str = "https://oidc.prod.example/.well-known/jwks.json";
const AUDIENCE: &str = "meerkat-console";

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

fn build_state(
    issuer: &str,
    jwks_uri: &str,
    jwks_json: Value,
) -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "phase_e_dataset".to_string(),
            table: "phase_e_table".to_string(),
        },
        trusted_meta_toml: trusted_toml(),
        auth: AuthPolicy {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: vec![
                "alice@example.com".to_string(),
                "svc:deploy-bot".to_string(),
            ],
        },
        trusted_oidc: TrustedOidcRuntimeConfig {
            discovery_json: json!({"issuer":issuer,"jwks_uri":jwks_uri}).to_string(),
            jwks_json: jwks_json.to_string(),
            audience: AUDIENCE.to_string(),
            require_verified_email: false,
        },
        console: ConsolePolicy {
            require_app_auth: true,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: release_json(),
    })
    .expect("decision state should build")
}

fn route_with_token(
    state: &meerkat_meta::RuntimeDecisionState,
    token: &str,
) -> meerkat_meta::ConsoleRestJsonResponse {
    handle_console_rest_json_route(
        state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: format!("/console/modules?auth_token={token}"),
            auth: None,
        },
    )
}

fn default_claims(issuer: &str, audience: &str) -> Value {
    json!({
        "sub":"user-123",
        "email":"alice@example.com",
        "provider":"google_oauth",
        "iss":issuer,
        "aud":audience,
        "exp":4_000_000_000_u64,
        "nbf":1_700_000_000_u64
    })
}

fn sign_rs256(kid: &str, claims: &Value, pem: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_string());
    encode(
        &header,
        claims,
        &EncodingKey::from_rsa_pem(pem.as_bytes()).expect("rsa encoding key"),
    )
    .expect("rs256 token")
}

fn sign_es256(kid: &str, claims: &Value, pem: &str) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_string());
    encode(
        &header,
        claims,
        &EncodingKey::from_ec_pem(pem.as_bytes()).expect("ec encoding key"),
    )
    .expect("es256 token")
}

fn sign_hs256(kid: &str, claims: &Value, secret: &str) -> String {
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some(kid.to_string());
    encode(
        &header,
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("hs256 token")
}

fn tamper_signature(token: &str) -> String {
    let mut parts = token
        .split('.')
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert_eq!(parts.len(), 3, "jwt should have three parts");
    let replacement = if parts[2].starts_with('a') { "b" } else { "a" };
    parts[2].replace_range(0..1, replacement);
    format!("{}.{}.{}", parts[0], parts[1], parts[2])
}

fn rsa_jwk(kid: &str, alg: &str, n: &str) -> Value {
    json!({"kid":kid,"kty":"RSA","alg":alg,"n":n,"e":RSA_E})
}

fn ec_jwk(kid: &str) -> Value {
    json!({"kid":kid,"kty":"EC","alg":"ES256","crv":"P-256","x":EC_A_X,"y":EC_A_Y})
}

fn hs_jwk(kid: &str, secret: &str) -> Value {
    json!({
        "kid":kid,
        "kty":"oct",
        "alg":"HS256",
        "k":URL_SAFE_NO_PAD.encode(secret.as_bytes())
    })
}

#[test]
fn phase_e_req_e_001_rs256_token_accepted_with_matching_jwks_key() {
    let state = build_state(
        PROD_ISSUER,
        PROD_JWKS_URI,
        json!({"keys":[rsa_jwk("rsa-current", "RS256", RSA_A_N)]}),
    );
    let token = sign_rs256(
        "rsa-current",
        &default_claims(PROD_ISSUER, AUDIENCE),
        RSA_A_PRIVATE_PEM,
    );

    let response = route_with_token(&state, &token);
    assert_eq!(response.status, 200);
}

#[test]
fn phase_e_req_e_001_es256_token_accepted_with_matching_jwks_key() {
    let state = build_state(
        PROD_ISSUER,
        PROD_JWKS_URI,
        json!({"keys":[ec_jwk("ec-current")]}),
    );
    let token = sign_es256(
        "ec-current",
        &default_claims(PROD_ISSUER, AUDIENCE),
        EC_A_PRIVATE_PEM,
    );

    let response = route_with_token(&state, &token);
    assert_eq!(response.status, 200);
}

#[test]
fn phase_e_req_e_001_jwks_rotation_accepts_new_kid_and_rejects_stale_kid() {
    let pre_rotation = build_state(
        PROD_ISSUER,
        PROD_JWKS_URI,
        json!({"keys":[rsa_jwk("rsa-old", "RS256", RSA_A_N)]}),
    );
    let post_rotation = build_state(
        PROD_ISSUER,
        PROD_JWKS_URI,
        json!({"keys":[rsa_jwk("rsa-new", "RS256", RSA_B_N)]}),
    );

    let old_token = sign_rs256(
        "rsa-old",
        &default_claims(PROD_ISSUER, AUDIENCE),
        RSA_A_PRIVATE_PEM,
    );
    let new_token = sign_rs256(
        "rsa-new",
        &default_claims(PROD_ISSUER, AUDIENCE),
        RSA_B_PRIVATE_PEM,
    );

    let old_pre_rotation = route_with_token(&pre_rotation, &old_token);
    let new_post_rotation = route_with_token(&post_rotation, &new_token);
    let old_post_rotation = route_with_token(&post_rotation, &old_token);

    assert_eq!(old_pre_rotation.status, 200);
    assert_eq!(new_post_rotation.status, 200);
    assert_eq!(old_post_rotation.status, 401);
    assert_eq!(
        old_post_rotation.body,
        json!({"error":"unauthorized","reason":"jwks_key_not_found"})
    );
}

#[test]
fn phase_e_req_e_001_adverse_cases_fail_for_alg_signature_key_issuer_and_audience() {
    let state = build_state(
        PROD_ISSUER,
        PROD_JWKS_URI,
        json!({"keys":[
            rsa_jwk("rsa-current", "RS256", RSA_A_N),
            rsa_jwk("rsa-alg-mismatch", "ES256", RSA_A_N)
        ]}),
    );

    let alg_mismatch_token = sign_rs256(
        "rsa-alg-mismatch",
        &default_claims(PROD_ISSUER, AUDIENCE),
        RSA_A_PRIVATE_PEM,
    );
    let wrong_key_token = sign_rs256(
        "rsa-current",
        &default_claims(PROD_ISSUER, AUDIENCE),
        RSA_B_PRIVATE_PEM,
    );
    let bad_signature_token = tamper_signature(&sign_rs256(
        "rsa-current",
        &default_claims(PROD_ISSUER, AUDIENCE),
        RSA_A_PRIVATE_PEM,
    ));
    let issuer_mismatch_token = sign_rs256(
        "rsa-current",
        &default_claims("https://attacker.example", AUDIENCE),
        RSA_A_PRIVATE_PEM,
    );
    let audience_mismatch_token = sign_rs256(
        "rsa-current",
        &default_claims(PROD_ISSUER, "wrong-audience"),
        RSA_A_PRIVATE_PEM,
    );

    let alg_mismatch = route_with_token(&state, &alg_mismatch_token);
    let wrong_key = route_with_token(&state, &wrong_key_token);
    let bad_signature = route_with_token(&state, &bad_signature_token);
    let issuer_mismatch = route_with_token(&state, &issuer_mismatch_token);
    let audience_mismatch = route_with_token(&state, &audience_mismatch_token);

    assert_eq!(alg_mismatch.status, 401);
    assert_eq!(
        alg_mismatch.body,
        json!({"error":"unauthorized","reason":"jwks_key_not_found"})
    );

    for denied in [wrong_key, bad_signature, issuer_mismatch, audience_mismatch] {
        assert_eq!(denied.status, 401);
        assert_eq!(
            denied.body,
            json!({"error":"unauthorized","reason":"invalid_token"})
        );
    }
}

#[test]
fn phase_e_req_e_002_hs256_is_dev_only() {
    let dev_state = build_state(
        DEV_ISSUER,
        DEV_JWKS_URI,
        json!({"keys":[hs_jwk("dev-hs", DEV_HS_SECRET)]}),
    );
    let prod_state = build_state(
        PROD_ISSUER,
        PROD_JWKS_URI,
        json!({"keys":[hs_jwk("dev-hs", DEV_HS_SECRET)]}),
    );

    let dev_token = sign_hs256(
        "dev-hs",
        &default_claims(DEV_ISSUER, AUDIENCE),
        DEV_HS_SECRET,
    );
    let prod_token = sign_hs256(
        "dev-hs",
        &default_claims(PROD_ISSUER, AUDIENCE),
        DEV_HS_SECRET,
    );

    let dev_allowed = route_with_token(&dev_state, &dev_token);
    let prod_denied = route_with_token(&prod_state, &prod_token);

    assert_eq!(dev_allowed.status, 200);
    assert_eq!(prod_denied.status, 401);
    assert_eq!(
        prod_denied.body,
        json!({"error":"unauthorized","reason":"hs256_not_allowed"})
    );
}

#[test]
fn phase_e_req_e_002_rejects_dot_local_for_hs256() {
    let dot_local_state = build_state(
        LOCAL_SUFFIX_ISSUER,
        LOCAL_SUFFIX_JWKS_URI,
        json!({"keys":[hs_jwk("dot-local-hs", DEV_HS_SECRET)]}),
    );
    let dot_local_token = sign_hs256(
        "dot-local-hs",
        &default_claims(LOCAL_SUFFIX_ISSUER, AUDIENCE),
        DEV_HS_SECRET,
    );

    let dot_local_denied = route_with_token(&dot_local_state, &dot_local_token);

    assert_eq!(dot_local_denied.status, 401);
    assert_eq!(
        dot_local_denied.body,
        json!({"error":"unauthorized","reason":"hs256_not_allowed"})
    );
}

#[test]
fn phase_e_req_e_003_allowlist_and_service_identity_are_preserved_in_console_flow() {
    let state = build_state(
        PROD_ISSUER,
        PROD_JWKS_URI,
        json!({"keys":[rsa_jwk("rsa-current", "RS256", RSA_A_N)]}),
    );

    let denied_user = sign_rs256(
        "rsa-current",
        &json!({
            "sub":"user-denied",
            "email":"mallory@example.com",
            "provider":"google_oauth",
            "iss":PROD_ISSUER,
            "aud":AUDIENCE,
            "exp":4_000_000_000_u64,
        }),
        RSA_A_PRIVATE_PEM,
    );
    let allowed_service = sign_rs256(
        "rsa-current",
        &json!({
            "sub":"svc:deploy-bot",
            "actor_type":"service",
            "provider":"generic_oidc",
            "iss":PROD_ISSUER,
            "aud":AUDIENCE,
            "exp":4_000_000_000_u64,
        }),
        RSA_A_PRIVATE_PEM,
    );
    let denied_service = sign_rs256(
        "rsa-current",
        &json!({
            "sub":"svc:not-allowlisted",
            "actor_type":"service",
            "provider":"generic_oidc",
            "iss":PROD_ISSUER,
            "aud":AUDIENCE,
            "exp":4_000_000_000_u64,
        }),
        RSA_A_PRIVATE_PEM,
    );

    let denied_user_response = route_with_token(&state, &denied_user);
    let allowed_service_response = route_with_token(&state, &allowed_service);
    let denied_service_response = route_with_token(&state, &denied_service);

    assert_eq!(denied_user_response.status, 401);
    assert_eq!(
        denied_user_response.body,
        json!({"error":"unauthorized","reason":"email_not_allowlisted"})
    );
    assert_eq!(allowed_service_response.status, 200);
    assert_eq!(denied_service_response.status, 401);
    assert_eq!(
        denied_service_response.body,
        json!({"error":"unauthorized","reason":"service_identity_not_allowlisted"})
    );
}

// ---------------------------------------------------------------------------
// provider: "oidc" console auth configuration (library embedders fronting the
// console with a real issuer such as Google)
// ---------------------------------------------------------------------------

/// Claims the way an external issuer such as Google mints them: no
/// Meta-specific `provider` claim (console ingress maps its absence to
/// `GenericOidc`, which the oidc policy expects).
fn external_issuer_claims(issuer: &str, audience: &str, email: &str) -> Value {
    json!({
        "sub": format!("sub-{email}"),
        "email": email,
        "email_verified": true,
        "iss": issuer,
        "aud": audience,
        "exp": 4_000_000_000_u64,
        "nbf": 1_700_000_000_u64
    })
}

fn oidc_config(jwks: Value) -> Value {
    json!({
        "provider": "oidc",
        "issuer": PROD_ISSUER,
        "audience": AUDIENCE,
        "jwks_json": jwks,
        "email_allowlist": ["alice@example.com"],
    })
}

#[test]
fn oidc_auth_config_accepts_an_rs256_token_from_the_inline_jwks() {
    let state = meerkat_meta::console_auth_config::parse_console_auth_config(&oidc_config(
        json!({"keys":[rsa_jwk("rsa-current", "RS256", RSA_A_N)]}),
    ))
    .expect("oidc auth config");
    assert!(state.console.require_app_auth);
    assert!(state.trusted_oidc.require_verified_email);
    let discovery: Value =
        serde_json::from_str(&state.trusted_oidc.discovery_json).expect("discovery json");
    assert_eq!(discovery["issuer"], PROD_ISSUER);
    assert_eq!(
        discovery["jwks_uri"],
        meerkat_meta::console_auth_config::OIDC_INLINE_JWKS_MARKER,
        "an inline JWKS must not invent a fetch URL"
    );
    let token = sign_rs256(
        "rsa-current",
        &external_issuer_claims(PROD_ISSUER, AUDIENCE, "alice@example.com"),
        RSA_A_PRIVATE_PEM,
    );
    assert_eq!(route_with_token(&state, &token).status, 200);
    // An allowlisted address on an UNVERIFIED account is not the person.
    let mut unverified = external_issuer_claims(PROD_ISSUER, AUDIENCE, "alice@example.com");
    unverified["email_verified"] = json!(false);
    let denied = route_with_token(
        &state,
        &sign_rs256("rsa-current", &unverified, RSA_A_PRIVATE_PEM),
    );
    assert_eq!(denied.status, 401);
    assert_eq!(denied.body["reason"], "email_not_verified");
    let mut missing_flag = external_issuer_claims(PROD_ISSUER, AUDIENCE, "alice@example.com");
    missing_flag
        .as_object_mut()
        .expect("object")
        .remove("email_verified");
    let denied = route_with_token(
        &state,
        &sign_rs256("rsa-current", &missing_flag, RSA_A_PRIVATE_PEM),
    );
    assert_eq!(denied.status, 401);
    assert_eq!(denied.body["reason"], "email_not_verified");
    // No email claim at all: `sub` must not stand in for the allowlist.
    let sub_only = json!({"sub":"alice@example.com","email_verified":true,"iss":PROD_ISSUER,
        "aud":AUDIENCE,"exp":4_000_000_000_u64,"nbf":1_700_000_000_u64});
    let denied = route_with_token(
        &state,
        &sign_rs256("rsa-current", &sub_only, RSA_A_PRIVATE_PEM),
    );
    assert_eq!(denied.status, 401);
    assert_eq!(denied.body["reason"], "missing_token_identity");
    let not_allowlisted = sign_rs256(
        "rsa-current",
        &external_issuer_claims(PROD_ISSUER, AUDIENCE, "mallory@example.com"),
        RSA_A_PRIVATE_PEM,
    );
    let denied = route_with_token(&state, &not_allowlisted);
    assert_eq!(denied.status, 401);
    assert_eq!(denied.body["reason"], "email_not_allowlisted");
}

#[test]
fn oidc_auth_config_rejects_a_wrong_audience_and_a_foreign_issuer() {
    // The JWKS may also arrive as a string (for example from an environment
    // variable).
    let mut config = oidc_config(Value::Null);
    config["jwks_json"] =
        Value::String(json!({"keys":[rsa_jwk("rsa-current", "RS256", RSA_A_N)]}).to_string());
    let state = meerkat_meta::console_auth_config::parse_console_auth_config(&config)
        .expect("oidc auth config from a JWKS string");
    let wrong_audience = sign_rs256(
        "rsa-current",
        &external_issuer_claims(PROD_ISSUER, "some-other-client-id", "alice@example.com"),
        RSA_A_PRIVATE_PEM,
    );
    assert_eq!(route_with_token(&state, &wrong_audience).status, 401);
    let foreign_issuer = sign_rs256(
        "rsa-current",
        &external_issuer_claims(
            "https://accounts.example.net",
            AUDIENCE,
            "alice@example.com",
        ),
        RSA_A_PRIVATE_PEM,
    );
    assert_eq!(route_with_token(&state, &foreign_issuer).status, 401);
    let hs256_forgery = sign_hs256(
        "rsa-current",
        &external_issuer_claims(PROD_ISSUER, AUDIENCE, "alice@example.com"),
        "guessable",
    );
    assert_eq!(route_with_token(&state, &hs256_forgery).status, 401);
}

#[test]
fn oidc_auth_config_fails_closed_on_incomplete_or_symmetric_input() {
    let parse = meerkat_meta::console_auth_config::parse_console_auth_config;
    let rsa = json!({"keys":[rsa_jwk("k", "RS256", RSA_A_N)]});
    let mut missing_audience = oidc_config(rsa.clone());
    missing_audience
        .as_object_mut()
        .expect("object")
        .remove("audience");
    assert!(parse(&missing_audience).unwrap_err().contains("audience"));
    let mut missing_allowlist = oidc_config(rsa.clone());
    missing_allowlist
        .as_object_mut()
        .expect("object")
        .remove("email_allowlist");
    assert!(
        parse(&missing_allowlist)
            .unwrap_err()
            .contains("email_allowlist")
    );
    let mut both_sources = oidc_config(rsa.clone());
    both_sources["jwks_uri"] = json!("https://accounts.google.com/.well-known/jwks");
    assert!(parse(&both_sources).unwrap_err().contains("exactly one"));
    let mut neither = oidc_config(Value::Null);
    neither.as_object_mut().expect("object").remove("jwks_json");
    assert!(
        parse(&neither)
            .unwrap_err()
            .contains("jwks_json or jwks_uri")
    );
    let mut plain_http = oidc_config(Value::Null);
    plain_http
        .as_object_mut()
        .expect("object")
        .remove("jwks_json");
    plain_http["jwks_uri"] = json!("http://accounts.google.com/certs");
    assert!(parse(&plain_http).unwrap_err().contains("https"));
    let symmetric = oidc_config(json!({"keys":[hs_jwk("dev", "console-test-signing")]}));
    assert!(parse(&symmetric).unwrap_err().contains("symmetric"));
    let empty = oidc_config(json!({"keys":[]}));
    assert!(parse(&empty).unwrap_err().contains("no keys"));
}

#[test]
fn jwt_development_provider_keeps_its_historical_principal_fallback() {
    let state = meerkat_meta::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": "console-test-signing",
        "email_allowlist": ["alice@example.com"],
    }))
    .expect("jwt auth config");
    assert!(!state.trusted_oidc.require_verified_email);
}
