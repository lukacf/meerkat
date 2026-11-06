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
