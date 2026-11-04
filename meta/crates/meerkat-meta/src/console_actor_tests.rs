//! `meta/console/whoami`: who the Console acts as for one request, derived
//! from the connection alone. A verified human token is a person, a service
//! token is a service, a token that names no human is a token principal,
//! and a request without a verified token on an open Console is anonymous.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::console_human_admission_tests::{BOB, state};
use super::console_token_lapse_tests::{console_rpc_as, human_claims, now_seconds, token};
use super::*;

const WHOAMI: &str = "meta/console/whoami";

/// The access configuration the tests read approver ids and the access
/// admin from.
fn access() -> AccessController {
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        approvers: std::collections::BTreeMap::from([(
            "person:bob".to_string(),
            crate::access::AccessApprover {
                subjects: vec![BOB.to_string()],
                groups: Vec::new(),
            },
        )]),
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller")
}

#[tokio::test]
async fn whoami_reports_the_actor_of_each_kind_of_caller() {
    let expiry = now_seconds() + 3600;
    let refused_from_ms = json!((expiry + 31) * 1000);
    let enforced = console_json_router_with_state(state());
    let ask = |app: &Router, bearer: Option<String>| {
        let app = app.clone();
        async move { console_rpc_as(&app, bearer.as_deref(), WHOAMI, json!({})).await }
    };

    let person = ask(&enforced, Some(token(human_claims(), Some(expiry)))).await;
    assert!(person["error"].is_null(), "{person}");
    assert_eq!(person["result"]["actor"]["kind"], "person", "{person}");
    assert_eq!(person["result"]["actor"]["principal"], BOB, "{person}");
    assert_eq!(
        person["result"]["actor"]["assurance"], "person_authenticated",
        "{person}"
    );
    assert_eq!(person["result"]["posture"], "enforced", "{person}");
    assert_eq!(
        person["result"]["login_expires_at_ms"], refused_from_ms,
        "{person}"
    );

    let service = ask(
        &enforced,
        Some(token(json!({ "email": "svc:robot" }), Some(expiry))),
    )
    .await;
    assert_eq!(service["result"]["actor"]["kind"], "service", "{service}");
    assert_eq!(
        service["result"]["actor"]["principal"], "svc:robot",
        "{service}"
    );
    assert_eq!(
        service["result"]["actor"]["assurance"], "service_credential",
        "{service}"
    );

    // A token that names no human (no `sub`) identifies a principal and
    // claims no assurance.
    let principal = ask(
        &enforced,
        Some(token(
            json!({ "email": BOB, "provider": "google_oauth" }),
            Some(expiry),
        )),
    )
    .await;
    assert_eq!(
        principal["result"]["actor"]["kind"], "token_principal",
        "{principal}"
    );
    assert_eq!(
        principal["result"]["actor"]["principal"], BOB,
        "{principal}"
    );
    assert!(
        principal["result"]["actor"]["assurance"].is_null(),
        "{principal}"
    );

    let mut open = state();
    open.decisions.console.require_app_auth = false;
    let anonymous = ask(&console_json_router_with_state(open.clone()), None).await;
    assert!(anonymous["error"].is_null(), "{anonymous}");
    assert_eq!(
        anonymous["result"]["actor"]["kind"], "anonymous",
        "{anonymous}"
    );
    assert!(
        anonymous["result"]["actor"]["principal"].is_null(),
        "{anonymous}"
    );
    assert!(
        anonymous["result"]["actor"]["assurance"].is_null(),
        "{anonymous}"
    );
    assert_eq!(anonymous["result"]["posture"], "open", "{anonymous}");
    assert!(
        anonymous["result"]["login_expires_at_ms"].is_null(),
        "{anonymous}"
    );
    assert_eq!(anonymous["result"]["can_administer"], false, "{anonymous}");
    assert_eq!(
        anonymous["result"]["approver_ids"],
        json!([]),
        "{anonymous}"
    );

    // On an open Console with an access controller, a caller that
    // volunteers a verified token is identified, as today.
    open.access = Some(access());
    let volunteered = ask(
        &console_json_router_with_state(open),
        Some(token(human_claims(), Some(expiry))),
    )
    .await;
    assert_eq!(
        volunteered["result"]["actor"]["kind"], "person",
        "{volunteered}"
    );
    assert_eq!(volunteered["result"]["posture"], "open", "{volunteered}");
    assert_eq!(
        volunteered["result"]["approver_ids"],
        json!(["person:bob"]),
        "{volunteered}"
    );
    assert_eq!(
        volunteered["result"]["can_administer"], false,
        "{volunteered}"
    );
}

/// The caller scope is an opaque digest of the caller the Console resolved:
/// stable for one caller, different for another, and shared by every
/// anonymous caller of an open Console.
#[tokio::test]
async fn whoami_caller_scope_is_stable_per_caller() {
    let expiry = now_seconds() + 3600;
    let enforced = console_json_router_with_state(state());
    let scope = |answer: &Value| answer["result"]["caller_scope"].clone();
    let bob = console_rpc_as(
        &enforced,
        Some(&token(human_claims(), Some(expiry))),
        WHOAMI,
        json!({}),
    )
    .await;
    let bob_again = console_rpc_as(
        &enforced,
        Some(&token(human_claims(), Some(expiry + 60))),
        WHOAMI,
        json!({}),
    )
    .await;
    let robot = console_rpc_as(
        &enforced,
        Some(&token(json!({ "email": "svc:robot" }), Some(expiry))),
        WHOAMI,
        json!({}),
    )
    .await;
    assert!(
        scope(&bob)
            .as_str()
            .is_some_and(|value| value.starts_with("console-caller:v1:")),
        "{bob}"
    );
    assert_eq!(scope(&bob), scope(&bob_again));
    assert_ne!(scope(&bob), scope(&robot));
}

/// A token minted with an `auth_config` shared secret (`provider: "jwt"`)
/// names a shared account, not a person, unless the configuration declares
/// `accounts: "personal"`: a front with one shared sign-in mints every
/// holder of its password the same `sub`.
#[tokio::test]
async fn shared_account_token_is_a_token_principal() {
    let issuer = "http://127.0.0.1/meta-gateway";
    let secret = "shared-front-secret";
    let decisions = |accounts: Option<&str>| {
        let mut config = json!({
            "provider": "jwt",
            "shared_secret": secret,
            "issuer": issuer,
            "audience": "shared-front",
            "email_allowlist": ["owner@example.test"],
        });
        if let Some(accounts) = accounts {
            config["accounts"] = json!(accounts);
        }
        crate::console_auth_config::parse_console_auth_config(&config)
    };
    let mint = || {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &json!({
                "iss": issuer,
                "aud": "shared-front",
                "sub": "owner@example.test",
                "email": "owner@example.test",
                "email_verified": true,
                "sid": "login-1",
                "exp": now_seconds() + 3600,
            }),
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .expect("signed fixture JWT")
    };
    for (accounts, kind) in [(None, "token_principal"), (Some("personal"), "person")] {
        let mut shared = state();
        shared.decisions = decisions(accounts).expect("auth config");
        let answer = console_rpc_as(
            &console_json_router_with_state(shared),
            Some(&mint()),
            WHOAMI,
            json!({}),
        )
        .await;
        assert_eq!(
            answer["result"]["actor"]["kind"], kind,
            "{accounts:?}: {answer}"
        );
    }
    assert!(
        decisions(Some("everyone")).is_err(),
        "an unknown accounts value is refused"
    );
}

/// `body` posted to `path` on `app` with no `Authorization` header: the
/// status.
async fn post_without_header(app: &Router, path: &str, body: Value) -> StatusCode {
    use tower::ServiceExt;
    app.clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .expect("console request"),
        )
        .await
        .expect("console response")
        .status()
}

/// `path` read on `app` with no `Authorization` header: the status.
async fn get_without_header(app: &Router, path: &str) -> StatusCode {
    use tower::ServiceExt;
    app.clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri(path)
                .body(axum::body::Body::empty())
                .expect("console request"),
        )
        .await
        .expect("console response")
        .status()
}

/// A Console write takes its token in the `Authorization` header: a POST
/// whose only token is in the query string is refused 401, since a token in
/// a URL reaches proxy logs and browser history. A GET still takes a query
/// token (`EventSource` cannot set headers).
#[tokio::test]
async fn post_with_query_token_only_is_refused() {
    let enforced = console_json_router_with_state(state());
    let bob = token(human_claims(), Some(now_seconds() + 3600));
    let whoami = json!({ "jsonrpc": "2.0", "id": 1, "method": WHOAMI, "params": {} });
    assert_eq!(
        post_without_header(
            &enforced,
            &format!("/console/rpc?auth_token={bob}"),
            whoami.clone()
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "a console RPC with only a query token"
    );
    let send = json!({
        "identity": "agent-a",
        "content": "hello",
        "origin": "console",
        "idempotency_key": "query-token-send",
    });
    assert_eq!(
        post_without_header(&enforced, &format!("/console/send?auth_token={bob}"), send).await,
        StatusCode::UNAUTHORIZED,
        "a console send with only a query token"
    );
    // On an open Console too: the token is refused, not dropped for an
    // anonymous call.
    let mut open = state();
    open.decisions.console.require_app_auth = false;
    assert_eq!(
        post_without_header(
            &console_json_router_with_state(open),
            &format!("/console/rpc?auth_token={bob}"),
            whoami
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "an open console's RPC with only a query token"
    );
    // The header form is answered.
    let person = console_rpc_as(&enforced, Some(&bob), WHOAMI, json!({})).await;
    assert_eq!(person["result"]["actor"]["kind"], "person", "{person}");
    // A GET is judged on its query token.
    assert_eq!(
        get_without_header(&enforced, "/console/experience?auth_token=not-a-token").await,
        StatusCode::UNAUTHORIZED,
        "a GET with a bad query token"
    );
    assert_ne!(
        get_without_header(&enforced, &format!("/console/experience?auth_token={bob}")).await,
        StatusCode::UNAUTHORIZED,
        "a GET with a good query token"
    );
}
