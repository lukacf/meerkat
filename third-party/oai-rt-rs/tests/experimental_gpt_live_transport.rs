#![cfg(feature = "experimental-gpt-live")]

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use futures::SinkExt;
use oai_rt_rs::experimental::gpt_live::{
    CallSession, ClientEvent, CreateCallRequest, Delegation, DelegationFunctionCallOutput,
    EventCarrier, ExtraFields, FunctionCallId, FunctionCallOutput, FunctionTool,
    GptLiveCredentials, GptLiveEndpoints, GptLiveTransport, InputTextContent,
    MAX_RAW_JSON_EVENT_BYTES, ResponsesConfig, ResponsesDelegation, ServerEvent, SessionAudio,
    SessionAudioOutput, SessionContextAppend, SidebandHeaders, TransportError,
    WebSocketFailureClass,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Capture {
    call_query: HashMap<String, String>,
    call_headers: HeaderMap,
    call_body: Option<Value>,
    sideband_call_id: Option<String>,
    sideband_headers: HeaderMap,
    client_event: Option<Value>,
    client_close_observed: bool,
}

type SharedCapture = Arc<Mutex<Capture>>;

fn assert_error_chain_redacted(error: &dyn Error, secrets: &[&str]) {
    let mut current = Some(error);
    while let Some(entry) = current {
        let diagnostics = format!("{entry}\n{entry:?}");
        for secret in secrets {
            assert!(
                !diagnostics.contains(secret),
                "error diagnostics leaked {secret}"
            );
        }
        current = entry.source();
    }
}

async fn create_call(
    State(capture): State<SharedCapture>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let authorization = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    {
        let mut capture = capture.lock().expect("capture lock");
        capture.call_query = query;
        capture.call_headers = headers;
        capture.call_body = serde_json::from_slice(&body).ok();
    }

    if authorization == "Bearer FIXTURE_HTTP_FAILURE_TOKEN" {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("content-type", "application/json")
            .body(Body::from(r#"{"error":"FIXTURE_PRIVATE_ERROR_BODY"}"#))
            .expect("failure response");
    }
    if authorization == "Bearer FIXTURE_MISSING_LOCATION_TOKEN" {
        return Response::builder()
            .status(StatusCode::CREATED)
            .header("content-type", "text/plain")
            .body(Body::from("v=0\r\nFIXTURE_PRIVATE_ANSWER_SDP"))
            .expect("missing location response");
    }

    let location = match authorization.as_str() {
        "Bearer FIXTURE_WEBSOCKET_FAILURE_TOKEN" => "/v1/realtime/calls/rtc_fixture_rejected",
        "Bearer FIXTURE_CLIENT_CLOSE_TOKEN" => "/v1/realtime/calls/rtc_fixture_client_close",
        "Bearer FIXTURE_OVERSIZED_EVENT_TOKEN" => "/v1/realtime/calls/rtc_fixture_oversized",
        _ => "/v1/realtime/calls/rtc_fixture_call",
    };
    Response::builder()
        .status(StatusCode::CREATED)
        .header("content-type", "text/plain; charset=utf-8")
        .header("location", location)
        .body(Body::from("v=0\r\nFIXTURE_PRIVATE_ANSWER_SDP"))
        .expect("success response")
}

async fn connect_sideband(
    Path(call_id): Path<String>,
    State(capture): State<SharedCapture>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response<Body> {
    {
        let mut capture = capture.lock().expect("capture lock");
        capture.sideband_call_id = Some(call_id.clone());
        capture.sideband_headers = headers;
    }
    if call_id == "rtc_fixture_rejected" {
        return Response::builder()
            .status(StatusCode::FORBIDDEN)
            .header("x-private-error", "FIXTURE_PRIVATE_WEBSOCKET_HEADER")
            .body(Body::from("FIXTURE_PRIVATE_WEBSOCKET_BODY"))
            .expect("rejected sideband response");
    }
    let wait_for_client_close = call_id == "rtc_fixture_client_close";
    let send_oversized_event = call_id == "rtc_fixture_oversized";
    upgrade
        .on_upgrade(move |socket| {
            serve_sideband(socket, capture, wait_for_client_close, send_oversized_event)
        })
        .into_response()
}

async fn serve_sideband(
    mut socket: WebSocket,
    capture: SharedCapture,
    wait_for_client_close: bool,
    send_oversized_event: bool,
) {
    socket
        .send(AxumMessage::Text(
            r#"{"type":"session.started","session":{"id":"rtc_fixture_call","expires_at":0,"status":"active"}}"#
                .into(),
        ))
        .await
        .expect("send session.started");
    if send_oversized_event {
        let _ = socket
            .send(AxumMessage::Text(
                "x".repeat(MAX_RAW_JSON_EVENT_BYTES + 1).into(),
            ))
            .await;
        return;
    }
    if wait_for_client_close {
        while let Some(message) = socket.recv().await {
            if matches!(message, Ok(AxumMessage::Close(_))) {
                capture.lock().expect("capture lock").client_close_observed = true;
                let _ = socket.close().await;
                return;
            }
        }
        return;
    }
    if let Some(Ok(AxumMessage::Text(text))) = socket.recv().await {
        capture.lock().expect("capture lock").client_event = serde_json::from_str(&text).ok();
    }
    let _ = socket.send(AxumMessage::Close(None)).await;
}

fn request() -> CreateCallRequest {
    CreateCallRequest {
        sdp: "v=0\r\nFIXTURE_PRIVATE_OFFER_SDP".to_owned(),
        session: CallSession {
            model: "gpt-live-1-codex".to_owned(),
            audio: SessionAudio {
                output: SessionAudioOutput {
                    voice: "cove".to_owned(),
                    extra: ExtraFields::new(),
                },
                extra: ExtraFields::new(),
            },
            delegation: None,
            instructions: None,
            extra: ExtraFields::new(),
        },
    }
}

fn responses_request() -> CreateCallRequest {
    let mut request = request();
    request.session.delegation = Some(Delegation::Responses(ResponsesDelegation::new(
        ResponsesConfig {
            model: "gpt-fixture-bridge".to_owned(),
            instructions: Some("FIXTURE_PRIVATE_BRIDGE_INSTRUCTIONS".to_owned()),
            tools: vec![FunctionTool::new(
                "invoke_meerkat",
                "Delegate to the channel-bound fixture agent.",
                json!({
                    "type": "object",
                    "properties": { "request": { "type": "string" } },
                    "required": ["request"],
                    "additionalProperties": false
                }),
                ExtraFields::new(),
            )],
            extra: ExtraFields::new(),
        },
        ExtraFields::new(),
    )));
    request
}

fn credentials(token: &str) -> GptLiveCredentials {
    GptLiveCredentials::new(
        token,
        SidebandHeaders {
            account_id: Some("FIXTURE_ACCOUNT_ID".to_owned()),
            attestation: Some("FIXTURE_ATTESTATION".to_owned()),
            originator: Some("codex_cli_rs".to_owned()),
            session_id: Some("FIXTURE_SESSION_ID".to_owned()),
            thread_id: Some("FIXTURE_THREAD_ID".to_owned()),
            version: Some("1.2.3".to_owned()),
            x_session_id: Some("FIXTURE_X_SESSION_ID".to_owned()),
            user_agent: Some("oai-rt-rs-fixture".to_owned()),
        },
    )
}

async fn local_transport() -> (GptLiveTransport, SharedCapture, tokio::task::JoinHandle<()>) {
    let capture = Arc::new(Mutex::new(Capture::default()));
    let app = Router::new()
        .route("/backend-api/codex/realtime/calls", post(create_call))
        .route("/v1/live/{call_id}", get(connect_sideband))
        .with_state(Arc::clone(&capture));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local test server");
    let address = listener.local_addr().expect("local address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve local test app");
    });
    let endpoints = GptLiveEndpoints::new(
        &format!(
            "http://{address}/backend-api/codex/realtime/calls?intent=quicksilver&architecture=avas"
        ),
        &format!("ws://{address}/v1/live/"),
    )
    .expect("local endpoints");
    (
        GptLiveTransport::with_endpoints(endpoints).expect("local transport"),
        capture,
        server,
    )
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn call_and_sideband_match_the_private_mechanical_contract() {
    let (transport, capture, server) = local_transport().await;
    let credentials = credentials("FIXTURE_BEARER_TOKEN");
    let created = transport
        .create_call(&request(), &credentials)
        .await
        .expect("create call");
    assert_eq!(created.answer_sdp, "v=0\r\nFIXTURE_PRIVATE_ANSWER_SDP");
    assert_eq!(created.call_id.as_str(), "rtc_fixture_call");

    {
        let capture = capture.lock().expect("capture lock");
        assert_eq!(
            capture.call_query.get("intent").map(String::as_str),
            Some("quicksilver")
        );
        assert_eq!(
            capture.call_query.get("architecture").map(String::as_str),
            Some("avas")
        );
        assert_eq!(
            capture.call_headers.get("authorization").unwrap(),
            "Bearer FIXTURE_BEARER_TOKEN"
        );
        assert_eq!(
            capture.call_headers.get("openai-alpha").unwrap(),
            "quicksilver=v2"
        );
        assert_eq!(
            capture.call_headers.get("content-type").unwrap(),
            "application/json"
        );
        assert_eq!(
            capture.call_body.as_ref(),
            Some(&json!({
                "sdp": "v=0\r\nFIXTURE_PRIVATE_OFFER_SDP",
                "session": {
                    "model": "gpt-live-1-codex",
                    "audio": { "output": { "voice": "cove" } }
                }
            }))
        );
        drop(capture);
    }

    let sideband = transport
        .connect_sideband(&created.call_id, &credentials)
        .await
        .expect("connect sideband");
    let (sender, mut receiver) = sideband.split();
    let observation = receiver
        .next_observation()
        .await
        .expect("receive sideband event")
        .expect("session.started");
    assert_eq!(observation.carrier(), EventCarrier::Sideband);
    assert!(observation.byte_count() > 0);
    assert!(matches!(
        observation.event(),
        ServerEvent::SessionStarted(_)
    ));
    let receive_close = tokio::spawn(async move { receiver.next_event().await });
    sender
        .send(&ClientEvent::DelegationFunctionCallOutput(
            DelegationFunctionCallOutput::new(FunctionCallOutput::new(
                FunctionCallId::new("call_fixture_bridge"),
                "FIXTURE_PRIVATE_FUNCTION_OUTPUT",
            )),
        ))
        .await
        .expect("send function output");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if capture.lock().expect("capture lock").client_event.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("server captured client event");

    {
        let capture = capture.lock().expect("capture lock");
        assert_eq!(
            capture.sideband_call_id.as_deref(),
            Some("rtc_fixture_call")
        );
        let expected_headers = [
            ("authorization", "Bearer FIXTURE_BEARER_TOKEN"),
            ("openai-alpha", "quicksilver=v2"),
            ("chatgpt-account-id", "FIXTURE_ACCOUNT_ID"),
            ("x-oai-attestation", "FIXTURE_ATTESTATION"),
            ("originator", "codex_cli_rs"),
            ("session-id", "FIXTURE_SESSION_ID"),
            ("thread-id", "FIXTURE_THREAD_ID"),
            ("version", "1.2.3"),
            ("x-session-id", "FIXTURE_X_SESSION_ID"),
            ("user-agent", "oai-rt-rs-fixture"),
        ];
        for (name, expected) in expected_headers {
            assert_eq!(capture.sideband_headers.get(name).unwrap(), expected);
        }
        assert_eq!(
            capture.client_event.as_ref(),
            Some(&json!({
                "type": "delegation.function_call_output.create",
                "item": {
                    "type": "function_call_output",
                    "call_id": "call_fixture_bridge",
                    "output": "FIXTURE_PRIVATE_FUNCTION_OUTPUT"
                }
            }))
        );
        drop(capture);
    }

    let _ = receive_close
        .await
        .expect("receive task")
        .expect("clean server close");
    server.abort();
}

#[tokio::test]
async fn responses_delegation_routes_through_call_creation() {
    let (transport, capture, server) = local_transport().await;
    transport
        .create_call(&responses_request(), &credentials("FIXTURE_BEARER_TOKEN"))
        .await
        .expect("create Responses-mode call");

    let capture = capture.lock().expect("capture lock");
    assert_eq!(
        capture
            .call_body
            .as_ref()
            .and_then(|body| body.pointer("/session/delegation/type")),
        Some(&json!("responses"))
    );
    assert_eq!(
        capture
            .call_body
            .as_ref()
            .and_then(|body| body.pointer("/session/delegation/responses/tools/0/type")),
        Some(&json!("function"))
    );
    assert_eq!(
        capture
            .call_body
            .as_ref()
            .and_then(|body| body.pointer("/session/delegation/responses/tools/0/name")),
        Some(&json!("invoke_meerkat"))
    );
    drop(capture);
    server.abort();
}

#[tokio::test]
async fn call_failures_are_typed_and_do_not_include_response_bodies() {
    let (transport, _capture, server) = local_transport().await;
    let error = transport
        .create_call(&request(), &credentials("FIXTURE_HTTP_FAILURE_TOKEN"))
        .await
        .expect_err("HTTP failure");
    assert!(matches!(
        error,
        TransportError::UnexpectedStatus(StatusCode::UNAUTHORIZED)
    ));
    assert!(!error.to_string().contains("FIXTURE_PRIVATE_ERROR_BODY"));

    let error = transport
        .create_call(&request(), &credentials("FIXTURE_MISSING_LOCATION_TOKEN"))
        .await
        .expect_err("missing Location");
    assert!(matches!(error, TransportError::MissingCallLocation));
    server.abort();
}

#[tokio::test]
async fn sideband_handshake_error_debug_redacts_private_response_material() {
    let (transport, _capture, server) = local_transport().await;
    let credentials = credentials("FIXTURE_WEBSOCKET_FAILURE_TOKEN");
    let created = transport
        .create_call(&request(), &credentials)
        .await
        .expect("create call");
    let error = transport
        .connect_sideband(&created.call_id, &credentials)
        .await
        .expect_err("sideband handshake must fail");
    assert!(matches!(error, TransportError::WebSocket(_)));
    let diagnostics = format!("{error:?}\n{error}");
    assert!(diagnostics.contains("WebSocket(Handshake)"));
    assert!(!diagnostics.contains("FIXTURE_PRIVATE_WEBSOCKET_HEADER"));
    assert!(!diagnostics.contains("FIXTURE_PRIVATE_WEBSOCKET_BODY"));
    assert_error_chain_redacted(
        &error,
        &[
            "FIXTURE_PRIVATE_WEBSOCKET_HEADER",
            "FIXTURE_PRIVATE_WEBSOCKET_BODY",
        ],
    );
    server.abort();
}

#[tokio::test]
async fn http_error_source_chain_does_not_retain_the_private_url() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind unused address");
    let address = listener.local_addr().expect("unused address");
    drop(listener);
    let secret = "FIXTURE_PRIVATE_HTTP_URL_SECRET";
    let endpoints = GptLiveEndpoints::new(
        &format!("http://{address}/{secret}?credential={secret}"),
        "ws://127.0.0.1:1/v1/live/",
    )
    .expect("test endpoints");
    let transport = GptLiveTransport::with_endpoints(endpoints).expect("transport");
    let error = transport
        .create_call(&request(), &credentials("FIXTURE_HTTP_CONNECT_TOKEN"))
        .await
        .expect_err("connection must fail");
    assert!(matches!(error, TransportError::Http(_)));
    assert_error_chain_redacted(&error, &[secret]);
}

#[tokio::test]
async fn call_session_extras_cannot_replace_delegation() {
    let (transport, capture, server) = local_transport().await;
    let mut request = request();
    request.session.extra.insert(
        "delegation".to_owned(),
        json!({ "type": "responses", "secret": "FIXTURE_PRIVATE_DELEGATION_COLLISION" }),
    );
    let error = transport
        .create_call(&request, &credentials("FIXTURE_BEARER_TOKEN"))
        .await
        .expect_err("reserved delegation collision");
    assert!(matches!(
        error,
        TransportError::Codec(
            oai_rt_rs::experimental::gpt_live::CodecError::ReservedExtraField {
                scope: "call session",
                field: "delegation",
            }
        )
    ));
    assert_error_chain_redacted(&error, &["FIXTURE_PRIVATE_DELEGATION_COLLISION"]);
    assert!(capture.lock().expect("capture lock").call_body.is_none());
    server.abort();
}

#[tokio::test]
async fn responses_tool_extras_cannot_replace_typed_fields() {
    let (transport, capture, server) = local_transport().await;
    let mut request = responses_request();
    let Some(Delegation::Responses(delegation)) = request.session.delegation.as_mut() else {
        panic!("Responses delegation fixture");
    };
    delegation.responses.tools[0].extra.insert(
        "type".to_owned(),
        json!("FIXTURE_PRIVATE_TOOL_TYPE_COLLISION"),
    );
    let error = transport
        .create_call(&request, &credentials("FIXTURE_BEARER_TOKEN"))
        .await
        .expect_err("reserved tool type collision");
    assert!(matches!(
        error,
        TransportError::Codec(
            oai_rt_rs::experimental::gpt_live::CodecError::ReservedExtraField {
                scope: "responses function tool",
                field: "type",
            }
        )
    ));
    assert_error_chain_redacted(&error, &["FIXTURE_PRIVATE_TOOL_TYPE_COLLISION"]);
    assert!(capture.lock().expect("capture lock").call_body.is_none());
    server.abort();
}

#[tokio::test]
async fn sideband_rejects_raw_messages_above_the_hard_bound() {
    let (transport, _capture, server) = local_transport().await;
    let credentials = credentials("FIXTURE_OVERSIZED_EVENT_TOKEN");
    let created = transport
        .create_call(&request(), &credentials)
        .await
        .expect("create oversized-event call");
    let sideband = transport
        .connect_sideband(&created.call_id, &credentials)
        .await
        .expect("connect sideband");
    let (_sender, mut receiver) = sideband.split();
    assert!(matches!(
        receiver.next_event().await.expect("session event"),
        Some(ServerEvent::SessionStarted(_))
    ));
    let error = receiver
        .next_event()
        .await
        .expect_err("oversized event must fail before decoding");
    assert!(matches!(
        error,
        TransportError::WebSocket(WebSocketFailureClass::Capacity)
    ));
    server.abort();
}

#[tokio::test]
async fn split_sender_close_completes_the_close_handshake() {
    let (transport, capture, server) = local_transport().await;
    let credentials = credentials("FIXTURE_CLIENT_CLOSE_TOKEN");
    let created = transport
        .create_call(&request(), &credentials)
        .await
        .expect("create call");
    let sideband = transport
        .connect_sideband(&created.call_id, &credentials)
        .await
        .expect("connect sideband");
    let (sender, mut receiver) = sideband.split();
    assert!(matches!(
        receiver.next_event().await.expect("session event"),
        Some(ServerEvent::SessionStarted(_))
    ));

    let receive_close = tokio::spawn(async move { receiver.next_event().await });
    sender.close().await.expect("send close frame");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if capture.lock().expect("capture lock").client_close_observed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("server observed close");
    let closed = tokio::time::timeout(Duration::from_secs(1), receive_close)
        .await
        .expect("receiver terminated")
        .expect("receive task")
        .expect("clean close event");
    assert!(closed.is_none());

    let error = sender
        .send(&ClientEvent::SessionContextAppend(SessionContextAppend {
            channel: None,
            content: vec![InputTextContent {
                content_type: "input_text".to_owned(),
                text: "FIXTURE_PRIVATE_POST_CLOSE_CONTEXT".to_owned(),
                extra: ExtraFields::new(),
            }],
            extra: ExtraFields::new(),
        }))
        .await
        .expect_err("post-close send must fail");
    assert!(matches!(error, TransportError::WebSocket(_)));
    server.abort();
}
