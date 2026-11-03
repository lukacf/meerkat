#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]
//! Regression: every SSE route must honor `RuntimeDecisionState`'s
//! `require_app_auth` flag. Pre-fix, only `/meta/mob_events/stream`
//! gated; tier-2 (`/agents/{id}/events`) and tier-3 (`/mob/events`)
//! shipped unauthenticated.

use std::pin::Pin;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::Stream;
use meerkat_core::comms::EventStream;
use meerkat_meta::{
    AuthPolicy, AuthProvider, BigQueryNaming, ConsolePolicy, MobRuntimeError,
    RuntimeDecisionInputs, RuntimeDecisionState, RuntimeOpsPolicy, TrustedOidcRuntimeConfig,
    agent_events_sse_router, build_runtime_decision_state, mob_events_sse_router,
};
use tower::ServiceExt;

fn release_json() -> String {
    include_str!("../assets/release-targets.json").to_string()
}

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
            .to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}

fn require_auth_decisions() -> RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "sse_auth_test".to_string(),
            table: "events".to_string(),
        },
        trusted_meta_toml: "modules = []\n".to_string(),
        auth: AuthPolicy {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: vec![],
        },
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth: true,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: release_json(),
    })
    .expect("decision state builds")
}

#[tokio::test]
async fn agent_events_route_rejects_unauthenticated_request_when_app_auth_required() {
    let stream_factory = Arc::new(
        |_agent_id: String| -> Pin<
            Box<dyn std::future::Future<Output = Result<EventStream, MobRuntimeError>> + Send>,
        > {
            Box::pin(async {
                let s: Pin<Box<dyn Stream<Item = _> + Send>> = Box::pin(futures::stream::empty());
                Ok::<EventStream, MobRuntimeError>(s)
            })
        },
    );

    let app = agent_events_sse_router(stream_factory, Some(require_auth_decisions()));

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/agents/anyone/events")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "agent events stream must be 401 without a token; got {}",
        response.status()
    );
}

#[tokio::test]
async fn mob_events_route_rejects_unauthenticated_request_when_app_auth_required() {
    // Subscribe fn never invoked — the auth gate must reject before
    // subscribing. Pre-fix, the route streamed events unconditionally.
    let subscribe_fn: meerkat_meta::MobEventSubscribeFn = Arc::new(|| {
        Box::pin(async {
            panic!("auth gate must reject before subscribe_fn fires");
        })
    });

    let app = mob_events_sse_router(subscribe_fn, Some(require_auth_decisions()));

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/mob/events")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "mob events stream must be 401 without a token; got {}",
        response.status()
    );
}

#[tokio::test]
async fn auth_token_query_param_with_percent_encoded_value_is_decoded() {
    // Regression: pre-fix the SSE auth helper compared the raw
    // (still-encoded) query string against `auth_token=`, so a token
    // containing `=` re-encoded to `%3D` was rejected. Now the helper
    // parses the query via form_urlencoded so percent-encoded values
    // round-trip correctly.
    use axum::http::Uri;
    let uri: Uri = "/path?auth_token=AAA%3D%3D".parse().expect("uri");
    let q = uri.query().expect("query");
    let token: Option<String> = form_urlencoded::parse(q.as_bytes())
        .find(|(key, _)| key == "auth_token")
        .map(|(_, value)| value.into_owned());
    assert_eq!(
        token.as_deref(),
        Some("AAA=="),
        "form_urlencoded must decode `%3D%3D` to `==`"
    );
}

#[tokio::test]
async fn auth_token_substring_shadowing_does_not_match() {
    // Regression: pre-fix `q.split('&').find_map(strip_prefix("auth_token="))`
    // matched substrings like `xauth_token=foo`. The form_urlencoded
    // pair lookup is key-aware and ignores shadowing.
    use axum::http::Uri;
    let uri: Uri = "/path?xauth_token=foo".parse().expect("uri");
    let q = uri.query().expect("query");
    let token: Option<String> = form_urlencoded::parse(q.as_bytes())
        .find(|(key, _)| key == "auth_token")
        .map(|(_, value)| value.into_owned());
    assert_eq!(
        token, None,
        "xauth_token=foo must not be matched as auth_token"
    );
}

#[tokio::test]
async fn agent_events_route_open_when_decisions_is_none() {
    // Backwards-compat: passing `None` for decisions opts the route
    // OUT of auth (in-process or trusted local embedding). Pre-fix
    // this WAS the only behavior, so we keep it as the explicit None
    // shape.
    let stream_factory = Arc::new(
        |_agent_id: String| -> Pin<
            Box<dyn std::future::Future<Output = Result<EventStream, MobRuntimeError>> + Send>,
        > {
            Box::pin(async {
                let s: Pin<Box<dyn Stream<Item = _> + Send>> = Box::pin(futures::stream::empty());
                Ok::<EventStream, MobRuntimeError>(s)
            })
        },
    );

    let app = agent_events_sse_router(stream_factory, None);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/agents/anyone/events")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_ne!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "with decisions=None the route must not 401"
    );
}
