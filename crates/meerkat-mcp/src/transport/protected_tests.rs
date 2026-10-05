#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use crate::transport::sse::{
    ReqwestSseClient, SseClient, SseClientConfig, SseClientTransport, SseTransportError,
};
use crate::transport::streamable_http::ReqwestStreamableHttpClient;
use rmcp::model::{CallToolRequest, CallToolRequestParams};
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
    ClientJsonRpcMessage::request(call.into(), 1.into())
}
fn response() -> Value {
    json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"ok"}]}})
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
async fn protected_posts_do_not_follow_redirects_for_http_or_legacy_sse() {
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
    let http = ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), Default::default());
    let sse = ReqwestSseClient::new(Default::default());
    assert!(
        http.post_message(
            source.url.clone().into(),
            message(true),
            None,
            None,
            Default::default()
        )
        .await
        .is_err()
    );
    assert!(
        sse.post_message(source.url.parse().unwrap(), message(true), None)
            .await
            .is_err()
    );
    assert_eq!(forwarded.load(Ordering::SeqCst), 0);
    http.post_message(
        source.url.clone().into(),
        message(false),
        None,
        None,
        Default::default(),
    )
    .await
    .unwrap();
    sse.post_message(source.url.parse().unwrap(), message(false), None)
        .await
        .unwrap();
    assert_eq!(forwarded.load(Ordering::SeqCst), 2);
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

#[tokio::test]
async fn real_rmcp_stdio_trace_keeps_live_metadata_and_malformed_input_private() {
    use futures::FutureExt;
    use rmcp::{
        ServiceExt,
        model::{ServerCapabilities, ServerInfo},
    };
    let captured = Capture::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let _trace_guard = tracing::subscriber::set_default(subscriber);
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
    assert!(logs.contains("MCP input refused: invalid JSON-RPC frame"));
    assert!(!logs.contains(SECRET));
}
