#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::disallowed_methods
)]
use super::*;
use crate::transport::sse::{
    ReqwestSseClient, SseClient, SseClientConfig, SseClientTransport, SseTransportError,
};
use crate::transport::streamable_http::{
    RequestDispatch, RequestDisposition, ReqwestStreamableHttpClient, SessionExpiryRecorder,
};
use rmcp::model::{CallToolRequest, CallToolRequestParams, NumberOrString};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const KEY: &str = "io.example/private";
const SECRET: &str = "transport-private-sentinel";
const LIMIT: Duration = Duration::from_secs(5);

fn metadata() -> Map<String, Value> {
    Map::from_iter([(KEY.into(), json!(SECRET))])
}
fn message(protected: bool) -> ClientJsonRpcMessage {
    let mut call = CallToolRequest::new(CallToolRequestParams::new("read"));
    if protected {
        call.extensions.insert(ProtectedMetadata(metadata()));
    }
    ClientJsonRpcMessage::request(call.into(), NumberOrString::Number(1))
}
fn response() -> Value {
    json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"ok"}]}})
}

#[test]
fn protected_http_client_explicitly_disables_protocol_nack_retries() {
    // Configuration pin only: reqwest does not expose a client's retry policy.
    // This does not claim to exercise HTTP/2 NACK behavior. Scope the assertion
    // to the actual builder so an unrelated call cannot satisfy it.
    let function = include_str!("protected.rs")
        .split_once("pub(crate) fn protected_http_client()")
        .unwrap()
        .1
        .split_once("\n}")
        .unwrap()
        .0;
    let compact: String = function.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.contains(".retry(reqwest::retry::never())"));
    assert_eq!(compact.matches(".retry(").count(), 1);
}

#[test]
fn metadata_is_opaque_until_final_wire_encoding() {
    let call = message(true);
    assert!(!format!("{call:?}").contains(SECRET));
    assert!(!serde_json::to_string(&call).unwrap().contains(SECRET));
    let mut wire = BytesMut::new();
    ProtectedOutputCodec.encode(call, &mut wire).unwrap();
    let decoded: Value = serde_json::from_slice(&wire).unwrap();
    assert_eq!(decoded["params"]["_meta"][KEY], SECRET);
    assert_eq!(decoded["params"]["name"], "read");
}

#[test]
fn scrub_only_protocol_metadata_and_preserve_application_values() {
    let state = ProtectedMetadataState::default();
    let mut value = response();
    value["result"]["_meta"] = json!({KEY:SECRET, "public":"kept"});
    value["result"]["content"][0]["_meta"] = json!({KEY:SECRET});
    value["result"]["content"].as_array_mut().unwrap().push(json!({
        "type":"resource", "_meta":{KEY:SECRET},
        "resource":{"uri":"test://document", "text":"{\"_meta\":{\"application\":true}}", "_meta":{KEY:SECRET}}
    }));
    value["result"]["structuredContent"] = json!({"nested":{"_meta":{KEY:"application-value", "io.meerkat/origin":"application-origin"}}});
    let bytes = serde_json::to_vec(&value).unwrap();
    let ordinary = serde_json::to_value(state.parse(&bytes).unwrap()).unwrap();
    assert_eq!(ordinary["result"]["_meta"][KEY], SECRET);
    state.register(&metadata()).unwrap();
    let scrubbed = state.parse(&bytes).unwrap();
    assert!(!format!("{scrubbed:?}").contains(SECRET));
    let scrubbed = serde_json::to_value(scrubbed).unwrap();
    assert_eq!(scrubbed["result"]["_meta"]["public"], "kept");
    assert_eq!(
        scrubbed["result"]["structuredContent"],
        value["result"]["structuredContent"]
    );
    let error = json!({"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"failure","data":{"_meta":{KEY:"application-value"}}}});
    let decoded =
        serde_json::to_value(state.parse(&serde_json::to_vec(&error).unwrap()).unwrap()).unwrap();
    assert_eq!(decoded["error"]["data"], error["error"]["data"]);
}

#[test]
fn fragmented_frames_scan_only_new_bytes_and_reject_unterminated_oversize() {
    let mut codec = ProtectedInputCodec {
        next_index: 0,
        state: Default::default(),
    };
    let mut value = response();
    value["result"]["content"][0]["text"] = json!("x".repeat(64 * 1024));
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut buffer = BytesMut::new();
    for fragment in bytes.chunks(31) {
        buffer.extend_from_slice(fragment);
        assert!(codec.decode(&mut buffer).unwrap().is_none());
        assert_eq!(codec.next_index, buffer.len());
    }
    buffer.extend_from_slice(b"\n");
    assert!(matches!(
        codec.decode(&mut buffer).unwrap(),
        Some(InputFrame::Message(_))
    ));
    assert_eq!(codec.next_index, 0);
    buffer.resize(MAX_FRAME_BYTES + 1, b'x');
    let error = codec.decode(&mut buffer).err().unwrap();
    assert_eq!(error.to_string(), "invalid or oversized MCP JSON-RPC frame");
}

#[tokio::test]
async fn stdio_ignores_optional_notifications_and_recovers_after_fixed_parse_error() {
    let (client, server) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(client);
    let (server_read, mut server_write) = tokio::io::split(server);
    let mut transport = ProtectedStdioTransport::new(read, write, Default::default());
    let frames = format!(
        "{{\"method\":\"optional/notification\",\"params\":{{\"payload\":\"{SECRET}\"}}}}\ninvalid-{SECRET}\n{}\n",
        response()
    );
    server_write.write_all(frames.as_bytes()).await.unwrap();
    let received = tokio::time::timeout(LIMIT, transport.receive())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(received).unwrap(), response());
    let mut returned = String::new();
    tokio::time::timeout(LIMIT, BufReader::new(server_read).read_line(&mut returned))
        .await
        .unwrap()
        .unwrap();
    let error: Value = serde_json::from_str(&returned).unwrap();
    assert_eq!(error["error"]["code"], -32700);
    assert_eq!(error["error"]["message"], "Parse error");
    assert!(!returned.contains(SECRET));
    transport.close().await.unwrap();
}

struct HttpFixture {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl HttpFixture {
    async fn start(app: axum::Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { url, task }
    }
}

#[tokio::test]
async fn outgoing_http_frame_bound_is_checked_before_sending() {
    let received = Arc::new(AtomicUsize::new(0));
    let count = received.clone();
    let server = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::post(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::ACCEPTED
            }
        }),
    ))
    .await;
    let http = ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), Default::default());
    let sse = ReqwestSseClient::new(Default::default());
    http.post_message(
        server.url.clone().into(),
        message(false),
        None,
        None,
        Default::default(),
    )
    .await
    .unwrap();
    sse.post_message(server.url.parse().unwrap(), message(false), None)
        .await
        .unwrap();
    assert_eq!(received.load(Ordering::SeqCst), 2);

    fn oversized_message(protected: bool) -> ClientJsonRpcMessage {
        let mut message = message(protected);
        let ClientJsonRpcMessage::Request(request) = &mut message else {
            panic!("expected request")
        };
        let ClientRequest::CallToolRequest(call) = &mut request.request else {
            panic!("expected tool call")
        };
        // The source string fits in the frame bound; JSON escaping doubles it.
        call.params.arguments = Some(Map::from_iter([(
            "payload".into(),
            Value::String("\\".repeat(MAX_FRAME_BYTES / 2)),
        )]));
        message
    }
    for protected in [false, true] {
        let error = http
            .post_message(
                server.url.clone().into(),
                oversized_message(protected),
                None,
                None,
                Default::default(),
            )
            .await
            .err()
            .unwrap();
        assert!(
            matches!(error, StreamableHttpError::UnexpectedServerResponse(message)
            if message == "invalid or oversized MCP JSON-RPC frame")
        );
        let error = sse
            .post_message(
                server.url.parse().unwrap(),
                oversized_message(protected),
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(matches!(error, SseTransportError::Io(error)
            if error.to_string() == "invalid or oversized MCP JSON-RPC frame"));
    }
    assert_eq!(received.load(Ordering::SeqCst), 2);
}

/// Length of each streamed oversized response body: three frame bounds, past
/// what loopback socket buffers hold once the client stops reading, so a
/// client that stops at the bound never lets the server finish.
const STREAMED_BODY_BYTES: usize = 3 * MAX_FRAME_BYTES;

/// Reads one request: its head and, when it names one, its body.
async fn read_request(stream: &mut tokio::net::TcpStream) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        request.extend_from_slice(&buffer[..read]);
        let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
        let body = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|length| length.trim().parse::<usize>().ok())
            .unwrap_or(0);
        if request.len() >= end + 4 + body {
            return;
        }
    }
}

/// Answers the first request on `listener` with a chunked `content_type`
/// body of [`STREAMED_BODY_BYTES`] made of `block` repeated, written while it
/// is generated: the fixture never holds the body. The task returns how many
/// body bytes the server handed over before its client stopped reading.
fn serve_streamed_body(
    listener: tokio::net::TcpListener,
    content_type: &'static str,
    block: Vec<u8>,
) -> tokio::task::JoinHandle<usize> {
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return 0;
        };
        read_request(&mut stream).await;
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\n\
             transfer-encoding: chunked\r\nconnection: close\r\n\r\n"
        );
        if stream.write_all(head.as_bytes()).await.is_err() {
            return 0;
        }
        let mut sent = 0;
        while sent < STREAMED_BODY_BYTES {
            let chunk = &block[..block.len().min(STREAMED_BODY_BYTES - sent)];
            let written = async {
                stream
                    .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .await?;
                stream.write_all(chunk).await?;
                stream.write_all(b"\r\n").await
            }
            .await;
            if written.is_err() {
                return sent;
            }
            sent += chunk.len();
        }
        let _ = stream.write_all(b"0\r\n\r\n").await;
        sent
    })
}

/// The body bytes the streaming server handed over, once it ended.
async fn body_bytes_sent(server: tokio::task::JoinHandle<usize>) -> usize {
    tokio::time::timeout(Duration::from_mins(2), server)
        .await
        .expect("the body server ends once its client stops reading")
        .unwrap()
}

/// The client read the body up to the frame bound and stopped there: it
/// never read, let alone held, the whole body.
fn assert_read_stopped_at_the_frame_bound(sent: usize) {
    assert!(
        sent < STREAMED_BODY_BYTES,
        "the client read the whole {STREAMED_BODY_BYTES}-byte body ({sent} bytes sent)"
    );
    assert!(
        sent >= MAX_FRAME_BYTES - 1024 * 1024,
        "the read stopped after {sent} bytes, before the {MAX_FRAME_BYTES}-byte frame bound"
    );
}

/// GUARD: a JSON response body past the frame bound is refused while it
/// streams: the read stops at the bound instead of buffering the body.
#[tokio::test]
async fn json_response_past_the_frame_bound_is_refused_at_the_bound() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    // Whitespace never completes a JSON-RPC message.
    let server = serve_streamed_body(listener, "application/json", vec![b' '; 64 * 1024]);
    let http = ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), Default::default());

    let error = http
        .post_message(url.into(), message(false), None, None, Default::default())
        .await
        .err()
        .unwrap();

    assert_read_stopped_at_the_frame_bound(body_bytes_sent(server).await);
    assert!(
        matches!(&error, StreamableHttpError::UnexpectedServerResponse(message)
            if message == "invalid or oversized MCP response"),
        "{error:?}"
    );
}

/// GUARD: an SSE event past the frame bound is refused while it streams:
/// the read stops at the bound instead of buffering the event.
#[tokio::test]
async fn sse_event_past_the_frame_bound_is_refused_at_the_bound() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    // `data:` lines of one event that never ends: no blank line follows.
    let mut line = b"data: ".to_vec();
    line.resize(64 * 1024 - 1, b'x');
    line.push(b'\n');
    let server = serve_streamed_body(listener, "text/event-stream", line);
    let http = ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), Default::default());

    let response = http
        .post_message(url.into(), message(false), None, None, Default::default())
        .await
        .unwrap();
    let StreamableHttpPostResponse::Sse(mut events, _) = response else {
        panic!("expected an SSE response")
    };
    let first = tokio::time::timeout(Duration::from_mins(2), events.next())
        .await
        .unwrap();
    drop(events);

    assert_read_stopped_at_the_frame_bound(body_bytes_sent(server).await);
    let Some(Err(sse_stream::Error::Body(error))) = first else {
        panic!("expected the frame bound refusal, got {first:?}")
    };
    let error = error.downcast::<std::io::Error>().unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "invalid or oversized MCP JSON-RPC frame");
}

#[tokio::test]
async fn bounded_http_encoding_preserves_json_and_content_type_precedence() {
    use http::header::CONTENT_TYPE;
    use http::{HeaderMap, HeaderValue};
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
    let server = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::post(move |headers: HeaderMap, body: axum::body::Bytes| {
            let sent = sent.clone();
            async move {
                sent.send((headers, body)).unwrap();
                axum::http::StatusCode::ACCEPTED
            }
        }),
    ))
    .await;

    async fn assert_request(
        received: &mut tokio::sync::mpsc::UnboundedReceiver<(HeaderMap, axum::body::Bytes)>,
        content_types: &[&str],
        protected: bool,
    ) {
        let (headers, body) = tokio::time::timeout(LIMIT, received.recv())
            .await
            .unwrap()
            .unwrap();
        let actual: Vec<_> = headers
            .get_all(CONTENT_TYPE)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect();
        assert_eq!(actual, content_types);
        let mut expected = serde_json::to_value(message(false)).unwrap();
        if protected {
            expected["params"]["_meta"] = json!({KEY: SECRET});
        }
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), expected);
    }

    for protected in [false, true] {
        for configured in [false, true] {
            let mut headers = HeaderMap::new();
            if configured {
                headers.insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/example"),
                );
            }
            let http =
                ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), headers.clone());
            http.post_message(
                server.url.clone().into(),
                message(protected),
                None,
                None,
                Default::default(),
            )
            .await
            .unwrap();
            assert_request(
                &mut received,
                &[if configured {
                    "application/example"
                } else {
                    "application/json"
                }],
                protected,
            )
            .await;

            let sse = ReqwestSseClient::new(headers);
            sse.post_message(server.url.parse().unwrap(), message(protected), None)
                .await
                .unwrap();
            let expected: &[&str] = if configured {
                &["application/json", "application/example"]
            } else {
                &["application/json"]
            };
            assert_request(&mut received, expected, protected).await;
        }

        let http =
            ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), Default::default());
        http.post_message(
            server.url.clone().into(),
            message(protected),
            None,
            None,
            std::collections::HashMap::from_iter([(
                CONTENT_TYPE,
                HeaderValue::from_static("application/custom"),
            )]),
        )
        .await
        .unwrap();
        assert_request(&mut received, &["application/custom"], protected).await;
    }
}

#[tokio::test]
async fn ordinary_sse_data_is_untouched_before_rmcp_parses_it() {
    let data = r#"  { "jsonrpc":"2.0", "id":1, "result":{"content":[]}, "future":[1e0] }  "#;
    let stream_body = format!(
        "event: message\nid: ordinary\nretry: 123\ndata: {data}\n\nevent: future-extension\ndata: not-json-rpc\n\n"
    );
    let server = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::get(move || {
            let body = stream_body.clone();
            async move { ([("content-type", "text/event-stream")], body) }
        }),
    ))
    .await;
    let response = reqwest::get(&server.url).await.unwrap();
    let events: Vec<_> = tokio::time::timeout(
        LIMIT,
        protected_sse_stream(response, ProtectedMetadataState::default(), false).collect(),
    )
    .await
    .unwrap();
    let events: Vec<_> = events.into_iter().collect::<Result<_, _>>().unwrap();
    assert_eq!(
        events,
        vec![
            sse_stream::Sse::default()
                .event("message")
                .id("ordinary")
                .retry(123)
                .data(data),
            sse_stream::Sse::default()
                .event("future-extension")
                .data("not-json-rpc"),
        ]
    );
}

#[tokio::test]
async fn sse_starts_scrubbing_on_the_same_stream_after_metadata_registration() {
    let mut value = response();
    value["result"]["_meta"] = json!({KEY:SECRET, "public":"kept"});
    value["result"]["content"][0]["_meta"] = json!({KEY:SECRET});
    let data = format!("  {value}  ");
    let stream_body = format!("data: {data}\n\ndata: {data}\n\n");
    let server = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::get(move || {
            let body = stream_body.clone();
            async move { ([("content-type", "text/event-stream")], body) }
        }),
    ))
    .await;
    let response = reqwest::get(&server.url).await.unwrap();
    let state = ProtectedMetadataState::default();
    let mut stream = protected_sse_stream(response, state.clone(), false);
    let ordinary = tokio::time::timeout(LIMIT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(ordinary.data.as_deref(), Some(data.as_str()));

    state.register(&metadata()).unwrap();
    let protected = tokio::time::timeout(LIMIT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let scrubbed = protected.data.unwrap();
    assert!(!scrubbed.contains(SECRET));
    let scrubbed: Value = serde_json::from_str(&scrubbed).unwrap();
    assert_eq!(scrubbed["result"]["_meta"]["public"], "kept");
    assert_eq!(scrubbed["result"]["content"][0]["text"], "ok");
    assert!(
        tokio::time::timeout(LIMIT, stream.next())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn sse_preserves_messages_across_comments_heartbeats_and_empty_event_types() {
    let mut data = response();
    data["result"]["_meta"] = json!({KEY:SECRET});
    let stream_body = format!(
        ": heartbeat\n\nevent: ping\ndata: keepalive\n\ndata:\n\nevent:\ndata: {data}\n\nevent: extension\ndata: {SECRET}\n\nevent: message\ndata: {data}\n\n"
    );
    let server = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::get(move || {
            let body = stream_body.clone();
            async move { ([("content-type", "text/event-stream")], body) }
        }),
    ))
    .await;
    let response = reqwest::get(&server.url).await.unwrap();
    let state = ProtectedMetadataState::default();
    state.register(&metadata()).unwrap();
    let events: Vec<_> = tokio::time::timeout(
        LIMIT,
        protected_sse_stream(response, state, false).collect(),
    )
    .await
    .unwrap();
    assert_eq!(events.len(), 2);
    for event in events {
        assert!(!event.unwrap().data.unwrap().contains(SECRET));
    }
}

#[tokio::test]
async fn session_expiry_is_typed_and_recorded_for_ordinary_and_protected_posts() {
    // The only transport is built with `reinit_on_expired_session(false)`, so
    // no 404 is ever re-initialized and re-sent. Each case uses a fresh
    // recorder, so each one really reaches the server and gets its own 404.
    let posts = Arc::new(AtomicUsize::new(0));
    let count = posts.clone();
    let server = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::post(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::NOT_FOUND
            }
        }),
    ))
    .await;
    for (case, protected) in [(0, false), (1, true), (2, false)] {
        let state = ProtectedMetadataState::default();
        if case > 0 {
            // Protected calls registered on this connection (case 2: an
            // unselected post on a connection that carries protected calls).
            state.register(&metadata()).unwrap();
        }
        let expiry = SessionExpiryRecorder::default();
        let client =
            ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), Default::default())
                .with_protected_metadata(state)
                .with_session_expiry(expiry.clone());
        let (message, dispatch) = dispatched(protected);
        let result = client
            .post_message(
                server.url.clone().into(),
                message,
                Some("session".into()),
                None,
                Default::default(),
            )
            .await;
        assert!(matches!(result, Err(StreamableHttpError::SessionExpired)));
        assert!(expiry.expired(), "case {case}");
        assert_eq!(
            dispatch.disposition(),
            Some(RequestDisposition::SentSessionExpired),
            "case {case}"
        );
        assert_eq!(posts.load(Ordering::SeqCst), case + 1, "exactly one POST");
    }
}

/// A call carrying its request-local disposition witness.
fn dispatched(protected: bool) -> (ClientJsonRpcMessage, RequestDispatch) {
    let dispatch = RequestDispatch::default();
    let mut message = message(protected);
    let ClientJsonRpcMessage::Request(request) = &mut message else {
        panic!("expected request")
    };
    let ClientRequest::CallToolRequest(call) = &mut request.request else {
        panic!("expected tool call")
    };
    call.extensions.insert(dispatch.clone());
    (message, dispatch)
}

/// A request queued before the expiry was recorded still never enters the
/// dead session: it is refused at transport entry, unsent.
#[tokio::test]
async fn known_expiry_refuses_a_queued_request_at_transport_entry() {
    let posts = Arc::new(AtomicUsize::new(0));
    let count = posts.clone();
    let server = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::post(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::NOT_FOUND
            }
        }),
    ))
    .await;
    let expiry = SessionExpiryRecorder::default();
    let client =
        ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), Default::default())
            .with_session_expiry(expiry.clone());
    let (first, first_dispatch) = dispatched(false);
    let (queued, queued_dispatch) = dispatched(false);
    let _ = client
        .post_message(
            server.url.clone().into(),
            first,
            Some("session".into()),
            None,
            Default::default(),
        )
        .await;
    assert_eq!(
        first_dispatch.disposition(),
        Some(RequestDisposition::SentSessionExpired)
    );
    let refused = client
        .post_message(
            server.url.clone().into(),
            queued,
            Some("session".into()),
            None,
            Default::default(),
        )
        .await;
    assert!(matches!(refused, Err(StreamableHttpError::SessionExpired)));
    assert_eq!(
        queued_dispatch.disposition(),
        Some(RequestDisposition::RefusedExpired)
    );
    assert_eq!(
        posts.load(Ordering::SeqCst),
        1,
        "the queued request was never sent"
    );
}

/// The attached-session GET stream expiring kills the session the same way:
/// a later call is refused unsent.
#[tokio::test]
async fn attached_get_404_records_expiry_and_later_calls_are_refused_unsent() {
    let posts = Arc::new(AtomicUsize::new(0));
    let count = posts.clone();
    let server = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::get(|| async { axum::http::StatusCode::NOT_FOUND }).post(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::ACCEPTED
            }
        }),
    ))
    .await;
    let expiry = SessionExpiryRecorder::default();
    let client =
        ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), Default::default())
            .with_session_expiry(expiry.clone());
    let stream = client
        .get_stream(
            server.url.clone().into(),
            "session".into(),
            None,
            None,
            Default::default(),
        )
        .await;
    assert!(matches!(stream, Err(StreamableHttpError::SessionExpired)));
    assert!(expiry.expired());
    let (call, dispatch) = dispatched(false);
    let refused = client
        .post_message(
            server.url.clone().into(),
            call,
            Some("session".into()),
            None,
            Default::default(),
        )
        .await;
    assert!(matches!(refused, Err(StreamableHttpError::SessionExpired)));
    assert_eq!(
        dispatch.disposition(),
        Some(RequestDisposition::RefusedExpired)
    );
    assert_eq!(posts.load(Ordering::SeqCst), 0);
}

/// A frame refused by the bound before sending leaves no disposition (never
/// sent), so its failure stays ordinary, never uncertain.
#[tokio::test]
async fn a_pre_send_failure_records_no_send() {
    let posts = Arc::new(AtomicUsize::new(0));
    let count = posts.clone();
    let server = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::post(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::NOT_FOUND
            }
        }),
    ))
    .await;
    let client =
        ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), Default::default());
    let (mut message, dispatch) = dispatched(false);
    let ClientJsonRpcMessage::Request(request) = &mut message else {
        panic!("expected request")
    };
    let ClientRequest::CallToolRequest(call) = &mut request.request else {
        panic!("expected tool call")
    };
    call.params.arguments = Some(Map::from_iter([(
        "payload".into(),
        Value::String("\\".repeat(MAX_FRAME_BYTES / 2)),
    )]));
    let result = client
        .post_message(
            server.url.clone().into(),
            message,
            Some("session".into()),
            None,
            Default::default(),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(dispatch.disposition(), None);
    assert_eq!(posts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn posts_do_not_follow_cross_origin_redirects_for_http_or_legacy_sse() {
    let forwarded = Arc::new(AtomicUsize::new(0));
    let count = forwarded.clone();
    let destination = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::post(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::ACCEPTED
            }
        }),
    ))
    .await;
    let location = destination.url.clone();
    let source = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::post(move || {
            let location = location.clone();
            async move {
                (
                    axum::http::StatusCode::TEMPORARY_REDIRECT,
                    [("location", location)],
                )
            }
        }),
    ))
    .await;
    let http = ReqwestStreamableHttpClient::new_with_auth_challenge(
        reqwest::header::HeaderMap::new(),
        Default::default(),
    );
    let sse = ReqwestSseClient::new(Default::default());
    // Protected and ordinary posts alike: a redirect to another origin is
    // refused, and nothing reaches the destination.
    for protected in [true, false] {
        assert!(
            http.post_message(
                source.url.clone().into(),
                message(protected),
                None,
                None,
                Default::default()
            )
            .await
            .is_err()
        );
        assert!(
            sse.post_message(source.url.parse().unwrap(), message(protected), None)
                .await
                .is_err()
        );
    }
    assert_eq!(forwarded.load(Ordering::SeqCst), 0);
}

/// A legacy SSE client whose stream names `endpoint` in its `endpoint` event,
/// and which records the URI of every message it is asked to post.
#[derive(Clone)]
struct EndpointClient {
    endpoint: Arc<str>,
    posts: Arc<std::sync::Mutex<Vec<String>>>,
}
impl SseClient for EndpointClient {
    type Error = io::Error;
    async fn post_message(
        &self,
        uri: http::Uri,
        _: ClientJsonRpcMessage,
        _: Option<String>,
    ) -> Result<(), SseTransportError<Self::Error>> {
        self.posts.lock().unwrap().push(uri.to_string());
        Ok(())
    }
    async fn get_stream(
        &self,
        _: http::Uri,
        _: Option<String>,
        _: Option<String>,
    ) -> Result<
        BoxStream<'static, Result<sse_stream::Sse, sse_stream::Error>>,
        SseTransportError<Self::Error>,
    > {
        let endpoint = self.endpoint.to_string();
        Ok(futures::stream::once(async move {
            Ok(sse_stream::Sse {
                event: Some("endpoint".into()),
                data: Some(endpoint),
                ..Default::default()
            })
        })
        .boxed())
    }
}

/// Start a legacy SSE transport configured for `source` whose stream names
/// `endpoint`, with the record of everything it posts.
async fn start_with_endpoint(
    source: &str,
    endpoint: &str,
) -> (
    Result<SseClientTransport<EndpointClient>, SseTransportError<io::Error>>,
    Arc<std::sync::Mutex<Vec<String>>>,
) {
    let posts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let started = SseClientTransport::start_with_client(
        EndpointClient {
            endpoint: endpoint.into(),
            posts: posts.clone(),
        },
        SseClientConfig {
            sse_endpoint: source.into(),
            use_message_endpoint: None,
        },
    )
    .await;
    (started, posts)
}

/// An `endpoint` event naming another host, port or scheme than the
/// configured endpoint is refused when the transport starts, so no message,
/// ordinary or protected, is ever posted there.
#[tokio::test]
async fn legacy_sse_refuses_a_message_endpoint_on_another_origin_before_any_post() {
    let source = "http://127.0.0.1:10001/events";
    let mut violations = Vec::new();
    for endpoint in [
        "http://localhost:10001/messages",
        "http://127.0.0.1:10002/messages",
        "https://127.0.0.1:10001/messages",
    ] {
        let (started, posts) = start_with_endpoint(source, endpoint).await;
        let refusal = match started {
            Ok(mut transport) => {
                for protected in [false, true] {
                    let _ = transport.send(message(protected)).await;
                }
                None
            }
            Err(error) => Some(error.to_string()),
        };
        let posted = posts.lock().unwrap().clone();
        if !posted.is_empty() {
            violations.push(format!("{endpoint}: posted {posted:?}"));
        }
        match refusal {
            None => violations.push(format!("{endpoint}: accepted as the message endpoint")),
            Some(refusal)
                if !refusal.contains("configured origin") || refusal.contains(endpoint) =>
            {
                violations.push(format!("{endpoint}: refused as {refusal:?}"));
            }
            Some(_) => {}
        }
    }
    assert!(violations.is_empty(), "{violations:#?}");
}

/// Same-origin endpoints still work, absolute (including an explicit default
/// port and a differently cased host), relative and query-only: ordinary and
/// protected messages are posted to the resolved endpoint.
#[tokio::test]
async fn legacy_sse_keeps_same_origin_message_endpoints() {
    for (source, endpoint, resolved) in [
        (
            "http://127.0.0.1:10001/events",
            "http://127.0.0.1:10001/messages?session=absolute",
            "http://127.0.0.1:10001/messages?session=absolute",
        ),
        (
            "http://mcp.example.test/events",
            "http://MCP.example.test:80/messages",
            "http://MCP.example.test:80/messages",
        ),
        (
            "http://127.0.0.1:10001/events",
            "/messages?session=relative",
            "http://127.0.0.1:10001/messages?session=relative",
        ),
        (
            "http://127.0.0.1:10001/events",
            "?session=query",
            "http://127.0.0.1:10001/events?session=query",
        ),
    ] {
        let (started, posts) = start_with_endpoint(source, endpoint).await;
        let Ok(mut transport) = started else {
            panic!("{endpoint} is on the origin of {source}");
        };
        for protected in [false, true] {
            transport.send(message(protected)).await.unwrap();
        }
        assert_eq!(*posts.lock().unwrap(), [resolved, resolved], "{endpoint}");
    }
}

/// The credential carried by the configured headers in the legacy SSE
/// connection controls.
const ORIGIN_MARKER: &str = "configured-origin-credential-marker";

/// What a fixture saw of one request: its method, the authority it was
/// addressed to (`Host`), its path and query, and whether any header carried
/// the credential marker.
#[derive(Clone, Debug)]
struct SeenRequest {
    method: String,
    authority: String,
    target: String,
    marker: bool,
}

/// A minimal legacy SSE MCP server. A GET opens the event stream, which first
/// names the endpoint built from this server's own authority and then carries
/// the answer to each POSTed request. Every request, to any path, is recorded.
struct LegacySseServer {
    fixture: HttpFixture,
    authority: String,
    seen: Arc<std::sync::Mutex<Vec<SeenRequest>>>,
}
impl LegacySseServer {
    async fn start(endpoint: impl FnOnce(&str) -> String) -> Self {
        use axum::response::IntoResponse;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let endpoint_event = format!("event: endpoint\ndata: {}\n\n", endpoint(&authority));
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (answers, unanswered) = tokio::sync::mpsc::unbounded_channel::<String>();
        let unanswered = Arc::new(std::sync::Mutex::new(Some(unanswered)));
        let record = seen.clone();
        let app = axum::Router::new().fallback(
            move |method: axum::http::Method,
                  uri: axum::http::Uri,
                  headers: axum::http::HeaderMap,
                  body: axum::body::Bytes| {
                let record = record.clone();
                let answers = answers.clone();
                let unanswered = unanswered.clone();
                let endpoint_event = endpoint_event.clone();
                async move {
                    record.lock().unwrap().push(SeenRequest {
                        method: method.to_string(),
                        authority: headers
                            .get("host")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        target: uri.to_string(),
                        marker: headers.values().any(|value| {
                            value
                                .to_str()
                                .is_ok_and(|value| value.contains(ORIGIN_MARKER))
                        }),
                    });
                    if method == axum::http::Method::GET {
                        let Some(pending) = unanswered.lock().unwrap().take() else {
                            return axum::http::StatusCode::CONFLICT.into_response();
                        };
                        let messages = futures::stream::unfold(pending, |mut pending| async move {
                            let answer = pending.recv().await?;
                            Some((format!("event: message\ndata: {answer}\n\n"), pending))
                        });
                        let events = futures::stream::once(async move { endpoint_event })
                            .chain(messages)
                            .map(Ok::<_, std::convert::Infallible>);
                        return (
                            [("content-type", "text/event-stream")],
                            axum::body::Body::from_stream(events),
                        )
                            .into_response();
                    }
                    if let Ok(request) = serde_json::from_slice::<Value>(&body)
                        && let Some(id) = request.get("id")
                    {
                        let result = match request["method"].as_str() {
                            Some("initialize") => json!({
                                "protocolVersion": request["params"]["protocolVersion"],
                                "capabilities": {"tools": {}},
                                "serverInfo": {"name": "legacy-sse-origin", "version": "0.0.0"},
                            }),
                            Some("tools/list") => json!({"tools": [{
                                "name": "probe",
                                "inputSchema": {"type": "object", "properties": {}},
                            }]}),
                            _ => json!({}),
                        };
                        let answer = json!({"jsonrpc": "2.0", "id": id, "result": result});
                        let _ = answers.send(answer.to_string());
                    }
                    axum::http::StatusCode::ACCEPTED.into_response()
                }
            },
        );
        let url = format!("http://{authority}/mcp");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            fixture: HttpFixture { url, task },
            authority,
            seen,
        }
    }

    fn url(&self) -> &str {
        &self.fixture.url
    }

    fn seen(&self) -> Vec<SeenRequest> {
        self.seen.lock().unwrap().clone()
    }
}

/// A TCP endpoint that counts the connections reaching it and closes each one
/// at once, so no HTTP or TLS exchange can complete there.
struct CountingListener {
    authority: String,
    connections: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for CountingListener {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl CountingListener {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let connections = Arc::new(AtomicUsize::new(0));
        let count = connections.clone();
        let task = tokio::spawn(async move {
            while let Ok((connection, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                drop(connection);
            }
        });
        Self {
            authority,
            connections,
            task,
        }
    }
}

type Connected = Result<(crate::McpConnection, Vec<Arc<meerkat_core::ToolDef>>), crate::McpError>;

/// Connect, initialize and list tools through the production legacy SSE
/// connection, with the credential marker in the configured headers.
async fn connect_legacy_sse(url: &str) -> Connected {
    let headers = std::collections::HashMap::from([(
        "authorization".to_string(),
        format!("Bearer {ORIGIN_MARKER}"),
    )]);
    let mut config = meerkat_core::McpServerConfig::sse("origin-probe", url, headers);
    config.connect_timeout_secs = Some(5);
    crate::McpConnection::connect_and_enumerate(&config).await
}

/// Why `connected` is not the expected refusal, if it is not. The refusal is
/// local connection feedback naming the origin rule, not the endpoint the
/// server supplied.
fn origin_refusal_violation(connected: Connected, other: &str) -> Option<String> {
    let Err(error) = connected else {
        return Some(format!("an endpoint on {other} was accepted"));
    };
    match &error {
        crate::McpError::ConnectionFailed { reason }
            if reason.contains("configured origin") && !reason.contains(other) =>
        {
            None
        }
        _ => Some(format!("unexpected refusal {error:?}")),
    }
}

/// The production legacy SSE connection refuses an `endpoint` event naming
/// another host, port or scheme before initialize: nothing is sent there, and
/// the configured credential never reaches it. Each case is checked in full
/// before the violations are reported.
#[tokio::test]
async fn legacy_sse_connection_refuses_another_origin_without_reaching_it() {
    let mut violations = Vec::new();

    // Another host: the configured URL names `localhost`, the endpoint the
    // same port by address.
    let source =
        LegacySseServer::start(|authority| format!("http://{authority}/mcp/messages")).await;
    let port = source.authority.rsplit_once(':').unwrap().1.to_string();
    let connected = connect_legacy_sse(&format!("http://localhost:{port}/mcp")).await;
    let sent: Vec<_> = source
        .seen()
        .into_iter()
        .filter(|seen| seen.method != "GET")
        .collect();
    if !sent.is_empty() {
        violations.push(format!(
            "another host: reached {}: {sent:?}",
            source.authority
        ));
    }
    violations.extend(
        origin_refusal_violation(connected, &source.authority)
            .map(|violation| format!("another host: {violation}")),
    );

    // Another port: the endpoint names a second server.
    let destination =
        LegacySseServer::start(|authority| format!("http://{authority}/mcp/messages")).await;
    let other = destination.authority.clone();
    let source = LegacySseServer::start(move |_| format!("http://{other}/mcp/messages")).await;
    let connected = connect_legacy_sse(source.url()).await;
    let reached = destination.seen();
    if !reached.is_empty() {
        violations.push(format!(
            "another port: reached {}: {reached:?}",
            destination.authority
        ));
    }
    violations.extend(
        origin_refusal_violation(connected, &destination.authority)
            .map(|violation| format!("another port: {violation}")),
    );

    // Another scheme: the endpoint names `https` on a listener that counts
    // every connection.
    let listener = CountingListener::start().await;
    let other = listener.authority.clone();
    let source = LegacySseServer::start(move |_| format!("https://{other}/mcp/messages")).await;
    let connected = connect_legacy_sse(source.url()).await;
    let connections = listener.connections.load(Ordering::SeqCst);
    if connections != 0 {
        violations.push(format!(
            "another scheme: {connections} connection(s) reached {}",
            listener.authority
        ));
    }
    violations.extend(
        origin_refusal_violation(connected, &listener.authority)
            .map(|violation| format!("another scheme: {violation}")),
    );
    let sent: Vec<_> = source
        .seen()
        .into_iter()
        .filter(|seen| seen.method != "GET")
        .collect();
    if !sent.is_empty() {
        violations.push(format!("another scheme: the source received {sent:?}"));
    }

    assert!(violations.is_empty(), "{violations:#?}");
}

/// The same connection still initializes and lists tools through a
/// same-origin endpoint, absolute, relative or query-only, and posts there
/// with the configured headers.
#[tokio::test]
async fn legacy_sse_connection_initializes_and_lists_on_same_origin_endpoints() {
    let cases: [(fn(&str) -> String, &str); 3] = [
        (
            |authority: &str| format!("http://{authority}/mcp/messages?session=absolute"),
            "/mcp/messages?session=absolute",
        ),
        (
            |_: &str| "/mcp/messages?session=relative".to_string(),
            "/mcp/messages?session=relative",
        ),
        (|_: &str| "?session=query".to_string(), "/mcp?session=query"),
    ];
    for (endpoint, target) in cases {
        let server = LegacySseServer::start(endpoint).await;
        let (_connection, tools) = connect_legacy_sse(server.url()).await.unwrap();
        let names: Vec<_> = tools.iter().map(|tool| tool.name.to_string()).collect();
        assert_eq!(names, ["probe"], "{target}");
        let posts: Vec<_> = server
            .seen()
            .into_iter()
            .filter(|seen| seen.method == "POST")
            .collect();
        assert!(!posts.is_empty(), "{target}");
        assert!(
            posts.iter().all(|seen| seen.target == target
                && seen.authority == server.authority
                && seen.marker),
            "{target}: {posts:?}"
        );
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_rmcp_stdio_trace_keeps_live_metadata_and_malformed_input_private() {
    use futures::FutureExt;
    use rmcp::{
        ServiceExt,
        model::{ServerCapabilities, ServerInfo},
    };
    use tokio::io::AsyncReadExt;

    const CHILD_ENV: &str = "MEERKAT_MCP_TRACE_CAPTURE_CHILD";
    const CHILD_COMPLETE: &str = "meerkat-mcp-trace-capture-complete";
    if std::env::var_os(CHILD_ENV).as_deref() != Some(std::ffi::OsStr::new("1")) {
        // rmcp spawns its own workers. Capture them with one global subscriber
        // in a separate process, without capturing unrelated tests' canaries.
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "transport::protected::tests::real_rmcp_stdio_trace_keeps_live_metadata_and_malformed_input_private",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let status = match tokio::time::timeout(LIMIT * 4, child.wait()).await {
            Ok(status) => status.unwrap(),
            Err(_) => {
                child.kill().await.unwrap();
                panic!("isolated MCP TRACE fixture timed out");
            }
        };
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .take(64 * 1024)
            .read_to_string(&mut output)
            .await
            .unwrap();
        assert!(
            status.success(),
            "isolated MCP TRACE fixture failed: {output}"
        );
        // An exact-name mismatch must not pass by running zero tests.
        assert!(output.contains(CHILD_COMPLETE));
        return;
    }

    let captured = Capture::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let (client, server) = tokio::io::duplex(16384);
    let (read, write) = tokio::io::split(client);
    let (server_read, mut server_write) = tokio::io::split(server);
    let state = ProtectedMetadataState::default();
    state.register(&metadata()).unwrap();
    let transport = ProtectedStdioTransport::new(read, write, state);
    let peer = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        let initialize: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        let info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        let response = json!({"jsonrpc":"2.0","id":initialize["id"],"result":info});
        server_write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
        let initialized: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(initialized["method"], "notifications/initialized");
        let call: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(call["params"]["_meta"][KEY], SECRET);
        let response = json!({"jsonrpc":"2.0","id":call["id"],"result":{"_meta":{KEY:SECRET},"content":[{"type":"text","text":"safe-result","_meta":{KEY:SECRET}}]}});
        server_write
            .write_all(format!("invalid-{SECRET}\n{response}\n").as_bytes())
            .await
            .unwrap();
        let parse_error: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(parse_error["error"]["code"], -32700);
    });
    let service = tokio::time::timeout(LIMIT, ().serve(transport))
        .await
        .unwrap()
        .unwrap();
    let result = std::panic::AssertUnwindSafe(async {
        tracing::info!("ordinary-diagnostic-survives");
        let mut request = CallToolRequest::new(CallToolRequestParams::new("read"));
        request.extensions.insert(ProtectedMetadata(metadata()));
        let result = tokio::time::timeout(LIMIT, service.send_request(request.into()))
            .await
            .unwrap()
            .unwrap();
        assert!(!format!("{result:?}").contains(SECRET));
    })
    .catch_unwind()
    .await;
    service.cancel().await.unwrap();
    let joined = tokio::time::timeout(LIMIT, peer).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    joined.unwrap().unwrap();
    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("ordinary-diagnostic-survives"));
    assert!(logs.lines().any(|line| {
        line.contains("TRACE")
            && line.contains("rmcp::service")
            && line.contains("new event")
            && line.contains("safe-result")
    }));
    assert!(logs.contains("MCP input refused: invalid JSON-RPC frame"));
    assert!(!logs.contains(SECRET));
    println!("{CHILD_COMPLETE}");
}

/// A target that counts the requests reaching it, and an origin that
/// answers every request with a redirect to it.
async fn cross_origin_redirect_fixture() -> (HttpFixture, HttpFixture, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let target = HttpFixture::start(axum::Router::new().fallback(move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            axum::http::StatusCode::ACCEPTED
        }
    }))
    .await;
    let location = format!("{}?leak=redirect-location-canary", target.url);
    let origin = HttpFixture::start(axum::Router::new().fallback(move || {
        let location = location.clone();
        async move {
            (
                axum::http::StatusCode::TEMPORARY_REDIRECT,
                [("location", location)],
                "redirect-body-canary",
            )
        }
    }))
    .await;
    (origin, target, hits)
}

/// The production Streamable HTTP client refuses a redirect to another
/// origin (the configured headers would follow it) without reaching the
/// target or rendering the `Location`.
#[tokio::test]
async fn streamable_http_refuses_a_cross_origin_redirect() {
    let (origin, _target, hits) = cross_origin_redirect_fixture().await;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-api-key", "configured-header-canary".parse().unwrap());
    let client = ReqwestStreamableHttpClient::new_with_auth_challenge(headers, Default::default());
    let error = client
        .post_message(
            origin.url.clone().into(),
            message(false),
            None,
            None,
            Default::default(),
        )
        .await
        .expect_err("a cross-origin redirect is refused");
    let rendered = format!("{error} {error:?}");
    assert!(rendered.contains("redirect"), "{rendered}");
    assert!(!rendered.contains("redirect-location-canary"), "{rendered}");
    assert!(!rendered.contains("redirect-body-canary"), "{rendered}");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

/// The production legacy SSE client refuses the same way.
#[tokio::test]
async fn sse_refuses_a_cross_origin_redirect() {
    let (origin, _target, hits) = cross_origin_redirect_fixture().await;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-api-key", "configured-header-canary".parse().unwrap());
    let client = ReqwestSseClient::new(headers);
    let error = client
        .post_message(origin.url.parse().unwrap(), message(false), None)
        .await
        .expect_err("a cross-origin redirect is refused");
    let rendered = format!("{error} {error:?}");
    assert!(rendered.contains("redirect"), "{rendered}");
    assert!(!rendered.contains("redirect-location-canary"), "{rendered}");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

/// A same-origin `307` (the Starlette `/mcp` to `/mcp/` case) is followed
/// with its method and body, by the production client.
#[tokio::test]
async fn streamable_http_follows_a_same_origin_trailing_slash_redirect() {
    let bodies = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = bodies.clone();
    let server = HttpFixture::start(
        axum::Router::new()
            .route(
                "/mcp",
                axum::routing::post(|| async {
                    (
                        axum::http::StatusCode::TEMPORARY_REDIRECT,
                        [("location", "/mcp/")],
                    )
                }),
            )
            .route(
                "/mcp/",
                axum::routing::post(move |body: String| {
                    let seen = seen.clone();
                    async move {
                        seen.lock().unwrap().push(body);
                        axum::http::StatusCode::ACCEPTED
                    }
                }),
            ),
    )
    .await;
    let client = ReqwestStreamableHttpClient::new_with_auth_challenge(
        reqwest::header::HeaderMap::new(),
        Default::default(),
    );
    let accepted = client
        .post_message(
            server.url.clone().into(),
            message(false),
            None,
            None,
            Default::default(),
        )
        .await;
    assert!(
        matches!(
            accepted,
            Ok(rmcp::transport::streamable_http_client::StreamableHttpPostResponse::Accepted)
        ),
        "{accepted:?}"
    );
    let bodies = bodies.lock().unwrap();
    assert_eq!(
        bodies.len(),
        1,
        "the redirect target received the request once"
    );
    assert!(bodies[0].contains("tools/call"), "{}", bodies[0]);
}

/// A same-origin route: `/mcp` answers 307 to `/mcp/`, which answers with
/// `final_status`. Counts the physical POSTs on each path separately.
async fn same_origin_redirect_fixture(
    final_status: axum::http::StatusCode,
) -> (HttpFixture, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let routed = Arc::new(AtomicUsize::new(0));
    let handled = Arc::new(AtomicUsize::new(0));
    let (routed_count, handled_count) = (routed.clone(), handled.clone());
    let server = HttpFixture::start(
        axum::Router::new()
            .route(
                "/mcp",
                axum::routing::post(move || {
                    let routed = routed_count.clone();
                    async move {
                        routed.fetch_add(1, Ordering::SeqCst);
                        (
                            axum::http::StatusCode::TEMPORARY_REDIRECT,
                            [("location", "/mcp/")],
                        )
                    }
                }),
            )
            .route(
                "/mcp/",
                axum::routing::post(move || {
                    let handled = handled_count.clone();
                    async move {
                        handled.fetch_add(1, Ordering::SeqCst);
                        final_status
                    }
                }),
            ),
    )
    .await;
    (server, routed, handled)
}

/// One routed call is two physical POSTs; when it then fails, its own
/// disposition says it was redirected (typed uncertainty), and nothing is
/// re-sent.
#[tokio::test]
async fn a_redirected_call_that_fails_records_its_redirect() {
    let (server, routed, handled) =
        same_origin_redirect_fixture(axum::http::StatusCode::INTERNAL_SERVER_ERROR).await;
    let client = ReqwestStreamableHttpClient::new_with_auth_challenge(
        reqwest::header::HeaderMap::new(),
        Default::default(),
    );
    let (call, dispatch) = dispatched(false);
    let result = client
        .post_message(
            server.url.clone().into(),
            call,
            None,
            None,
            Default::default(),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(
        dispatch.disposition(),
        Some(RequestDisposition::SentRedirected)
    );
    assert_eq!(
        routed.load(Ordering::SeqCst),
        1,
        "one physical POST to the route"
    );
    assert_eq!(
        handled.load(Ordering::SeqCst),
        1,
        "one physical POST to the handler"
    );
}

/// Controls: a protected call is never redirected (stopped at the first
/// 3xx, still recorded as redirected, the handler never reached); an
/// independent call that fails without a redirect stays plainly `Sent`.
#[tokio::test]
async fn redirect_disposition_controls() {
    let (server, routed, handled) =
        same_origin_redirect_fixture(axum::http::StatusCode::ACCEPTED).await;
    let state = ProtectedMetadataState::default();
    state.register(&metadata()).unwrap();
    let client = ReqwestStreamableHttpClient::new_with_auth_challenge(
        reqwest::header::HeaderMap::new(),
        Default::default(),
    )
    .with_protected_metadata(state);
    let (call, dispatch) = dispatched(true);
    let refused = client
        .post_message(
            server.url.clone().into(),
            call,
            None,
            None,
            Default::default(),
        )
        .await;
    assert!(refused.is_err());
    assert_eq!(
        dispatch.disposition(),
        Some(RequestDisposition::SentRedirected)
    );
    assert_eq!(routed.load(Ordering::SeqCst), 1);
    assert_eq!(handled.load(Ordering::SeqCst), 0);

    let failing = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::post(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
    ))
    .await;
    let (call, dispatch) = dispatched(false);
    let failed = client
        .post_message(
            failing.url.clone().into(),
            call,
            None,
            None,
            Default::default(),
        )
        .await;
    assert!(failed.is_err());
    assert_eq!(dispatch.disposition(), Some(RequestDisposition::Sent));
}

/// A stdio writer that accepts no bytes until it is opened.
#[derive(Clone, Default)]
struct HeldWriter(Arc<std::sync::Mutex<HeldState>>);

#[derive(Default)]
struct HeldState {
    open: bool,
    waker: Option<std::task::Waker>,
    written: Vec<u8>,
}

impl HeldWriter {
    fn open(&self) {
        let mut state = self.0.lock().unwrap();
        state.open = true;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    fn written(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().written.clone()).unwrap()
    }

    fn poll_open(&self, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        let mut state = self.0.lock().unwrap();
        if state.open {
            std::task::Poll::Ready(())
        } else {
            state.waker = Some(cx.waker().clone());
            std::task::Poll::Pending
        }
    }
}

impl AsyncWrite for HeldWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::task::ready!(self.poll_open(cx));
        self.0.lock().unwrap().written.extend_from_slice(buf);
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        self.poll_open(cx).map(Ok)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

struct Counted(Arc<AtomicUsize>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn held_call(dropped: &Arc<AtomicUsize>, id: i64) -> ClientJsonRpcMessage {
    let mut call = CallToolRequest::new(CallToolRequestParams::new("read"));
    call.extensions
        .insert(SendCustody::new(Counted(Arc::clone(dropped))));
    ClientJsonRpcMessage::request(call.into(), NumberOrString::Number(id))
}

/// A send whose sender is cancelled while the stdio writer holds its bytes
/// keeps the request's custody with the writer: it is released only when a
/// later flush writes those bytes, or when the writer is retired. (Fails-old:
/// the codec consumed the request with its custody before the write.)
#[tokio::test]
async fn a_held_stdio_writer_keeps_send_custody_after_its_sender_is_cancelled() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let writer = HeldWriter::default();
    let (read, _peer) = tokio::io::duplex(64);
    let mut transport =
        ProtectedStdioTransport::new(read, writer.clone(), ProtectedMetadataState::default());

    let cancelled = tokio::time::timeout(
        Duration::from_millis(100),
        transport.send(held_call(&dropped, 1)),
    )
    .await;
    assert!(cancelled.is_err(), "the held writer kept the send pending");
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        0,
        "the cancelled send's custody stays with the held writer"
    );

    writer.open();
    transport.send(message(false)).await.unwrap();
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        1,
        "the flush that wrote the held bytes released their custody"
    );
    assert!(writer.written().contains(r#""id":1"#));

    // Retirement releases the custody of bytes that were never flushed.
    let retired = Arc::new(AtomicUsize::new(0));
    let held = HeldWriter::default();
    let (read, _peer) = tokio::io::duplex(64);
    let mut transport =
        ProtectedStdioTransport::new(read, held.clone(), ProtectedMetadataState::default());
    let cancelled = tokio::time::timeout(
        Duration::from_millis(100),
        transport.send(held_call(&retired, 2)),
    )
    .await;
    assert!(cancelled.is_err());
    assert_eq!(retired.load(Ordering::SeqCst), 0);
    transport.close().await.unwrap();
    assert_eq!(retired.load(Ordering::SeqCst), 1);
}
