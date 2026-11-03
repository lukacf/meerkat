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
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use axum::http::{StatusCode, header};
use axum::routing::get;
use axum::{Extension, Router};
use serde_json::json;
use tower::ServiceExt;

use meerkat_meta::{JwksCache, JwksCacheConfig, JwksCacheError, ValidatedJwt, with_auth_layer};

// ---------------------------------------------------------------------------
// Inline mock HTTP server that supports pre-allocated ports
// ---------------------------------------------------------------------------

struct OidcMockServer {
    base_url: String,
    _join_handle: thread::JoinHandle<()>,
    captured_requests: Arc<Mutex<Vec<String>>>,
}

impl OidcMockServer {
    /// Start a mock OIDC server that serves discovery then JWKS responses in order.
    /// The `build_responses` closure receives the base_url so jwks_uri can reference it.
    fn start(build_responses: impl FnOnce(&str) -> Vec<(u16, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("set nonblocking");
        let addr = listener.local_addr().expect("local addr");
        let base_url = format!("http://{addr}");

        let responses = build_responses(&base_url);
        let captured_requests = Arc::new(Mutex::new(Vec::new()));
        let thread_captured = Arc::clone(&captured_requests);

        let join_handle = thread::spawn(move || {
            for (status, body) in responses {
                let mut stream = wait_for_connection(&listener, Duration::from_secs(5));
                let request_line = read_request_line(&mut stream);
                thread_captured.lock().expect("lock").push(request_line);
                let response = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
                stream.flush().expect("flush");
            }
        });

        OidcMockServer {
            base_url,
            _join_handle: join_handle,
            captured_requests,
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn request_count(&self) -> usize {
        self.captured_requests.lock().expect("lock").len()
    }
}

fn wait_for_connection(listener: &TcpListener, timeout: Duration) -> TcpStream {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).expect("set stream blocking");
                return stream;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for connection"
                );
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("accept failed: {e}"),
        }
    }
}

fn read_request_line(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set timeout");
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).expect("read");
    let text = String::from_utf8_lossy(&buf[..n]);
    text.lines().next().unwrap_or("").to_string()
}

fn json_body(value: &serde_json::Value) -> String {
    serde_json::to_string(value).expect("serialize")
}

// ---------------------------------------------------------------------------
// Helper: build an HS256-signed JWT with the `jsonwebtoken` dev-dependency
// ---------------------------------------------------------------------------

fn build_hs256_token(secret: &[u8], kid: Option<&str>, claims: &serde_json::Value) -> String {
    use jsonwebtoken::{EncodingKey, Header, encode};
    let mut header = Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = kid.map(ToString::to_string);
    encode(&header, claims, &EncodingKey::from_secret(secret)).expect("encode test JWT")
}

fn hs256_secret() -> Vec<u8> {
    b"super-secret-test-key-32-bytes!!".to_vec()
}

fn hs256_jwk_json(secret: &[u8], kid: &str) -> serde_json::Value {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let k = URL_SAFE_NO_PAD.encode(secret);
    json!({
        "kty": "oct",
        "kid": kid,
        "alg": "HS256",
        "k": k,
    })
}

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Build a standard pair of discovery + JWKS response bodies.
fn discovery_jwks_responses(base_url: &str, secret: &[u8], kid: &str) -> Vec<(u16, String)> {
    vec![
        (
            200,
            json_body(&json!({
                "issuer": "https://test-issuer.example.com",
                "jwks_uri": format!("{base_url}/jwks"),
            })),
        ),
        (
            200,
            json_body(&json!({
                "keys": [hs256_jwk_json(secret, kid)]
            })),
        ),
    ]
}

/// Audience the test tokens are minted for. The exported `JwksCache` now fails
/// closed when no audience is configured, so the test helpers must set one.
const TEST_AUDIENCE: &str = "meta-test-audience";

fn make_cache(base_url: &str) -> JwksCache {
    let config = JwksCacheConfig {
        discovery_url: format!("{base_url}/.well-known/openid-configuration"),
        refresh_interval: Duration::from_hours(1),
        http_timeout: Duration::from_secs(5),
        issuer: None,
        audience: Some(TEST_AUDIENCE.to_string()),
        leeway_seconds: 60,
        // These tests use HS256 as a convenient symmetric signer; opt in
        // explicitly (the cache rejects HS256 by default).
        allow_hs256: true,
    };
    JwksCache::new(config)
}

fn make_cache_with_issuer(base_url: &str, issuer: &str) -> JwksCache {
    let config = JwksCacheConfig {
        discovery_url: format!("{base_url}/.well-known/openid-configuration"),
        refresh_interval: Duration::from_hours(1),
        http_timeout: Duration::from_secs(5),
        issuer: Some(issuer.to_string()),
        audience: Some(TEST_AUDIENCE.to_string()),
        leeway_seconds: 60,
        allow_hs256: true,
    };
    JwksCache::new(config)
}

// ---------------------------------------------------------------------------
// JwksCache tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn jwks_cache_refresh_and_validate_token() {
    let secret = hs256_secret();
    let claims = json!({
        "sub": "user-1",
        "iss": "https://test-issuer.example.com",
        "aud": TEST_AUDIENCE,
        "exp": now_epoch() + 3600,
    });
    let token = build_hs256_token(&secret, Some("key-1"), &claims);

    let mock =
        OidcMockServer::start(|base_url| discovery_jwks_responses(base_url, &secret, "key-1"));

    let cache = make_cache_with_issuer(mock.base_url(), "https://test-issuer.example.com");
    let jwt = cache
        .validate_token(&token)
        .await
        .expect("token validation should succeed");
    assert_eq!(jwt.subject.as_deref(), Some("user-1"));
    assert_eq!(
        jwt.issuer.as_deref(),
        Some("https://test-issuer.example.com")
    );
}

#[tokio::test]
async fn jwks_cache_kid_miss_triggers_refresh() {
    let secret = hs256_secret();
    let claims = json!({
        "sub": "user-2",
        "aud": TEST_AUDIENCE,
        "exp": now_epoch() + 3600,
    });
    let token = build_hs256_token(&secret, Some("key-2"), &claims);

    let mock = OidcMockServer::start(|base_url| {
        let base = base_url.to_string();
        vec![
            // Initial refresh — discovery
            (
                200,
                json_body(&json!({
                    "issuer": "https://test-issuer.example.com",
                    "jwks_uri": format!("{base}/jwks"),
                })),
            ),
            // Initial refresh — JWKS with key-1 only (kid miss)
            (
                200,
                json_body(&json!({
                    "keys": [hs256_jwk_json(&secret, "key-1")]
                })),
            ),
            // Kid-miss refresh — discovery
            (
                200,
                json_body(&json!({
                    "issuer": "https://test-issuer.example.com",
                    "jwks_uri": format!("{base}/jwks"),
                })),
            ),
            // Kid-miss refresh — JWKS with key-2
            (
                200,
                json_body(&json!({
                    "keys": [hs256_jwk_json(&secret, "key-2")]
                })),
            ),
        ]
    });

    let cache = make_cache(mock.base_url());
    let jwt = cache
        .validate_token(&token)
        .await
        .expect("should succeed after kid-miss refresh");
    assert_eq!(jwt.subject.as_deref(), Some("user-2"));

    // Verify 4 HTTP requests were made (2 refresh cycles of discovery+JWKS).
    assert_eq!(
        mock.request_count(),
        4,
        "expected 4 HTTP requests (2 refresh cycles)"
    );
}

#[tokio::test]
async fn jwks_cache_returns_error_on_invalid_token() {
    let secret = hs256_secret();
    let mock =
        OidcMockServer::start(|base_url| discovery_jwks_responses(base_url, &secret, "key-1"));

    let cache = make_cache(mock.base_url());
    let result = cache.validate_token("not.a.valid-token").await;
    assert!(result.is_err(), "invalid token should fail validation");
}

#[tokio::test]
async fn jwks_cache_expired_token_rejected() {
    let secret = hs256_secret();
    let claims = json!({
        "sub": "expired-user",
        "exp": now_epoch() - 3600,
    });
    let token = build_hs256_token(&secret, Some("key-1"), &claims);

    let mock =
        OidcMockServer::start(|base_url| discovery_jwks_responses(base_url, &secret, "key-1"));

    let cache = make_cache(mock.base_url());
    let result = cache.validate_token(&token).await;
    assert!(result.is_err(), "expired token should be rejected");
}

#[tokio::test]
async fn jwks_cache_rejects_hs256_by_default() {
    // Security: the exported JwksCache must reject HS256 by default to avoid
    // algorithm confusion if the IdP's JWKS publishes an oct/alg-less key.
    let secret = hs256_secret();
    let claims = json!({
        "sub": "user-1",
        "aud": TEST_AUDIENCE,
        "exp": now_epoch() + 3600,
    });
    let token = build_hs256_token(&secret, Some("key-1"), &claims);
    let mock =
        OidcMockServer::start(|base_url| discovery_jwks_responses(base_url, &secret, "key-1"));

    // Default config (no allow_hs256): rejected.
    let config = JwksCacheConfig {
        discovery_url: format!("{}/.well-known/openid-configuration", mock.base_url()),
        refresh_interval: Duration::from_hours(1),
        http_timeout: Duration::from_secs(5),
        issuer: None,
        audience: Some(TEST_AUDIENCE.to_string()),
        leeway_seconds: 60,
        allow_hs256: false,
    };
    let cache = JwksCache::new(config);
    let result = cache.validate_token(&token).await;
    assert!(
        matches!(result, Err(JwksCacheError::Hs256NotAllowed)),
        "HS256 must be rejected by default, got {result:?}"
    );
}

#[tokio::test]
async fn jwks_cache_fails_closed_without_audience() {
    // Security: with no configured audience the cache must fail closed to
    // prevent cross-RP token reuse, rather than accepting any unexpired token.
    let secret = hs256_secret();
    let claims = json!({
        "sub": "user-1",
        "aud": "some-other-rp",
        "exp": now_epoch() + 3600,
    });
    let token = build_hs256_token(&secret, Some("key-1"), &claims);
    let mock =
        OidcMockServer::start(|base_url| discovery_jwks_responses(base_url, &secret, "key-1"));

    let config = JwksCacheConfig {
        discovery_url: format!("{}/.well-known/openid-configuration", mock.base_url()),
        refresh_interval: Duration::from_hours(1),
        http_timeout: Duration::from_secs(5),
        issuer: None,
        audience: None,
        leeway_seconds: 60,
        // HS256 allowed so the failure is attributable to the missing audience.
        allow_hs256: true,
    };
    let cache = JwksCache::new(config);
    let result = cache.validate_token(&token).await;
    assert!(
        matches!(result, Err(JwksCacheError::AudienceRequired)),
        "missing audience must fail closed, got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Auth middleware tests
// ---------------------------------------------------------------------------

async fn echo_identity(Extension(jwt): Extension<ValidatedJwt>) -> axum::Json<serde_json::Value> {
    axum::Json(json!({
        "sub": jwt.subject,
        "email": jwt.email,
    }))
}

fn build_test_app(cache: JwksCache) -> Router {
    let inner = Router::new().route("/protected", get(echo_identity));
    with_auth_layer(inner, cache)
}

#[tokio::test]
async fn auth_middleware_returns_401_without_token() {
    let secret = hs256_secret();
    let mock =
        OidcMockServer::start(|base_url| discovery_jwks_responses(base_url, &secret, "key-1"));

    let app = build_test_app(make_cache(mock.base_url()));

    let request = axum::http::Request::builder()
        .uri("/protected")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_middleware_returns_401_with_invalid_token() {
    let secret = hs256_secret();
    let mock = OidcMockServer::start(|base_url| {
        let base = base_url.to_string();
        vec![
            // Initial refresh
            (
                200,
                json_body(&json!({
                    "issuer": "https://test-issuer.example.com",
                    "jwks_uri": format!("{base}/jwks"),
                })),
            ),
            (
                200,
                json_body(&json!({
                    "keys": [hs256_jwk_json(&secret, "key-1")]
                })),
            ),
        ]
    });

    let app = build_test_app(make_cache(mock.base_url()));

    // Token signed with wrong secret
    let wrong_secret = b"wrong-secret-key-also-32-bytes!!";
    let claims = json!({ "sub": "hacker", "exp": now_epoch() + 3600 });
    let bad_token = build_hs256_token(wrong_secret, Some("key-1"), &claims);

    let request = axum::http::Request::builder()
        .uri("/protected")
        .header(header::AUTHORIZATION, format!("Bearer {bad_token}"))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_middleware_passes_valid_token_and_injects_identity() {
    let secret = hs256_secret();
    let claims = json!({
        "sub": "user-42",
        "email": "user42@example.com",
        "aud": TEST_AUDIENCE,
        "exp": now_epoch() + 3600,
    });
    let token = build_hs256_token(&secret, Some("key-1"), &claims);

    let mock =
        OidcMockServer::start(|base_url| discovery_jwks_responses(base_url, &secret, "key-1"));

    let app = build_test_app(make_cache(mock.base_url()));

    let request = axum::http::Request::builder()
        .uri("/protected")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["sub"], "user-42");
    assert_eq!(json["email"], "user42@example.com");
}
