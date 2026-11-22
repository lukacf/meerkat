//! Which host names the REST server answers for, by the `Host` a request
//! carries.
//!
//! Whatever address the server is bound to, it answers for `localhost` and
//! for an address literal, and for a name the realm's `[rest] served_hosts`
//! lists: a reverse proxy that forwards its public name, or a caller that
//! uses a container or service name, is refused until that name is listed.
//! Any other name is refused before any route runs, with `403` and the code
//! `HOST_NOT_SERVED`, a name that resolves to loopback included, and the
//! refusal names the host it refused. A request without a `Host` is served.
//!
//! The rows drive the router in process, composed as the binary composes it.
//! Every row keeps the realm state, the user configuration and the
//! credentials under its own temporary directory.

use super::allowed_origins_tests::{Answer, config_generation, load_rest_state, send};
use super::*;
use axum::body::Body;
use axum::http::{Request, header};
use tower::ServiceExt as _;

/// Bound on one request: it only turns a hang into a failure.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The public name a reverse proxy forwards.
const PUBLIC_NAME: &str = "api.example.com";
const REFUSAL_CODE: &str = "HOST_NOT_SERVED";

/// `GET /config` addressed to `host`.
fn read_addressed_to(host: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/config")
        .header(header::HOST, host)
        .body(Body::empty())
        .unwrap()
}

/// A `PATCH /config` of one harmless key, addressed to `host`.
fn write_addressed_to(host: &str) -> Request<Body> {
    Request::builder()
        .method("PATCH")
        .uri("/config")
        .header(header::HOST, host)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"rest": {"port": 8099}}).to_string()))
        .unwrap()
}

/// The refusal: `403` and the server's error envelope with the host code.
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
}

/// A reverse proxy on the same machine forwards the public name it was
/// called under. The server on loopback refuses that name, reads and writes,
/// until the realm lists it.
#[tokio::test]
async fn a_forwarded_public_name_is_refused_on_loopback_until_the_realm_lists_it() {
    let temp = TempDir::new().unwrap();
    let state = load_rest_state(&temp, "").await.expect("the state loads");
    let before = config_generation(&state).await;

    let read = send(&state, read_addressed_to(PUBLIC_NAME)).await;
    assert_refused(&read, "a read addressed to a name the realm does not list");
    let write = send(&state, write_addressed_to(PUBLIC_NAME)).await;
    assert_eq!(
        config_generation(&state).await,
        before,
        "the config document is not written: {}",
        write.body
    );
    assert_refused(
        &write,
        "a write addressed to a name the realm does not list",
    );

    let temp = TempDir::new().unwrap();
    let state = load_rest_state(&temp, &format!("served_hosts = [\"{PUBLIC_NAME}\"]"))
        .await
        .expect("the state loads");
    let before = config_generation(&state).await;
    for host in [
        PUBLIC_NAME,
        "api.example.com:443",
        "API.Example.com",
        "api.example.com.",
    ] {
        let read = send(&state, read_addressed_to(host)).await;
        assert_eq!(read.status, StatusCode::OK, "{host}: {}", read.body);
    }
    let write = send(&state, write_addressed_to(PUBLIC_NAME)).await;
    assert_eq!(write.status, StatusCode::OK, "{}", write.body);
    assert!(
        config_generation(&state).await > before,
        "a write addressed to a listed name is applied"
    );
    // Listing one name lists no other.
    let other = send(&state, read_addressed_to("other.example.com")).await;
    assert_refused(&other, "a name the realm does not list");
}

/// The spellings of the server's own names on loopback: `localhost` with
/// and without a port and with a trailing dot, both loopback literals, and
/// no `Host` at all are served.
#[tokio::test]
async fn loopback_names_are_served_on_loopback() {
    let temp = TempDir::new().unwrap();
    let state = load_rest_state(&temp, "").await.expect("the state loads");

    for host in [
        "localhost",
        "localhost:8080",
        "localhost.",
        "localhost.:8080",
        "LOCALHOST:8080",
        "127.0.0.1",
        "127.0.0.1:8080",
        "[::1]",
        "[::1]:8080",
    ] {
        let answer = send(&state, read_addressed_to(host)).await;
        assert_eq!(answer.status, StatusCode::OK, "{host}: {}", answer.body);
    }
    let no_host = send(
        &state,
        Request::builder()
            .method("GET")
            .uri("/config")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(no_host.status, StatusCode::OK, "{}", no_host.body);
}

/// A name that only resolves to loopback is not the server's name, and a
/// request names one host.
#[tokio::test]
async fn a_name_that_resolves_to_loopback_is_refused_on_loopback() {
    let temp = TempDir::new().unwrap();
    let state = load_rest_state(&temp, "").await.expect("the state loads");

    for host in [
        "app.localhost",
        "app.localhost:8080",
        "localhost.example.com",
        "127.0.0.1.example.com:8080",
        "localtest.example",
    ] {
        let answer = send(&state, read_addressed_to(host)).await;
        assert_refused(&answer, host);
    }

    let two = send(
        &state,
        Request::builder()
            .method("GET")
            .uri("/config")
            .header(header::HOST, "localhost:8080")
            .header(header::HOST, PUBLIC_NAME)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_refused(&two, "a request that names two hosts");
}

/// The check is the same on every bind. A server bound to the unspecified
/// address whose realm lists no name refuses a request addressed to a name,
/// a plain read included, and a write is not applied; it serves a request
/// addressed to `localhost` or to an address literal.
#[tokio::test]
async fn a_server_not_on_loopback_refuses_a_name_the_realm_does_not_list() {
    let temp = TempDir::new().unwrap();
    let state = load_rest_state(&temp, "host = \"0.0.0.0\"")
        .await
        .expect("the state loads");
    let before = config_generation(&state).await;

    for host in ["rkat-rest:8080", "rest.internal", PUBLIC_NAME] {
        let read = send(&state, read_addressed_to(host)).await;
        assert_refused(&read, host);
    }
    let write = send(&state, write_addressed_to("rkat-rest:8080")).await;
    assert_eq!(
        config_generation(&state).await,
        before,
        "the config document is not written: {}",
        write.body
    );
    assert_refused(
        &write,
        "a write addressed to a name the realm does not list",
    );

    for host in [
        "127.0.0.1:8080",
        "[::1]:8080",
        "localhost:8080",
        "10.0.0.7:8080",
        "[fd00::7]:8080",
    ] {
        let answer = send(&state, read_addressed_to(host)).await;
        assert_eq!(answer.status, StatusCode::OK, "{host}: {}", answer.body);
    }
}

/// A server bound to another address answers for the names the realm
/// lists, in any letter case, with a trailing dot or a port, for
/// `localhost` and for address literals, and for no other name.
#[tokio::test]
async fn a_server_not_on_loopback_answers_listed_names_once_the_realm_lists_names() {
    let temp = TempDir::new().unwrap();
    let state = load_rest_state(
        &temp,
        &format!("host = \"0.0.0.0\"\nserved_hosts = [\"{PUBLIC_NAME}\", \"rkat-rest\"]"),
    )
    .await
    .expect("the state loads");
    for host in [
        PUBLIC_NAME,
        "API.Example.com:443",
        "api.example.com.",
        "rkat-rest:8080",
        "localhost:8080",
        "10.0.0.7:8080",
        "[fd00::7]:8080",
    ] {
        let answer = send(&state, read_addressed_to(host)).await;
        assert_eq!(answer.status, StatusCode::OK, "{host}: {}", answer.body);
    }
    let other = send(&state, read_addressed_to("rest.internal")).await;
    assert_refused(&other, "a name the realm does not list");
}

/// The refusal names the host it refused, so that an operator sees what to
/// list: `details.host` holds the name as the server compares it (no port,
/// lower case), at most 253 characters, with control characters removed.
#[tokio::test]
async fn the_refusal_names_the_host_it_refused() {
    let temp = TempDir::new().unwrap();
    let state = load_rest_state(&temp, "").await.expect("the state loads");

    let refused_host = |answer: &Answer| -> Value {
        serde_json::from_str::<Value>(&answer.body).unwrap_or(Value::Null)["details"]["host"]
            .clone()
    };

    let answer = send(&state, read_addressed_to("Api.Example.com:8443")).await;
    assert_refused(&answer, "a name the realm does not list");
    assert_eq!(refused_host(&answer), "api.example.com", "{}", answer.body);
    assert!(
        answer.body.contains("api.example.com"),
        "the message names the host: {}",
        answer.body
    );

    // A header value may hold a tab; nothing else below a space.
    let with_a_tab = Request::builder()
        .method("GET")
        .uri("/config")
        .header(
            header::HOST,
            axum::http::HeaderValue::from_bytes(b"api\t.example.com").unwrap(),
        )
        .body(Body::empty())
        .unwrap();
    let answer = send(&state, with_a_tab).await;
    assert_refused(&answer, "a name with a control character");
    assert_eq!(refused_host(&answer), "api.example.com", "{}", answer.body);

    let long_name = format!("{}.example.com", "a".repeat(400));
    let answer = send(&state, read_addressed_to(&long_name)).await;
    assert_refused(&answer, "a long name");
    let named = refused_host(&answer);
    let named = named.as_str().unwrap_or_default();
    assert_eq!(named.len(), 253, "{}", answer.body);
    assert!(long_name.starts_with(named), "{}", answer.body);
}

/// `served_hosts` lists host names: no scheme, no port, no path, no
/// wildcard. A realm that lists anything else does not start.
#[tokio::test]
async fn a_realm_that_lists_something_other_than_a_host_name_does_not_start() {
    for entry in [
        "*",
        "*.example.com",
        "https://api.example.com",
        "api.example.com:443",
        "api.example.com/console",
        "api example.com",
        "",
    ] {
        let temp = TempDir::new().unwrap();
        let loaded = load_rest_state(&temp, &format!("served_hosts = [\"{entry}\"]")).await;
        match loaded {
            Ok(_) => panic!("a realm that lists `{entry}` as a served host started"),
            Err(error) => assert!(
                error.to_string().contains("rest.served_hosts"),
                "`{entry}`: the refusal names the key: {error}"
            ),
        }
    }
}

/// Every route passes the same check: for each documented path and method,
/// and for a path and a method the router does not know, a request
/// addressed to a name the server does not answer for is refused before the
/// route runs.
#[tokio::test]
async fn every_route_refuses_a_host_name_the_server_does_not_answer_for() {
    let temp = TempDir::new().unwrap();
    let state = load_rest_state(&temp, "").await.expect("the state loads");

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
    requests.push(("OPTIONS".to_string(), "/config".to_string()));

    let mut served = Vec::new();
    for (method, path) in &requests {
        let request = Request::builder()
            .method(method.as_str())
            .uri(path.as_str())
            .header(header::HOST, PUBLIC_NAME)
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
        let body = tokio::time::timeout(
            DEADLINE,
            http_body_util::BodyExt::collect(response.into_body()),
        )
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
        "{} of {} requests addressed to a name the server does not answer for reached past the \
         check:\n{}",
        served.len(),
        requests.len(),
        served.join("\n")
    );
}
