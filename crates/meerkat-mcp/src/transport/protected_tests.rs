#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use crate::transport::sse::{
    ReqwestSseClient, SseClient, SseClientConfig, SseClientTransport, SseTransportError,
};
use crate::transport::streamable_http::ReqwestStreamableHttpClient;
use rmcp::model::{CallToolRequest, CallToolRequestParams, NumberOrString};
use rmcp::transport::streamable_http_client::{StreamableHttpClient, StreamableHttpError};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
async fn protected_connection_session_expiry_never_enters_rmcp_replay_branch() {
    let server = HttpFixture::start(axum::Router::new().route(
        "/mcp",
        axum::routing::post(|| async { axum::http::StatusCode::NOT_FOUND }),
    ))
    .await;
    let state = ProtectedMetadataState::default();
    let client =
        ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), Default::default())
            .with_protected_metadata(state.clone());
    let ordinary = client
        .post_message(
            server.url.clone().into(),
            message(false),
            Some("session".into()),
            None,
            Default::default(),
        )
        .await;
    assert!(matches!(ordinary, Err(StreamableHttpError::SessionExpired)));
    state.register(&metadata()).unwrap();
    for selected in [true, false] {
        let error = client
            .post_message(
                server.url.clone().into(),
                message(selected),
                Some("session".into()),
                None,
                Default::default(),
            )
            .await;
        assert!(matches!(
            error,
            Err(StreamableHttpError::UnexpectedServerResponse(_))
        ));
    }
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

#[derive(Clone)]
struct EndpointClient {
    posts: Arc<AtomicUsize>,
}
impl SseClient for EndpointClient {
    type Error = io::Error;
    async fn post_message(
        &self,
        _: http::Uri,
        _: ClientJsonRpcMessage,
        _: Option<String>,
    ) -> Result<(), SseTransportError<Self::Error>> {
        self.posts.fetch_add(1, Ordering::SeqCst);
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
        Ok(futures::stream::once(async {
            Ok(sse_stream::Sse {
                event: Some("endpoint".into()),
                data: Some("http://127.0.0.1:10002/messages".into()),
                ..Default::default()
            })
        })
        .boxed())
    }
}

#[tokio::test]
async fn protected_legacy_sse_rejects_cross_origin_endpoint_before_post() {
    let posts = Arc::new(AtomicUsize::new(0));
    let mut transport = SseClientTransport::start_with_client(
        EndpointClient {
            posts: posts.clone(),
        },
        SseClientConfig {
            sse_endpoint: "http://127.0.0.1:10001/events".into(),
            use_message_endpoint: None,
        },
    )
    .await
    .unwrap();
    assert!(transport.send(message(true)).await.is_err());
    assert_eq!(posts.load(Ordering::SeqCst), 0);
    transport.send(message(false)).await.unwrap();
    assert_eq!(posts.load(Ordering::SeqCst), 1);
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
