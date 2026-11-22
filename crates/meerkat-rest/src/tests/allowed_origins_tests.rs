//! Which callers the REST server answers, by the `Origin` a request carries.
//!
//! A program that is not a browser sends no `Origin` header, and is served as
//! before. A browser page sends its origin, and the server answers it only
//! when the realm's `[rest] allowed_origins` names that origin: the answer
//! then carries a cross-origin allowance for that one origin. Every other
//! origin is refused before any route runs, a preflight included, with `403`
//! and the code `ORIGIN_NOT_ALLOWED`, and gets no cross-origin allowance.
//! `null` is an origin nobody can list. A request that a browser marks as
//! coming from another site, and that carries no origin, is refused as well.
//!
//! The rows drive the router in process, composed as the binary composes it.
//! Every row keeps the realm state, the user configuration and the
//! credentials under its own temporary directory.

use super::*;
use axum::body::Body;
use axum::http::{HeaderMap, Request, header};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

/// Bound on one request: it only turns a hang into a failure.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

const REALM: &str = "allowed-origins";
/// A page served by another local server: another origin.
const OTHER_ORIGIN: &str = "http://localhost:5173";
const ALLOWED_ORIGIN: &str = "https://app.example.com";
const SECOND_ALLOWED_ORIGIN: &str = "http://127.0.0.1:3000";
const REFUSAL_CODE: &str = "ORIGIN_NOT_ALLOWED";

/// A server state composed as the binary composes it, on a realm whose
/// config document holds `rest_keys` in its `[rest]` table.
pub(super) async fn load_rest_state(
    temp: &TempDir,
    rest_keys: &str,
) -> Result<AppState, Box<dyn std::error::Error>> {
    let state_root = temp.path().join("realms");
    let realm_root = state_root.join(REALM);
    tokio::fs::create_dir_all(&realm_root).await.unwrap();
    tokio::fs::write(
        realm_root.join("config.toml"),
        format!("[rest]\n{rest_keys}\n"),
    )
    .await
    .unwrap();
    let mut bootstrap = RuntimeBootstrap::default();
    bootstrap.realm.selection = RealmSelection::Explicit {
        realm_id: REALM.to_string(),
    };
    bootstrap.realm.state_root = Some(state_root);
    bootstrap.context.context_root = Some(temp.path().to_path_buf());
    bootstrap.context.user_config_root = Some(temp.path().join("user"));
    AppState::load_from_with_bootstrap(
        temp.path().to_path_buf(),
        bootstrap,
        false,
        test_credential_store(temp.path()),
    )
    .await
}

/// A state whose realm allows no origin: the default.
async fn rest_state(temp: &TempDir) -> AppState {
    load_rest_state(temp, "")
        .await
        .expect("the server state loads")
}

/// A state whose realm allows [`ALLOWED_ORIGIN`] and
/// [`SECOND_ALLOWED_ORIGIN`].
async fn rest_state_allowing_two_origins(temp: &TempDir) -> AppState {
    load_rest_state(
        temp,
        &format!("allowed_origins = [\"{ALLOWED_ORIGIN}\", \"{SECOND_ALLOWED_ORIGIN}\"]"),
    )
    .await
    .expect("the server state loads")
}

pub(super) struct Answer {
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) body: String,
}

impl Answer {
    /// The names of the cross-origin response headers the answer carries.
    fn cross_origin_headers(&self) -> Vec<String> {
        self.headers
            .keys()
            .map(|name| name.as_str().to_string())
            .filter(|name| name.starts_with("access-control-"))
            .collect()
    }

    /// Every value of one header, lowercased and joined.
    fn header_values(&self, name: header::HeaderName) -> String {
        self.headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>()
            .join(", ")
            .to_ascii_lowercase()
    }
}

pub(super) async fn send(state: &AppState, request: Request<Body>) -> Answer {
    let response = tokio::time::timeout(DEADLINE, router(state.clone()).oneshot(request))
        .await
        .expect("the request answers")
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = tokio::time::timeout(DEADLINE, response.into_body().collect())
        .await
        .expect("the answer's body ends")
        .unwrap()
        .to_bytes();
    Answer {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

fn get_config_request() -> axum::http::request::Builder {
    Request::builder().method("GET").uri("/config")
}

/// A `PATCH /config` of one harmless key.
fn patch_config_request() -> axum::http::request::Builder {
    Request::builder()
        .method("PATCH")
        .uri("/config")
        .header(header::CONTENT_TYPE, "application/json")
}

fn patch_config_body() -> Body {
    Body::from(json!({"rest": {"port": 8099}}).to_string())
}

/// The preflight a browser sends before a cross-origin `PATCH /config` with
/// a JSON body.
fn preflight_request(origin: &str) -> Request<Body> {
    Request::builder()
        .method("OPTIONS")
        .uri("/config")
        .header(header::ORIGIN, origin)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PATCH")
        .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
        .body(Body::empty())
        .unwrap()
}

pub(super) async fn config_generation(state: &AppState) -> u64 {
    state
        .config_runtime
        .get()
        .await
        .expect("the config document reads")
        .generation
}

/// The refusal: `403`, the server's error envelope with the origin code, and
/// no cross-origin allowance.
fn assert_refused(answer: &Answer, what: &str) {
    assert_eq!(
        answer.status,
        StatusCode::FORBIDDEN,
        "{what}: {}",
        answer.body
    );
    let payload: Value = serde_json::from_str(&answer.body).unwrap_or(Value::Null);
    assert_eq!(payload["code"], REFUSAL_CODE, "{what}: {}", answer.body);
    assert!(payload["error"].is_string(), "{what}: {}", answer.body);
    assert_eq!(
        answer.cross_origin_headers(),
        Vec::<String>::new(),
        "{what}: a refused request gets no cross-origin allowance"
    );
}

#[tokio::test]
async fn a_read_from_an_origin_that_is_not_allowed_is_refused() {
    let temp = TempDir::new().unwrap();
    let state = rest_state(&temp).await;

    let answer = send(
        &state,
        get_config_request()
            .header(header::ORIGIN, OTHER_ORIGIN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_refused(&answer, "GET /config from an origin that is not allowed");
}

#[tokio::test]
async fn a_write_from_an_origin_that_is_not_allowed_is_refused_and_writes_nothing() {
    let temp = TempDir::new().unwrap();
    let state = rest_state(&temp).await;
    let before = config_generation(&state).await;

    let answer = send(
        &state,
        patch_config_request()
            .header(header::ORIGIN, OTHER_ORIGIN)
            .body(patch_config_body())
            .unwrap(),
    )
    .await;

    assert_eq!(
        config_generation(&state).await,
        before,
        "the config document is not written: {}",
        answer.body
    );
    assert_refused(&answer, "PATCH /config from an origin that is not allowed");
}

#[tokio::test]
async fn a_preflight_from_an_origin_that_is_not_allowed_is_refused() {
    let temp = TempDir::new().unwrap();
    let state = rest_state(&temp).await;

    let answer = send(&state, preflight_request(OTHER_ORIGIN)).await;

    assert_refused(&answer, "the preflight of an origin that is not allowed");
}

/// A session create from an origin that is not allowed is refused before
/// the route runs: no session is created, with the tool switches on or off.
#[tokio::test]
async fn a_session_create_from_an_origin_that_is_not_allowed_creates_nothing() {
    let temp = TempDir::new().unwrap();
    let mut state = rest_state(&temp).await;
    state.llm_client_override = Some(Arc::new(MockLlmClient));

    for body in [
        json!({"prompt": "Hello", "model": "claude-sonnet-4-6"}),
        json!({
            "prompt": "Hello",
            "model": "claude-sonnet-4-6",
            "enable_builtins": true,
            "enable_shell": true,
        }),
    ] {
        let answer = send(
            &state,
            Request::builder()
                .method("POST")
                .uri("/sessions")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, OTHER_ORIGIN)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        assert_refused(&answer, "POST /sessions from an origin that is not allowed");
    }

    let listed = send(
        &state,
        Request::builder()
            .method("GET")
            .uri("/sessions")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
    let payload: Value = serde_json::from_str(&listed.body).unwrap();
    assert_eq!(
        payload["sessions"],
        json!([]),
        "no session was created: {}",
        listed.body
    );
}

/// The control: a caller that sends no `Origin` is served as before, reads
/// and writes, and never meets the refusal.
#[tokio::test]
async fn a_request_without_an_origin_is_served() {
    let temp = TempDir::new().unwrap();
    let state = rest_state(&temp).await;
    let before = config_generation(&state).await;

    let health = send(
        &state,
        Request::builder()
            .method("GET")
            .uri("/health")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(health.status, StatusCode::OK, "{}", health.body);

    let read = send(&state, get_config_request().body(Body::empty()).unwrap()).await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.body);
    assert_eq!(
        read.cross_origin_headers(),
        Vec::<String>::new(),
        "an answer to a caller that named no origin carries no cross-origin allowance"
    );

    let write = send(
        &state,
        patch_config_request().body(patch_config_body()).unwrap(),
    )
    .await;
    assert_eq!(write.status, StatusCode::OK, "{}", write.body);
    assert!(
        config_generation(&state).await > before,
        "the write of a caller that named no origin is applied"
    );
}

#[tokio::test]
async fn an_allowed_origin_is_served_and_the_answer_names_only_that_origin() {
    let temp = TempDir::new().unwrap();
    let state = rest_state_allowing_two_origins(&temp).await;

    for origin in [ALLOWED_ORIGIN, SECOND_ALLOWED_ORIGIN] {
        let read = send(
            &state,
            get_config_request()
                .header(header::ORIGIN, origin)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(read.status, StatusCode::OK, "{origin}: {}", read.body);
        assert_eq!(
            read.header_values(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            origin,
            "the answer allows the asking origin and no other"
        );
        assert!(
            read.header_values(header::VARY).contains("origin"),
            "the answer varies by origin: {:?}",
            read.headers
        );

        let preflight = send(&state, preflight_request(origin)).await;
        assert!(
            preflight.status.is_success(),
            "{origin}: the preflight of an allowed origin is answered: {} {}",
            preflight.status,
            preflight.body
        );
        assert_eq!(
            preflight.header_values(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            origin,
            "the preflight's answer allows the asking origin and no other"
        );
        assert!(
            preflight
                .header_values(header::ACCESS_CONTROL_ALLOW_METHODS)
                .contains("patch"),
            "the preflight's answer allows the route's method: {:?}",
            preflight.headers
        );
        assert!(
            preflight
                .header_values(header::ACCESS_CONTROL_ALLOW_HEADERS)
                .contains("content-type"),
            "the preflight's answer allows the JSON content type: {:?}",
            preflight.headers
        );
    }

    let before = config_generation(&state).await;
    let write = send(
        &state,
        patch_config_request()
            .header(header::ORIGIN, ALLOWED_ORIGIN)
            .body(patch_config_body())
            .unwrap(),
    )
    .await;
    assert_eq!(write.status, StatusCode::OK, "{}", write.body);
    assert!(
        config_generation(&state).await > before,
        "the write of an allowed origin is applied"
    );

    // Allowing two origins allows no third.
    let other = send(
        &state,
        get_config_request()
            .header(header::ORIGIN, OTHER_ORIGIN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_refused(&other, "an origin the realm does not list");
    let other_preflight = send(&state, preflight_request(OTHER_ORIGIN)).await;
    assert_refused(
        &other_preflight,
        "the preflight of an origin the realm does not list",
    );
}

/// An origin is compared whole. One that only resembles a listed origin
/// (a longer host, another port, another scheme, a subdomain, a trailing
/// dot) is another origin, and so is an `Origin` header with no value.
#[tokio::test]
async fn an_origin_that_resembles_a_listed_one_is_refused() {
    let temp = TempDir::new().unwrap();
    let state = rest_state_allowing_two_origins(&temp).await;

    for origin in [
        "https://app.example.com.other.example",
        "https://app.example.com:8443",
        "http://app.example.com",
        "https://sub.app.example.com",
        "https://app.example.com.",
        "https://app.example.co",
        "app.example.com",
        "",
    ] {
        let answer = send(
            &state,
            get_config_request()
                .header(header::ORIGIN, origin)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_refused(&answer, &format!("the origin `{origin}`"));
    }
}

#[tokio::test]
async fn the_null_origin_and_a_request_with_two_origins_are_refused() {
    let temp = TempDir::new().unwrap();
    let state = rest_state_allowing_two_origins(&temp).await;

    let null = send(
        &state,
        get_config_request()
            .header(header::ORIGIN, "null")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_refused(&null, "the origin `null`");

    let two = send(
        &state,
        get_config_request()
            .header(header::ORIGIN, ALLOWED_ORIGIN)
            .header(header::ORIGIN, OTHER_ORIGIN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_refused(&two, "a request that carries two origins");
}

/// A browser sends no `Origin` on some requests to another site; it marks
/// them `Sec-Fetch-Site: cross-site`. Such a request names no origin the
/// realm could have allowed.
#[tokio::test]
async fn a_cross_site_request_without_an_origin_is_refused() {
    let temp = TempDir::new().unwrap();
    let state = rest_state_allowing_two_origins(&temp).await;

    let cross_site = send(
        &state,
        get_config_request()
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_refused(&cross_site, "a cross-site request without an origin");

    // The control: the address typed into a browser, and a caller that sends
    // the header with no site to name. `same-site` is served too: another
    // port of the same host is the same site to a browser, and the rule
    // refuses `cross-site` only.
    for site in ["none", "same-origin", "same-site"] {
        let answer = send(
            &state,
            get_config_request()
                .header("sec-fetch-site", site)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(answer.status, StatusCode::OK, "{site}: {}", answer.body);
    }
}

/// `allowed_origins` lists origins: a scheme, a host and an optional port.
/// A wildcard, `null`, a URL with a path and a bare host are not origins,
/// and a realm that lists one does not start.
#[tokio::test]
async fn a_realm_that_lists_something_other_than_an_origin_does_not_start() {
    for entry in [
        "*",
        "null",
        "https://app.example.com/",
        "https://app.example.com/console",
        "https://user@app.example.com",
        "app.example.com",
        "https://",
        "",
    ] {
        let temp = TempDir::new().unwrap();
        let loaded = load_rest_state(&temp, &format!("allowed_origins = [\"{entry}\"]")).await;
        match loaded {
            Ok(_) => panic!("a realm that lists `{entry}` as an allowed origin started"),
            Err(error) => assert!(
                error.to_string().contains("rest.allowed_origins"),
                "`{entry}`: the refusal names the key: {error}"
            ),
        }
    }

    let temp = TempDir::new().unwrap();
    rest_state_allowing_two_origins(&temp).await;
}

/// Every route passes the same check: for each documented path and method,
/// and for a path and a method the router does not know, a request from an
/// origin that is not allowed is refused before the route runs.
#[tokio::test]
async fn every_route_refuses_an_origin_that_is_not_allowed() {
    let temp = TempDir::new().unwrap();
    let state = rest_state(&temp).await;

    let mut requests: Vec<(String, String)> = Vec::new();
    for descriptor in meerkat_contracts::rest_path_catalog() {
        // A path parameter is any one segment.
        let path = descriptor
            .path
            .split('/')
            .map(|segment| {
                if segment.starts_with('{') {
                    "x"
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        for operation in &descriptor.operations {
            requests.push((operation.method.to_ascii_uppercase(), path.clone()));
        }
    }
    assert!(
        requests.len() >= 70,
        "the catalog lists the server's routes: {}",
        requests.len()
    );
    requests.push(("GET".to_string(), "/no-such-route".to_string()));
    requests.push(("DELETE".to_string(), "/health".to_string()));

    let mut served = Vec::new();
    for (method, path) in &requests {
        let request = Request::builder()
            .method(method.as_str())
            .uri(path.as_str())
            .header(header::ORIGIN, OTHER_ORIGIN)
            .body(Body::empty())
            .unwrap();
        let answered = tokio::time::timeout(DEADLINE, router(state.clone()).oneshot(request)).await;
        let Ok(Ok(response)) = answered else {
            served.push(format!("{method} {path}: no answer"));
            continue;
        };
        let status = response.status();
        if status != StatusCode::FORBIDDEN {
            served.push(format!("{method} {path}: {status}"));
            continue;
        }
        let body = tokio::time::timeout(DEADLINE, response.into_body().collect())
            .await
            .expect("a refusal's body ends")
            .unwrap()
            .to_bytes();
        let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if payload["code"] != REFUSAL_CODE {
            served.push(format!("{method} {path}: 403 {payload}"));
        }
    }
    assert!(
        served.is_empty(),
        "{} of {} requests from an origin that is not allowed reached past the check:\n{}",
        served.len(),
        requests.len(),
        served.join("\n")
    );
}
