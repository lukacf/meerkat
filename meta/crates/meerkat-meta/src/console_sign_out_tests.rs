//! `meta/console/sign_out`: a caller signs out of one console login (the
//! issuer's `sid`, or the token itself when it carries none) and keeps its
//! other logins and its schedules. Revoking a principal still ends every
//! login of that principal.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::console_human_admission_tests::{BOB, state};
use super::console_token_lapse_tests::{durable_fences, human_claims, now_seconds, token};
use super::*;

const SIGN_OUT: &str = "meta/console/sign_out";
const WHOAMI: &str = "meta/console/whoami";

/// A token of Bob in the login `sid` (a login of its own without one),
/// issued at `iat` and expiring in an hour.
pub(super) fn login_token(sid: Option<&str>, iat: u64) -> String {
    let mut claims = human_claims();
    if let Some(sid) = sid {
        claims["sid"] = json!(sid);
    }
    claims["iat"] = json!(iat);
    token(claims, Some(now_seconds() + 3600))
}

/// A service token, another principal than Bob.
fn robot_token() -> String {
    token(
        json!({ "sub": "svc:robot", "email": "svc:robot", "actor_type": "service" }),
        Some(now_seconds() + 3600),
    )
}

/// `method` with `params` on the console route as `token`, or with no
/// credentials: the HTTP status and the JSON-RPC answer.
pub(super) async fn console_answer(
    app: &Router,
    token: Option<&str>,
    method: &str,
    params: Value,
) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(axum::body::Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "id": "test",
                        "method": method,
                        "params": params,
                    })
                    .to_string(),
                ))
                .expect("console request"),
        )
        .await
        .expect("console response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("console body");
    (
        status,
        serde_json::from_slice(&body).expect("console answer"),
    )
}

/// Whether the console route accepts `token` now: its `whoami` answers.
pub(super) async fn accepted(app: &Router, token: &str) -> bool {
    let (status, answer) = console_answer(app, Some(token), WHOAMI, json!({})).await;
    status == StatusCode::OK && answer["error"].is_null()
}

/// The status of the console experience read with `token` in its query
/// string, as an `EventSource` or an image load sends one.
async fn experience_status(app: &Router, token: &str) -> StatusCode {
    use tower::ServiceExt;
    app.clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri(format!("/console/experience?auth_token={token}"))
                .body(axum::body::Body::empty())
                .expect("console request"),
        )
        .await
        .expect("console response")
        .status()
}

/// An enforced console over a durable session fence record in a fresh
/// state directory.
fn signed_in_console() -> (
    tempfile::TempDir,
    crate::auth::session_fence::ConsoleSessionFences,
    Router,
) {
    let (dir, fences) = durable_fences();
    let mut console = state();
    console.session_fences = fences.clone();
    (dir, fences, console_json_router_with_state(console))
}

/// Logins L1 and L2 of one person: after L1 signs out, every token of L1
/// minted before the sign-out is refused on every route, and L2 still
/// works. A sign-in again on L1 after the sign-out is a new start, and a
/// token without `sid` is a login of its own.
#[tokio::test]
async fn sign_out_fences_one_login() {
    let (_dir, _fences, app) = signed_in_console();
    let issued = now_seconds() - 60;
    let first = login_token(Some("login-1"), issued);
    let second = login_token(Some("login-2"), issued);
    assert!(accepted(&app, &first).await, "the first login is signed in");
    assert!(
        accepted(&app, &second).await,
        "the second login is signed in"
    );

    let (status, signed_out) = console_answer(&app, Some(&first), SIGN_OUT, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{signed_out}");
    assert!(signed_out["error"].is_null(), "{signed_out}");
    assert_eq!(signed_out["result"]["login"], "session", "{signed_out}");
    let accepts_from = signed_out["result"]["accepts_tokens_issued_from"]
        .as_u64()
        .expect("the first token of the login accepted again");

    // The first login is refused from now on, on the console's writes and
    // on its reads, and so is any other token of it minted before.
    assert!(!accepted(&app, &first).await, "the token that signed out");
    assert_eq!(
        experience_status(&app, &first).await,
        StatusCode::UNAUTHORIZED,
        "the token that signed out, on a read"
    );
    assert!(
        !accepted(&app, &login_token(Some("login-1"), now_seconds())).await,
        "another token of the login that signed out"
    );

    // The second login is untouched.
    assert!(accepted(&app, &second).await, "the other login");
    assert_ne!(
        experience_status(&app, &second).await,
        StatusCode::UNAUTHORIZED,
        "the other login, on a read"
    );

    // Signing in again on the same login after the sign-out starts it over.
    assert!(
        accepted(&app, &login_token(Some("login-1"), accepts_from)).await,
        "a sign-in again after the sign-out"
    );

    // A token without `sid` is its own login: its sign-out ends that token.
    let bare = login_token(None, issued);
    let other_bare = login_token(None, issued + 1);
    let (status, signed_out) = console_answer(&app, Some(&bare), SIGN_OUT, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{signed_out}");
    assert!(signed_out["error"].is_null(), "{signed_out}");
    assert_eq!(signed_out["result"]["login"], "token", "{signed_out}");
    assert!(
        signed_out["result"]["accepts_tokens_issued_from"].is_null(),
        "{signed_out}"
    );
    assert!(!accepted(&app, &bare).await, "the token that signed out");
    assert!(
        accepted(&app, &other_bare).await,
        "another token without sid"
    );
    assert!(accepted(&app, &second).await, "the other login");
}

/// Revoking a principal's sessions ends every login of that principal: each
/// `sid` login and each token without one. Another principal is untouched.
/// (A principal revoke also stops the schedules the principal defined
/// before it; the schedule tests pin that.)
#[tokio::test]
async fn principal_revoke_ends_all_logins() {
    let (_dir, _fences, app) = signed_in_console();
    let issued = now_seconds() - 60;
    let logins = [
        login_token(Some("login-1"), issued),
        login_token(Some("login-2"), issued),
        login_token(None, issued),
    ];
    let robot = robot_token();
    for login in &logins {
        assert!(accepted(&app, login).await, "signed in before the revoke");
    }
    // On a console without an access controller any caller a token
    // identified may revoke.
    let (status, revoked) = console_answer(
        &app,
        Some(&robot),
        "meta/console/revoke_sessions",
        json!({ "principal": BOB }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert!(revoked["error"].is_null(), "{revoked}");
    for login in &logins {
        assert!(!accepted(&app, login).await, "refused after the revoke");
    }
    assert!(accepted(&app, &robot).await, "another principal");
}

/// A person keeps at most 64 sign-outs of logins that only a fence of the
/// person ends: the 65th folds the oldest into a fence of every login of the
/// person. A login of the person older than the folded sign-out is signed
/// out with it, the folded login stays refused, the kept sign-outs still
/// refuse their logins, and a login newer than the fold keeps working. The
/// fold revokes nothing: the person's own fence is untouched.
#[tokio::test]
async fn the_sign_out_cap_folds_the_oldest_login() {
    let (_dir, fences, app) = signed_in_console();
    let older = login_token(Some("older"), now_seconds() - 60);
    assert!(accepted(&app, &older).await, "an older login is signed in");
    // Each login's token says it was issued a second after the one before,
    // later than the test's clock, so each sign-out fences its login at a
    // second of its own (a sign-out fences at the token's `iat` when that is
    // later than now).
    let first_fence = now_seconds() + 1_000;
    let mut tokens = Vec::new();
    for login in 0..65_u64 {
        let token = login_token(Some(&format!("login-{login}")), first_fence + login);
        let (status, signed_out) = console_answer(&app, Some(&token), SIGN_OUT, json!({})).await;
        assert_eq!(status, StatusCode::OK, "{signed_out}");
        assert!(signed_out["error"].is_null(), "{signed_out}");
        tokens.push(token);
    }

    assert!(
        !accepted(&app, &older).await,
        "an older login is signed out with the folded sign-out"
    );
    assert!(
        !accepted(&app, &tokens[0]).await,
        "the folded login stays refused"
    );
    assert!(
        !accepted(&app, &tokens[64]).await,
        "a kept sign-out still refuses its login"
    );
    let newer = login_token(
        Some("newer"),
        crate::runtime::session_fence_first_accepted_iat(first_fence),
    );
    assert!(accepted(&app, &newer).await, "a newer login keeps working");
    assert_eq!(fences.not_before(BOB), None, "the fold revokes nothing");
}

/// A sign-out needs a login, a durable record and the caller it was made
/// for: an open console's caller without a token has none and is answered
/// 401; a console whose session fence record has no file refuses it, as it
/// refuses a revocation; a sign-out made for another caller is refused with
/// `caller_changed`. A read-only console still signs a caller out: ending
/// one's own login changes nothing else.
#[tokio::test]
async fn sign_out_needs_a_login_and_a_durable_record() {
    let issued = now_seconds() - 60;

    let mut open = state();
    open.decisions.console.require_app_auth = false;
    let (status, answer) = console_answer(
        &console_json_router_with_state(open),
        None,
        SIGN_OUT,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
    assert_eq!(
        answer["error"]["data"]["kind"], "unauthenticated",
        "{answer}"
    );

    let detached = console_json_router_with_state(state());
    let login = login_token(Some("login-1"), issued);
    let (status, answer) = console_answer(&detached, Some(&login), SIGN_OUT, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["error"]["code"],
        json!(crate::rpc::CAPABILITY_UNAVAILABLE_CODE),
        "{answer}"
    );
    assert!(accepted(&detached, &login).await, "nothing was signed out");

    let (_dir, _fences, app) = signed_in_console();
    let scope_of = |answer: Value| answer["result"]["caller_scope"].clone();
    let robot_scope = scope_of(
        console_answer(&app, Some(&robot_token()), WHOAMI, json!({}))
            .await
            .1,
    );
    let (status, answer) = console_answer(
        &app,
        Some(&login),
        SIGN_OUT,
        json!({ "expected_caller_scope": robot_scope }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["error"]["code"], json!(-32056), "{answer}");
    assert_eq!(
        answer["error"]["data"]["kind"], "caller_changed",
        "{answer}"
    );
    assert!(accepted(&app, &login).await, "nothing was signed out");
    let own_scope = scope_of(
        console_answer(&app, Some(&login), WHOAMI, json!({}))
            .await
            .1,
    );
    let (status, answer) = console_answer(
        &app,
        Some(&login),
        SIGN_OUT,
        json!({ "expected_caller_scope": own_scope }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(answer["error"].is_null(), "{answer}");
    assert!(
        !accepted(&app, &login).await,
        "signed out for its own caller"
    );

    let (_read_only_dir, read_only_fences) = durable_fences();
    let mut read_only = state();
    read_only.decisions.console.read_only = true;
    read_only.session_fences = read_only_fences;
    let app = console_json_router_with_state(read_only);
    let login = login_token(Some("login-3"), issued);
    let (status, answer) = console_answer(&app, Some(&login), SIGN_OUT, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(answer["error"].is_null(), "{answer}");
    assert!(
        !accepted(&app, &login).await,
        "signed out of a read-only console"
    );
}

/// A sign-out wakes every open stream of the console, and a stream of the
/// login that signed out finds its caller no longer current, while a stream
/// of another login of the same person stays current: each stream verifies
/// its retained token again when the record changes.
#[tokio::test]
async fn a_sign_out_ends_the_streams_of_its_login_only() {
    let (_dir, fences, app) = signed_in_console();
    let decisions = state().decisions;
    let issued = now_seconds() - 60;
    let first = login_token(Some("login-1"), issued);
    let second = login_token(Some("login-2"), issued);
    let opened = |token: &str| {
        crate::runtime::resolve_authorized_console_caller_from_token(&decisions, &fences, token)
            .expect("a signed-in caller")
            .verification
    };
    let (first_stream, second_stream) = (opened(&first), opened(&second));
    let mut first_wakes = first_stream.session_fence_changes();
    let mut second_wakes = second_stream.session_fence_changes();
    first_wakes.mark_unchanged();
    second_wakes.mark_unchanged();

    let (status, signed_out) = console_answer(&app, Some(&first), SIGN_OUT, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{signed_out}");
    assert!(signed_out["error"].is_null(), "{signed_out}");

    assert!(first_wakes.has_changed().expect("the record is alive"));
    assert!(second_wakes.has_changed().expect("the record is alive"));
    let now = std::time::SystemTime::now();
    assert!(
        first_stream.current_at(now).is_none(),
        "the stream of the login that signed out ends"
    );
    assert!(
        second_stream.current_at(now).is_some(),
        "the stream of the other login stays open"
    );
}

/// `n - s` for the P-256 group order `n` and an ECDSA `s` (32 bytes each,
/// big-endian): the other valid `s` of a signature with the same `r`.
fn p256_order_minus(s: &[u8]) -> [u8; 32] {
    const ORDER: [u8; 32] = [
        0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xBC, 0xE6, 0xFA, 0xAD, 0xA7, 0x17, 0x9E, 0x84, 0xF3, 0xB9, 0xCA, 0xC2, 0xFC, 0x63,
        0x25, 0x51,
    ];
    let mut out = [0_u8; 32];
    let mut borrow = 0_u16;
    for ((out, order), s) in out.iter_mut().zip(ORDER).zip(s).rev() {
        let subtrahend = u16::from(*s) + borrow;
        let minuend = u16::from(order);
        let (value, next_borrow) = if minuend >= subtrahend {
            (minuend - subtrahend, 0)
        } else {
            (minuend + 256 - subtrahend, 1)
        };
        *out = u8::try_from(value).expect("a byte");
        borrow = next_borrow;
    }
    out
}

/// A token without `sid` is a login of its own. Once it signed out, the
/// same token (the same header and claims, signed by the issuer) stays
/// refused whatever valid encoding of its signature it comes with: an
/// ES256 signature `(r, s)` also verifies as `(r, n - s)`.
#[tokio::test]
async fn a_signed_out_es256_token_stays_refused_with_its_other_signature() {
    use base64::Engine as _;
    use ring::signature::KeyPair as _;
    let rng = ring::rand::SystemRandom::new();
    let algorithm = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(algorithm, &rng).expect("pkcs8");
    let key = ring::signature::EcdsaKeyPair::from_pkcs8(algorithm, pkcs8.as_ref(), &rng)
        .expect("key pair");
    let public = key.public_key().as_ref().to_vec();
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let (_dir, fences) = durable_fences();
    let mut console = state();
    let mut jwks: Value =
        serde_json::from_str(&console.decisions.trusted_oidc.jwks_json).expect("jwks");
    jwks["keys"].as_array_mut().expect("keys").push(json!({
        "kid": "es-signer",
        "kty": "EC",
        "crv": "P-256",
        "alg": "ES256",
        "x": b64.encode(&public[1..33]),
        "y": b64.encode(&public[33..65]),
    }));
    console.decisions.trusted_oidc.jwks_json = jwks.to_string();
    console.session_fences = fences;
    let app = console_json_router_with_state(console);

    let now = now_seconds();
    let header =
        b64.encode(json!({ "alg": "ES256", "typ": "JWT", "kid": "es-signer" }).to_string());
    let payload = b64.encode(
        json!({
            "iss": "https://trusted.meta.localhost",
            "aud": "meerkat-console",
            "sub": "oidc-sub-7",
            "email": BOB,
            "provider": "google_oauth",
            "iat": now - 60,
            "exp": now + 3600,
        })
        .to_string(),
    );
    let signing_input = format!("{header}.{payload}");
    let signature = key.sign(&rng, signing_input.as_bytes()).expect("sign");
    let signature = signature.as_ref();
    let original = format!("{signing_input}.{}", b64.encode(signature));
    let mut other = [0_u8; 64];
    other[..32].copy_from_slice(&signature[..32]);
    other[32..].copy_from_slice(&p256_order_minus(&signature[32..]));
    let other_encoding = format!("{signing_input}.{}", b64.encode(other));
    assert_ne!(original, other_encoding, "two encodings of one token");

    assert!(
        accepted(&app, &original).await,
        "an ES256 token is accepted"
    );
    assert!(
        accepted(&app, &other_encoding).await,
        "its other signature verifies too"
    );
    let (status, signed_out) = console_answer(&app, Some(&original), SIGN_OUT, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{signed_out}");
    assert!(signed_out["error"].is_null(), "{signed_out}");
    assert_eq!(signed_out["result"]["login"], "token", "{signed_out}");
    assert!(
        !accepted(&app, &original).await,
        "the token that signed out"
    );
    assert!(
        !accepted(&app, &other_encoding).await,
        "the token that signed out, with its other valid signature"
    );
}

/// The sign-out of a token without `sid` refuses that token for as long as
/// the verifier would accept it, its expiry leeway included: a token that
/// expired 10 s ago is accepted for 20 s more, and its sign-out holds
/// through a later change of the record (each change drops the sign-outs of
/// tokens the verifier refuses on expiry, and this one it does not refuse
/// yet).
#[tokio::test]
async fn a_signed_out_token_stays_refused_through_its_expiry_leeway() {
    let (_dir, _fences, app) = signed_in_console();
    let now = now_seconds();
    let mut claims = human_claims();
    claims["iat"] = json!(now - 60);
    let lapsing = token(claims, Some(now - 10));
    assert!(
        accepted(&app, &lapsing).await,
        "accepted within the expiry leeway"
    );
    let (status, signed_out) = console_answer(&app, Some(&lapsing), SIGN_OUT, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{signed_out}");
    assert!(signed_out["error"].is_null(), "{signed_out}");
    assert_eq!(signed_out["result"]["login"], "token", "{signed_out}");
    assert!(
        !accepted(&app, &lapsing).await,
        "refused right after its sign-out"
    );
    let other = login_token(Some("another-login"), now - 60);
    let (_, changed) = console_answer(&app, Some(&other), SIGN_OUT, json!({})).await;
    assert!(changed["error"].is_null(), "{changed}");
    assert!(
        !accepted(&app, &lapsing).await,
        "still refused after a later change of the record, inside its leeway"
    );
}
