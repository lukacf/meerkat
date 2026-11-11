//! What the Responses client does with a failure reported inside an HTTP
//! 200 stream.
//!
//! The stream shape these rows are about: a `response.failed` event whose
//! `error` is null, and after it an `error` event that carries a code and a
//! message, at the top level of the event or under `error`. The shape is
//! synthetic: it was not observed from a live service (a live request the
//! GitHub Copilot route refused was answered with HTTP 400 and the code
//! `invalid_request_body`). The rows feed it to the real Responses client
//! from a local server and pin what the client does when it meets it:
//!
//! - a `response.failed` without an error object decides nothing by itself:
//!   the cause is the `error` event after it, and a stream that ends, or
//!   goes on for too many frames, with no cause fails as a retryable error
//!   that says so;
//! - the flat and the nested error forms give the same typed error, with the
//!   code and the message in it;
//! - a code that names a refused request is the invalid-request class an
//!   HTTP 400 gives, and `model_not_found` the class an HTTP 404 gives:
//!   terminal, not retried;
//! - the classes the stream mapping already had are unchanged, and a
//!   transient one stays retryable.
//!
//! The same parser serves every backend of this client, so each row runs on
//! the public wire and on the Copilot wire.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Router, http::HeaderMap};
use futures::StreamExt;
use meerkat_core::error::{LlmFailureReason, LlmProviderErrorKind};
use meerkat_core::{Message, UserMessage};
use meerkat_llm_core::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::OpenAiClient;

const MODEL: &str = "gpt-5.5";

/// Every code that names a refused request and takes the class of an HTTP
/// 400. `model_not_found` has a row of its own: its class is the one of an
/// HTTP 404.
const REFUSAL_CODES: &[&str] = &[
    "invalid_request_body",
    "invalid_encrypted_content",
    "invalid_request_error",
    "invalid_prompt",
    "invalid_value",
    "invalid_argument",
    "unsupported_value",
    "unsupported_parameter",
];

const REFUSAL_TEXT: &str = "input[3]: the encrypted item could not be read";

/// One wire of the Responses client: its label and how to build its client
/// for a base URL.
type Wire = (&'static str, fn(String) -> OpenAiClient);

fn public_wire(base_url: String) -> OpenAiClient {
    OpenAiClient::new_with_base_url("test-key".to_string(), base_url)
}

fn copilot_wire(base_url: String) -> OpenAiClient {
    public_wire(base_url)
        .with_copilot_backend_wire()
        .with_copilot_responses_item_identity()
}

const WIRES: &[Wire] = &[("public", public_wire), ("copilot", copilot_wire)];

/// A `response.failed` event whose response carries no error object.
fn failed_without_error() -> Value {
    json!({"type": "response.failed", "response": {"id": "resp_1", "status": "failed", "error": null}})
}

/// A `response.failed` event whose response carries the cause.
fn failed_with(code: &str, message: &str) -> Value {
    json!({
        "type": "response.failed",
        "response": {"id": "resp_1", "status": "failed", "error": {"code": code, "message": message}},
    })
}

/// An `error` event with `code` and `message` at the top level.
fn flat_error(code: &str, message: &str) -> Value {
    json!({"type": "error", "code": code, "message": message})
}

/// An `error` event with `code` and `message` under `error`.
fn nested_error(code: &str, message: &str) -> Value {
    json!({"type": "error", "error": {"code": code, "message": message}})
}

/// A completed response with one line of answer text.
fn completed() -> Value {
    json!({
        "type": "response.completed",
        "response": {
            "id": "resp_1",
            "status": "completed",
            "output": [{
                "type": "message",
                "id": "msg_1",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "late answer", "annotations": []}],
            }],
            "usage": {"input_tokens": 3, "output_tokens": 2},
        },
    })
}

/// `frames` as one SSE body, ended by `[DONE]` when `done`.
fn sse(frames: &[Value], done: bool) -> String {
    let mut body = String::new();
    for frame in frames {
        body.push_str(&format!("data: {frame}\n\n"));
    }
    if done {
        body.push_str("data: [DONE]\n\n");
    }
    body
}

#[derive(Clone)]
struct Stub {
    body: Arc<String>,
    requests: Arc<AtomicUsize>,
    /// Echo the request's Authorization value in place of `{authorization}`.
    echo_authorization: bool,
}

async fn responses(State(stub): State<Stub>, headers: HeaderMap) -> impl IntoResponse {
    stub.requests.fetch_add(1, Ordering::SeqCst);
    let mut body = stub.body.as_str().to_owned();
    if stub.echo_authorization {
        let authorization = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        body = body.replace("{authorization}", authorization);
    }
    ([("content-type", "text/event-stream")], body)
}

/// Every event of one request answered with `body` over HTTP 200, and how
/// many requests the client sent.
async fn events_of(
    client: impl FnOnce(String) -> OpenAiClient,
    body: String,
    echo_authorization: bool,
) -> (Vec<Result<LlmEvent, LlmError>>, usize) {
    let requests = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/v1/responses", post(responses))
        .with_state(Stub {
            body: Arc::new(body),
            requests: Arc::clone(&requests),
            echo_authorization,
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind stub");
    let addr = listener.local_addr().expect("stub addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve stub");
    });
    let client = client(format!("http://{addr}"));
    let request = LlmRequest::new(MODEL, vec![Message::User(UserMessage::text("hello"))]);
    let events = client.stream(&request).collect::<Vec<_>>().await;
    server.abort();
    (events, requests.load(Ordering::SeqCst))
}

/// The error the stream of `body` ends with: its last event, the only `Done`,
/// after one request.
async fn terminal_error(wire: Wire, body: String) -> LlmError {
    let (label, client) = wire;
    let (events, requests) = events_of(client, body, false).await;
    assert_eq!(requests, 1, "{label}: the client sends one request");
    terminal_error_of(label, events)
}

fn terminal_error_of(label: &str, events: Vec<Result<LlmEvent, LlmError>>) -> LlmError {
    let done = events
        .iter()
        .filter(|event| matches!(event, Ok(LlmEvent::Done { .. })))
        .count();
    assert_eq!(done, 1, "{label}: exactly one Done: {events:?}");
    match events.into_iter().last() {
        Some(Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::Error { error },
        })) => error,
        other => panic!("{label}: the stream ends with an error, not {other:?}"),
    }
}

/// `error` is the terminal invalid-request class with `code` and `text` in
/// what the caller sees.
fn assert_terminal_refusal(case: &str, error: &LlmError, code: &str, text: &str) {
    let LlmError::InvalidRequest { message } = error else {
        panic!("{case}: an invalid request, not {error:?}");
    };
    assert!(
        message.contains(code) && message.contains(text),
        "{case}: the code and the message reach the caller: {message}"
    );
    assert!(!error.is_retryable(), "{case}: terminal: {error:?}");
    let LlmFailureReason::ProviderError(provider_error) = error.failure_reason() else {
        panic!("{case}: a typed provider failure");
    };
    assert_eq!(
        provider_error.kind,
        LlmProviderErrorKind::InvalidRequest,
        "{case}"
    );
    assert!(!provider_error.is_retryable(), "{case}");
    assert!(
        provider_error.details.to_string().contains(code)
            && provider_error.details.to_string().contains(text),
        "{case}: the durable reason carries the code and the message: {}",
        provider_error.details
    );
}

/// (a) HTTP 200, `response.failed` with a null error, then the cause in a
/// flat `error` event: the response fails as a terminal invalid request that
/// carries the code and the message. The first cause decides; what follows
/// it (a second error, a bare `response.completed`, `[DONE]`) changes
/// nothing.
#[tokio::test]
async fn a_failed_response_without_an_error_takes_its_cause_from_the_error_event() {
    for wire in WIRES {
        let body = sse(
            &[
                json!({"type": "response.created", "response": {"id": "resp_1", "status": "in_progress", "error": null}}),
                failed_without_error(),
                flat_error("invalid_request_body", REFUSAL_TEXT),
                flat_error("server_error", "a later frame"),
                json!({"type": "response.completed"}),
            ],
            true,
        );
        let error = terminal_error(*wire, body).await;
        assert_terminal_refusal(wire.0, &error, "invalid_request_body", REFUSAL_TEXT);
        assert_eq!(
            error.to_string(),
            format!("Invalid request: invalid_request_body: {REFUSAL_TEXT}"),
            "{}",
            wire.0
        );
    }
}

/// (b) The cause reads the same wherever the stream puts it: in a flat
/// `error` event, in a nested one, either of them after a `response.failed`
/// with a null error or alone, or inside the `response.failed` itself.
#[tokio::test]
async fn the_flat_and_the_nested_error_forms_give_the_same_error() {
    let code = "invalid_value";
    let shapes: [(&str, Vec<Value>); 5] = [
        (
            "failed without error, flat error",
            vec![failed_without_error(), flat_error(code, REFUSAL_TEXT)],
        ),
        (
            "failed without error, nested error",
            vec![failed_without_error(), nested_error(code, REFUSAL_TEXT)],
        ),
        ("flat error alone", vec![flat_error(code, REFUSAL_TEXT)]),
        ("nested error alone", vec![nested_error(code, REFUSAL_TEXT)]),
        (
            "failed with its error",
            vec![failed_with(code, REFUSAL_TEXT)],
        ),
    ];
    for wire in WIRES {
        for (shape, frames) in &shapes {
            let case = format!("{}: {shape}", wire.0);
            let error = terminal_error(*wire, sse(frames, false)).await;
            assert_terminal_refusal(&case, &error, code, REFUSAL_TEXT);
            assert_eq!(
                format!("{error:?}"),
                format!(
                    "{:?}",
                    LlmError::InvalidRequest {
                        message: format!("{code}: {REFUSAL_TEXT}"),
                    }
                ),
                "{case}"
            );
        }
    }
}

/// (c) Every code that names a request the service will never accept is the
/// terminal invalid-request class, as the cause after a `response.failed`
/// with a null error and inside a `response.failed`.
#[tokio::test]
async fn every_request_refusal_code_is_a_terminal_invalid_request() {
    for wire in WIRES {
        for code in REFUSAL_CODES {
            for (shape, frames) in [
                (
                    "flat error after a failure without one",
                    vec![failed_without_error(), flat_error(code, REFUSAL_TEXT)],
                ),
                (
                    "failed with its error",
                    vec![failed_with(code, REFUSAL_TEXT)],
                ),
            ] {
                let case = format!("{}: {code}: {shape}", wire.0);
                let error = terminal_error(*wire, sse(&frames, true)).await;
                assert_terminal_refusal(&case, &error, code, REFUSAL_TEXT);
            }
        }
    }
}

/// An error object that names no code is classified by its `type`: the
/// request-refusal type is the terminal invalid-request class too.
#[tokio::test]
async fn an_error_without_a_code_is_classified_by_its_type() {
    for wire in WIRES {
        let frames = [
            failed_without_error(),
            json!({
                "type": "error",
                "error": {"type": "invalid_request_error", "code": null, "message": REFUSAL_TEXT},
            }),
        ];
        let error = terminal_error(*wire, sse(&frames, false)).await;
        assert_terminal_refusal(wire.0, &error, "invalid_request_error", REFUSAL_TEXT);
    }
}

/// An exhausted plan usage limit is exhausted quota, not a failure to retry:
/// named as a code, or as the `type` of an error object without a code.
#[tokio::test]
async fn a_reached_usage_limit_is_exhausted_quota() {
    let text = "The usage limit has been reached";
    for wire in WIRES {
        for (shape, frames) in [
            (
                "flat code",
                vec![
                    failed_without_error(),
                    flat_error("usage_limit_reached", text),
                ],
            ),
            (
                "type of a nested error",
                vec![json!({
                    "type": "response.failed",
                    "response": {"error": {"type": "usage_limit_reached", "message": text}},
                })],
            ),
        ] {
            let case = format!("{}: {shape}", wire.0);
            let error = terminal_error(*wire, sse(&frames, false)).await;
            assert!(
                matches!(&error, LlmError::QuotaExhausted { message }
                    if message.contains("usage_limit_reached") && message.contains(text)),
                "{case}: exhausted quota with its cause, not {error:?}"
            );
            assert!(!error.is_retryable(), "{case}");
        }
    }
}

/// (d) A `response.failed` with a null error whose cause never arrives (the
/// body ends, or `[DONE]` ends the stream, with or without output before or
/// a completed response after) fails the response as an error that names
/// the missing cause. It stays retryable: nothing says the request itself
/// was refused.
#[tokio::test]
async fn a_failed_response_whose_cause_never_arrives_fails_retryable_and_says_so() {
    let delta = json!({
        "type": "response.output_text.delta",
        "item_id": "msg_1",
        "output_index": 0,
        "content_index": 0,
        "delta": "partial answer",
    });
    let cases: [(&str, Vec<Value>, bool); 4] = [
        ("the body ends", vec![failed_without_error()], false),
        ("[DONE] follows", vec![failed_without_error()], true),
        (
            "output before, [DONE] after",
            vec![delta, failed_without_error()],
            true,
        ),
        (
            "a completed response after",
            vec![failed_without_error(), completed()],
            true,
        ),
    ];
    for wire in WIRES {
        for (shape, frames, done) in &cases {
            let case = format!("{}: {shape}", wire.0);
            let error = terminal_error(*wire, sse(frames, *done)).await;
            let LlmError::ServerError {
                status: 500,
                message,
            } = &error
            else {
                panic!("{case}: a server error, not {error:?}");
            };
            assert!(
                message.contains("response failed") && message.contains("no cause"),
                "{case}: the error names the missing cause: {message}"
            );
            assert!(error.is_retryable(), "{case}: retryable: {error:?}");
        }
    }
}

/// What a stream sends between a `response.failed` with a null error and
/// its cause is not read as output: the response failed, and the cause
/// still decides.
#[tokio::test]
async fn events_between_a_failure_and_its_cause_are_not_output() {
    for wire in WIRES {
        let frames = [
            failed_without_error(),
            completed(),
            flat_error("invalid_prompt", REFUSAL_TEXT),
        ];
        let (events, requests) = events_of(wire.1, sse(&frames, true), false).await;
        assert_eq!(requests, 1, "{}", wire.0);
        assert!(
            !events.iter().any(|event| matches!(
                event,
                Ok(LlmEvent::TextDelta { .. } | LlmEvent::AssistantOutput { .. })
            )),
            "{}: no output of a failed response: {events:?}",
            wire.0
        );
        let error = terminal_error_of(wire.0, events);
        assert_terminal_refusal(wire.0, &error, "invalid_prompt", REFUSAL_TEXT);
    }
}

/// One class the stream mapping already had: its code, whether it is
/// retryable, and the test for its class.
type ExistingClass = (&'static str, bool, fn(&LlmError) -> bool);

/// The classes the stream mapping already had: a server error, a rate limit,
/// an overloaded server and a code this client does not know are retryable;
/// a context overflow, exhausted quota and a policy stop are not.
const EXISTING_CLASSES: &[ExistingClass] = &[
    (
        "server_error",
        true,
        |error| matches!(error, LlmError::ServerError { status: 500, message } if message == "try again"),
    ),
    ("rate_limit_exceeded", true, |error| {
        matches!(
            error,
            LlmError::RateLimited {
                retry_after_ms: None
            }
        )
    }),
    ("server_is_overloaded", true, |error| {
        matches!(error, LlmError::ServerOverloaded)
    }),
    ("a_code_of_a_later_release", true, |error| {
        matches!(error, LlmError::Unknown { message }
            if message == "a_code_of_a_later_release: try again")
    }),
    ("context_length_exceeded", false, |error| {
        matches!(error, LlmError::ContextLengthExceeded { .. })
    }),
    ("insufficient_quota", false, |error| {
        matches!(error, LlmError::QuotaExhausted { .. })
    }),
    ("misalignment_policy_violation", false, |error| {
        matches!(error, LlmError::PolicyStop { code, .. }
            if code == "misalignment_policy_violation")
    }),
];

/// Every existing class, read from `frames_of(code, message)`, has its class
/// and its retryability.
async fn assert_existing_classes(shape: &str, frames_of: fn(&str, &str) -> Vec<Value>) {
    for wire in WIRES {
        for (code, retryable, is_class) in EXISTING_CLASSES {
            let case = format!("{}: {code}: {shape}", wire.0);
            let error = terminal_error(*wire, sse(&frames_of(code, "try again"), false)).await;
            assert!(is_class(&error), "{case}: {error:?}");
            assert_eq!(error.is_retryable(), *retryable, "{case}: {error:?}");
        }
    }
}

/// (e) No regression: in the shapes the stream mapping already read (an
/// `error` event alone, flat or nested, and a `response.failed` with its
/// error) every class keeps its class and its retryability. A transient
/// failure stays retryable.
#[tokio::test]
async fn the_existing_stream_error_classes_are_unchanged() {
    assert_existing_classes("flat error alone", |code, message| {
        vec![flat_error(code, message)]
    })
    .await;
    assert_existing_classes("nested error alone", |code, message| {
        vec![nested_error(code, message)]
    })
    .await;
    assert_existing_classes("failed with its error", |code, message| {
        vec![failed_with(code, message)]
    })
    .await;
}

/// (e) The cause after a `response.failed` with a null error is classified
/// like any other: a transient cause there stays retryable, and every other
/// existing class keeps its class.
#[tokio::test]
async fn a_cause_after_a_failure_without_one_keeps_the_existing_classes() {
    assert_existing_classes("flat error after a failure without one", |code, message| {
        vec![failed_without_error(), flat_error(code, message)]
    })
    .await;
    assert_existing_classes(
        "nested error after a failure without one",
        |code, message| vec![failed_without_error(), nested_error(code, message)],
    )
    .await;
}

const AUTHORIZATION_CANARY: &str = "c4n4ryRefusalBearer";

/// Signs every request with `Authorization: Bearer <canary>`, as a dynamic
/// authorizer does, and removes that value from response diagnostics.
struct CanaryBearerAuthorizer;

#[async_trait::async_trait]
impl meerkat_core::HttpAuthorizer for CanaryBearerAuthorizer {
    async fn authorize(
        &self,
        request: &mut meerkat_core::HttpAuthorizationRequest<'_>,
    ) -> Result<(), meerkat_core::AuthError> {
        request.headers.push((
            "Authorization".to_string(),
            format!("Bearer {AUTHORIZATION_CANARY}"),
        ));
        Ok(())
    }

    fn redact_response_diagnostic(&self, diagnostic: &mut String) {
        *diagnostic = diagnostic.replace(AUTHORIZATION_CANARY, "<redacted>");
    }

    fn label(&self) -> &'static str {
        "canary-bearer-refusal-test"
    }
}

/// The cause is provider text, so it passes the authorizer's redaction like
/// every other error a stream ends with: a refusal that echoes the request's
/// credential reaches the caller with its code and its message and without
/// the credential.
#[tokio::test]
async fn a_refusal_cause_that_echoes_the_credential_is_redacted() {
    for wire in WIRES {
        let frames = [
            failed_without_error(),
            flat_error(
                "invalid_request_body",
                "refused the request signed {authorization}",
            ),
        ];
        let (events, requests) = events_of(
            |base_url| (wire.1)(base_url).with_authorizer(Arc::new(CanaryBearerAuthorizer)),
            sse(&frames, false),
            true,
        )
        .await;
        assert_eq!(requests, 1, "{}", wire.0);
        let error = terminal_error_of(wire.0, events);
        assert_terminal_refusal(
            wire.0,
            &error,
            "invalid_request_body",
            "refused the request signed Bearer <redacted>",
        );
        let rendered = format!(
            "{error} {error:?} {} {:?}",
            serde_json::to_string(&error).expect("error JSON"),
            error.failure_reason()
        );
        assert!(
            !rendered.contains(AUTHORIZATION_CANARY),
            "{}: the credential stays out of the error: {rendered}",
            wire.0
        );
    }
}

/// A `response.failed` whose error object names a `type` and no code.
fn failed_with_type(error_type: &str, message: &str) -> Value {
    json!({
        "type": "response.failed",
        "response": {"id": "resp_1", "status": "failed", "error": {"type": error_type, "message": message}},
    })
}

/// An `error` event whose error object names a `type` and no code.
fn error_with_type(error_type: &str, message: &str) -> Value {
    json!({"type": "error", "error": {"type": error_type, "message": message}})
}

/// A `type` stands in for a missing code only where the type names a class
/// by itself: the request-refusal type and the exhausted-quota codes, in a
/// `response.failed` and in an `error` event.
#[tokio::test]
async fn a_type_stands_in_for_a_missing_code_only_for_the_request_and_quota_types() {
    for wire in WIRES {
        for (shape, frame_of) in [
            (
                "response.failed",
                failed_with_type as fn(&str, &str) -> Value,
            ),
            ("error event", error_with_type),
        ] {
            let case = format!("{}: {shape}", wire.0);
            let error = terminal_error(
                *wire,
                sse(&[frame_of("invalid_request_error", REFUSAL_TEXT)], false),
            )
            .await;
            assert_terminal_refusal(&case, &error, "invalid_request_error", REFUSAL_TEXT);
            for quota in ["insufficient_quota", "usage_limit_reached"] {
                let error =
                    terminal_error(*wire, sse(&[frame_of(quota, "no quota left")], false)).await;
                assert!(
                    matches!(&error, LlmError::QuotaExhausted { message }
                        if message == &format!("{quota}: no quota left")),
                    "{case}: {quota}: {error:?}"
                );
            }
        }
    }
}

/// Any other `type` without a code decides nothing: the event keeps the
/// class it had before a type was read at all. A `response.failed` is the
/// retryable server error with the object's message, an `error` event the
/// retryable unknown class. In particular a type alone never raises a
/// policy stop, as over HTTP, and never a rate limit or a quota class.
#[tokio::test]
async fn any_other_type_without_a_code_keeps_the_default_class() {
    for wire in WIRES {
        for error_type in [
            "api_error",
            "a_type_of_a_later_release",
            "misalignment_policy_violation",
            "rate_limit_exceeded",
            "server_error",
            "context_length_exceeded",
        ] {
            let case = format!("{}: {error_type}", wire.0);
            let failed = terminal_error(
                *wire,
                sse(&[failed_with_type(error_type, "upstream said no")], false),
            )
            .await;
            assert!(
                matches!(&failed, LlmError::ServerError { status: 500, message }
                    if message == "upstream said no"),
                "{case}: response.failed: {failed:?}"
            );
            assert!(failed.is_retryable(), "{case}");
            let event = terminal_error(
                *wire,
                sse(&[error_with_type(error_type, "upstream said no")], false),
            )
            .await;
            assert!(
                matches!(&event, LlmError::Unknown { message }
                    if message == "unknown: upstream said no"),
                "{case}: error event: {event:?}"
            );
            assert!(event.is_retryable(), "{case}");
        }
    }
}

/// `model_not_found` is one class whatever carries it: the class an HTTP 404
/// with that code is, `ModelNotFound` with the service's message. Terminal.
#[tokio::test]
async fn model_not_found_in_the_stream_is_the_model_not_found_class() {
    let text = "The model does not exist or you do not have access to it.";
    for wire in WIRES {
        for (shape, frames) in [
            (
                "flat error after a failure without one",
                vec![failed_without_error(), flat_error("model_not_found", text)],
            ),
            (
                "nested error alone",
                vec![nested_error("model_not_found", text)],
            ),
            (
                "failed with its error",
                vec![failed_with("model_not_found", text)],
            ),
        ] {
            let case = format!("{}: {shape}", wire.0);
            let error = terminal_error(*wire, sse(&frames, false)).await;
            assert!(
                matches!(&error, LlmError::ModelNotFound { model } if model == text),
                "{case}: {error:?}"
            );
            assert!(!error.is_retryable(), "{case}");
            assert_eq!(
                error.failure_reason(),
                LlmFailureReason::InvalidModel(text.to_string()),
                "{case}"
            );
            assert_eq!(
                format!("{error:?}"),
                format!(
                    "{:?}",
                    LlmError::from_http_status(404, text.to_string(), None)
                ),
                "{case}: the class of an HTTP 404"
            );
        }
    }
}

/// A refused-request code whose message names another class keeps that
/// class, as an HTTP 400 with the same message does: the code in front of
/// the message never hides the message from the classifier.
#[tokio::test]
async fn a_refusal_whose_message_names_another_class_keeps_that_class() {
    let spend_limit = "You have reached your specified API usage limits. You will regain access on 2026-12-01 at 00:00 UTC.";
    for wire in WIRES {
        for frames in [
            vec![
                failed_without_error(),
                flat_error("invalid_request_error", spend_limit),
            ],
            vec![failed_with("invalid_request_error", spend_limit)],
        ] {
            let error = terminal_error(*wire, sse(&frames, false)).await;
            assert!(
                matches!(&error, LlmError::QuotaExhausted { .. }),
                "{}: a spend limit is exhausted quota: {error:?}",
                wire.0
            );
            assert_eq!(
                format!("{error:?}"),
                format!(
                    "{:?}",
                    LlmError::from_http_status(400, spend_limit.to_string(), None)
                ),
                "{}",
                wire.0
            );
        }
        let error = terminal_error(
            *wire,
            sse(
                &[flat_error(
                    "invalid_request_body",
                    "This model's maximum context length is 1000 tokens.",
                )],
                false,
            ),
        )
        .await;
        assert!(
            matches!(&error, LlmError::ContextLengthExceeded { .. }),
            "{}: a context overflow keeps its class: {error:?}",
            wire.0
        );
    }
}

/// How the cause is read where a stream gives it in an unusual place. Each
/// reading is the one the stream mapping had before, except that an error
/// given as a plain string is now kept as the message.
#[tokio::test]
async fn the_cause_is_read_from_every_place_the_same_way_as_before() {
    type Check = fn(&LlmError) -> bool;
    let cases: Vec<(&str, Vec<Value>, Check)> = vec![
        (
            "a later response.failed carries the cause",
            vec![failed_without_error(), failed_with("invalid_value", "bad")],
            |error| matches!(error, LlmError::InvalidRequest { message } if message == "invalid_value: bad"),
        ),
        (
            "response.failed: a message and no code is the server error",
            vec![json!({"type": "response.failed", "response": {"error": {"message": "broke"}}})],
            |error| matches!(error, LlmError::ServerError { status: 500, message } if message == "broke"),
        ),
        (
            "response.failed: a code and no message has the default message",
            vec![
                json!({"type": "response.failed", "response": {"error": {"code": "server_error"}}}),
            ],
            |error| matches!(error, LlmError::ServerError { status: 500, message } if message == "response failed"),
        ),
        (
            "response.failed: an error object with nothing in it is no cause",
            vec![
                json!({"type": "response.failed", "response": {"error": {"param": null}}}),
                flat_error("invalid_value", "the cause after it"),
            ],
            |error| matches!(error, LlmError::InvalidRequest { message } if message == "invalid_value: the cause after it"),
        ),
        (
            "response.failed: the error under status_details",
            vec![json!({
                "type": "response.failed",
                "response": {"status_details": {"error": {"code": "rate_limit_exceeded", "message": "slow"}}},
            })],
            |error| matches!(error, LlmError::RateLimited { .. }),
        ),
        (
            "response.failed: the message of status_details when the error object has none",
            vec![json!({
                "type": "response.failed",
                "response": {
                    "error": {"code": "server_error"},
                    "status_details": {"error": {"message": "from the details"}},
                },
            })],
            |error| matches!(error, LlmError::ServerError { status: 500, message } if message == "from the details"),
        ),
        (
            "response.failed: an error that is a string is the message",
            vec![json!({"type": "response.failed", "response": {"error": "upstream said no"}})],
            |error| matches!(error, LlmError::ServerError { status: 500, message } if message == "upstream said no"),
        ),
        (
            "error event: the nested code wins over the flat one",
            vec![json!({
                "type": "error",
                "code": "server_error",
                "message": "flat",
                "error": {"code": "rate_limit_exceeded", "message": "nested"},
            })],
            |error| matches!(error, LlmError::RateLimited { .. }),
        ),
        (
            "error event: a flat code wins over the object's type",
            vec![json!({
                "type": "error",
                "code": "rate_limit_exceeded",
                "error": {"type": "invalid_request_error", "message": "nested"},
            })],
            |error| matches!(error, LlmError::RateLimited { .. }),
        ),
        (
            "error event: an empty code is no code",
            vec![json!({"type": "error", "code": "", "message": "m"})],
            |error| matches!(error, LlmError::Unknown { message } if message == "unknown: m"),
        ),
        (
            "error event: an empty message is no message",
            vec![json!({"type": "error", "code": "server_error", "message": ""})],
            |error| matches!(error, LlmError::ServerError { status: 500, message } if message == "unknown streaming error"),
        ),
        (
            "error event: nothing in it is the unknown class",
            vec![failed_without_error(), json!({"type": "error"})],
            |error| matches!(error, LlmError::Unknown { message } if message == "unknown: unknown streaming error"),
        ),
    ];
    for wire in WIRES {
        for (case, frames, is_expected) in &cases {
            let error = terminal_error(*wire, sse(frames, false)).await;
            assert!(is_expected(&error), "{}: {case}: {error:?}", wire.0);
        }
    }
}

#[derive(Clone)]
struct Paced {
    chunks: Arc<Vec<Vec<u8>>>,
    /// Never end the body after the last chunk.
    hold_open: bool,
}

async fn paced_responses(State(paced): State<Paced>) -> impl IntoResponse {
    let body = async_stream::stream! {
        for chunk in paced.chunks.iter() {
            tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            yield Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(chunk.clone()));
        }
        if paced.hold_open {
            futures::future::pending::<()>().await;
        }
    };
    (
        [("content-type", "text/event-stream")],
        axum::body::Body::from_stream(body),
    )
}

/// Every event of one request whose answer arrives as `chunks`, one body
/// chunk at a time. With `hold_open` the body never ends, so the client has
/// to end the response by itself; a stream still open after five seconds
/// fails the row.
async fn paced_events_of(
    wire: Wire,
    chunks: Vec<Vec<u8>>,
    hold_open: bool,
) -> Vec<Result<LlmEvent, LlmError>> {
    let app = Router::new()
        .route("/v1/responses", post(paced_responses))
        .with_state(Paced {
            chunks: Arc::new(chunks),
            hold_open,
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind stub");
    let addr = listener.local_addr().expect("stub addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve stub");
    });
    let client = (wire.1)(format!("http://{addr}"));
    let request = LlmRequest::new(MODEL, vec![Message::User(UserMessage::text("hello"))]);
    let events = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.stream(&request).collect::<Vec<_>>(),
    )
    .await
    .unwrap_or_else(|_| panic!("{}: the client never ended the response", wire.0));
    server.abort();
    events
}

/// One frame as a body chunk of its own.
fn chunk(frame: &Value) -> Vec<u8> {
    format!("data: {frame}\n\n").into_bytes()
}

/// A frame the client skips while it waits for a cause.
fn skipped_frame(index: usize) -> Value {
    json!({
        "type": "response.output_text.delta",
        "item_id": "msg_1",
        "output_index": 0,
        "content_index": 0,
        "delta": format!("late {index}"),
    })
}

fn assert_no_cause(case: &str, error: &LlmError) {
    assert!(
        matches!(error, LlmError::ServerError { status: 500, message }
            if message.contains("response failed") && message.contains("no cause")),
        "{case}: the failure without a cause, not {error:?}"
    );
    assert!(error.is_retryable(), "{case}");
}

/// The wait for a cause is bounded by a frame count, not only by the end of
/// the stream: a body that stays open and keeps sending frames after a
/// `response.failed` without an error ends, after sixteen frames that are
/// not the cause, as the retryable failure without a cause. A cause within
/// those frames still decides.
#[tokio::test]
async fn the_wait_for_a_cause_ends_after_sixteen_other_frames() {
    for wire in WIRES {
        let mut open: Vec<Vec<u8>> = vec![chunk(&failed_without_error())];
        open.extend((0..16).map(|index| chunk(&skipped_frame(index))));
        let error = terminal_error_of(wire.0, paced_events_of(*wire, open, true).await);
        assert_no_cause(&format!("{}: sixteen frames, body open", wire.0), &error);

        let mut caused: Vec<Vec<u8>> = vec![chunk(&failed_without_error())];
        caused.extend((0..15).map(|index| chunk(&skipped_frame(index))));
        caused.push(chunk(&flat_error("invalid_value", REFUSAL_TEXT)));
        let error = terminal_error_of(wire.0, paced_events_of(*wire, caused, true).await);
        assert_terminal_refusal(
            &format!("{}: the cause after fifteen frames", wire.0),
            &error,
            "invalid_value",
            REFUSAL_TEXT,
        );
    }
}

/// While the client waits for a cause it reports no wire liveness, so a
/// body that only sends keepalive comments after a `response.failed` without
/// an error does not hold the stream-inactivity watchdog off.
#[tokio::test]
async fn no_wire_liveness_is_reported_while_waiting_for_a_cause() {
    for wire in WIRES {
        let chunks = vec![
            b": keepalive before\n\n".to_vec(),
            chunk(&failed_without_error()),
            b": keepalive\n\n".to_vec(),
            b": keepalive\n\n".to_vec(),
            chunk(&flat_error("invalid_value", REFUSAL_TEXT)),
        ];
        let events = paced_events_of(*wire, chunks, false).await;
        let liveness = events
            .iter()
            .filter(|event| matches!(event, Ok(LlmEvent::WireLiveness)))
            .count();
        assert_eq!(
            liveness, 1,
            "{}: liveness for the comment before the failure only: {events:?}",
            wire.0
        );
        let error = terminal_error_of(wire.0, events);
        assert_terminal_refusal(wire.0, &error, "invalid_value", REFUSAL_TEXT);
    }
}

/// A frame the client cannot read while it waits for a cause (data that is
/// not JSON, a line that is not UTF-8) changes nothing: the response failed
/// and no cause came, which is the retryable failure, not a terminal parse
/// error.
#[tokio::test]
async fn an_unreadable_frame_while_waiting_for_a_cause_is_the_failure_without_a_cause() {
    for wire in WIRES {
        for (shape, unreadable) in [
            ("data that is not JSON", b"data: {not json\n\n".to_vec()),
            ("a line that is not UTF-8", b"data: \xFF\xFE\n\n".to_vec()),
        ] {
            let chunks = vec![
                chunk(&failed_without_error()),
                unreadable,
                chunk(&skipped_frame(0)),
            ];
            let error = terminal_error_of(wire.0, paced_events_of(*wire, chunks, false).await);
            assert_no_cause(&format!("{}: {shape}", wire.0), &error);
        }
    }
}
