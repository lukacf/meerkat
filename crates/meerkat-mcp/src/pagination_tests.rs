//! Real rmcp duplex requests through both exported wrappers. No production
//! constructor or mock list implementation is introduced for these regressions.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::McpConnection;
use crate::{McpError, McpProtocol};
use meerkat_core::{McpServerConfig, ToolDef};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool},
    service::{RequestContext, RoleClient, RunningService},
};
use serde_json::json;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};

#[derive(Clone, Copy)]
enum Case {
    TwoPages,
    LaterError,
    Repeat,
    Cycle,
    EmptyCursor,
    Duplicate,
    LaterIoFailure,
}
struct Server {
    case: Case,
    requests: Arc<Mutex<Vec<Option<String>>>>,
    fail_io: Arc<AtomicBool>,
}
fn tool(name: &str, description: &str) -> Tool {
    Tool::new(
        name.to_owned(),
        description.to_owned(),
        json!({
            "type":"object", "properties":{"value":{"type":"string"}}, "required":["value"]
        })
        .as_object()
        .unwrap()
        .clone(),
    )
}
fn page(tools: Vec<Tool>, next_cursor: Option<&str>) -> ListToolsResult {
    ListToolsResult {
        tools,
        next_cursor: next_cursor.map(str::to_owned),
        ..Default::default()
    }
}
impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let cursor = request.and_then(|request| request.cursor);
        let count = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(cursor.clone());
            requests.len()
        };
        // A broken cycle implementation fails a protocol assertion, not an
        // unbounded test. This guard is fixture behavior, never client policy.
        if count > 8 {
            return Err(ErrorData::invalid_params("fixture request guard", None));
        }
        match (self.case, cursor.as_deref()) {
            (Case::EmptyCursor, None) => Ok(page(vec![], Some(""))),
            (Case::EmptyCursor, Some("")) => Ok(page(vec![tool("beta", "second")], None)),
            (_, None) => Ok(page(vec![tool("alpha", "first")], Some("page-two"))),
            (Case::TwoPages, Some("page-two")) => Ok(page(vec![tool("beta", "second")], None)),
            (Case::Duplicate, Some("page-two")) => Ok(page(vec![tool("alpha", "second")], None)),
            (Case::LaterError, Some("page-two")) => Err(ErrorData::invalid_params(
                "second-page-refused",
                Some(json!({"fixture":"later-page"})),
            )),
            (Case::Repeat, Some("page-two")) => {
                Ok(page(vec![tool("beta", "second")], Some("page-two")))
            }
            (Case::Cycle, Some("page-two")) => {
                Ok(page(vec![tool("beta", "second")], Some("page-three")))
            }
            (Case::Cycle, Some("page-three")) => {
                Ok(page(vec![tool("gamma", "third")], Some("page-two")))
            }
            (Case::LaterIoFailure, Some("page-two")) => {
                // Fail the real duplex transport while sending this response;
                // no JSON-RPC error response or mocked client error is used.
                self.fail_io.store(true, Ordering::Release);
                Ok(page(vec![tool("beta", "unwritten")], None))
            }
            _ => Err(ErrorData::invalid_params("unexpected fixture cursor", None)),
        }
    }
}

struct FailOnDemandIo {
    inner: DuplexStream,
    fail: Arc<AtomicBool>,
}
impl AsyncRead for FailOnDemandIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.fail.load(Ordering::Acquire) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "fixture transport closed",
            )));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}
impl AsyncWrite for FailOnDemandIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if this.fail.load(Ordering::Acquire) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "fixture transport closed",
            )));
        }
        Pin::new(&mut this.inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[derive(Clone, Copy, Debug)]
enum Surface {
    Connection,
    Protocol,
}
const SURFACES: [Surface; 2] = [Surface::Connection, Surface::Protocol];
async fn enumerate(
    case: Case,
    surface: Surface,
) -> (Result<Vec<ToolDef>, McpError>, Vec<Option<String>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let fail_io = Arc::new(AtomicBool::new(false));
    let (client_io, server_io) = tokio::io::duplex(8192);
    let server = Server {
        case,
        requests: requests.clone(),
        fail_io: fail_io.clone(),
    };
    let server_task = tokio::spawn(async move {
        let running = server
            .serve(FailOnDemandIo {
                inner: server_io,
                fail: fail_io,
            })
            .await
            .unwrap();
        running.waiting().await.unwrap()
    });
    let service: RunningService<RoleClient, ()> =
        ().serve(client_io)
            .await
            .expect("actual initialize handshake");
    // Both wrappers retain their own production list_tools method. Only their
    // test transport is selected here; the list response traverses rmcp I/O.
    let (result, close) = match surface {
        Surface::Connection => {
            let connection = McpConnection {
                config: McpServerConfig::stdio(
                    "pagination-fixture",
                    "unused-duplex",
                    vec![],
                    Default::default(),
                ),
                service: service.into(),
            };
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                connection.list_tools("source-account"),
            )
            .await;
            (result, connection.close().await)
        }
        Surface::Protocol => {
            let protocol = McpProtocol::new(service);
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                protocol.list_tools("source-account"),
            )
            .await;
            (result, protocol.close().await)
        }
    };
    // Always close and join both actual peers before any result assertion.
    let server_join = server_task.await;
    close.expect("client peer cleanup");
    server_join.expect("server peer cleanup");
    let trace = requests.lock().unwrap().clone();
    (
        result.expect("enumeration must be bounded by fixture responses"),
        trace,
    )
}
fn names(tools: &[ToolDef]) -> Vec<String> {
    tools.iter().map(|tool| tool.name.to_string()).collect()
}

#[tokio::test]
async fn pagination_two_pages_preserve_metadata_and_provenance() {
    for surface in SURFACES {
        let (result, trace) = enumerate(Case::TwoPages, surface).await;
        let tools = result.expect("both pages must succeed");
        assert_eq!(trace, [None, Some("page-two".into())], "{surface:?}");
        assert_eq!(names(&tools), ["alpha", "beta"], "{surface:?}");
        assert_eq!(tools[1].description, "second");
        assert_eq!(tools[1].input_schema["required"], json!(["value"]));
        for tool in tools {
            let provenance = tool.provenance.expect("MCP provenance");
            assert_eq!(provenance.kind, meerkat_core::types::ToolSourceKind::Mcp);
            assert_eq!(provenance.source_id.to_string(), "source-account");
        }
    }
}
#[tokio::test]
async fn pagination_later_error_never_returns_partial_success() {
    for surface in SURFACES {
        let (result, trace) = enumerate(Case::LaterError, surface).await;
        assert_eq!(trace, [None, Some("page-two".into())]);
        assert!(
            matches!(result, Err(McpError::ProtocolError { message }) if message.contains("second-page-refused")),
            "{surface:?}"
        );
    }
}
#[tokio::test]
async fn pagination_repeated_cursor_is_a_protocol_error() {
    for surface in SURFACES {
        let (result, trace) = enumerate(Case::Repeat, surface).await;
        assert_eq!(trace, [None, Some("page-two".into())]);
        assert!(
            matches!(result, Err(McpError::ProtocolError { message }) if message.contains("repeated pagination cursor")),
            "{surface:?}"
        );
    }
}
#[tokio::test]
async fn pagination_longer_cursor_cycle_stops_before_re_request() {
    for surface in SURFACES {
        let (result, trace) = enumerate(Case::Cycle, surface).await;
        assert_eq!(
            trace,
            [None, Some("page-two".into()), Some("page-three".into())]
        );
        assert!(
            matches!(result, Err(McpError::ProtocolError { message }) if message.contains("repeated pagination cursor")),
            "{surface:?}"
        );
    }
}
#[tokio::test]
async fn pagination_empty_page_and_empty_opaque_cursor_are_not_completion() {
    for surface in SURFACES {
        let (result, trace) = enumerate(Case::EmptyCursor, surface).await;
        assert_eq!(trace, [None, Some(String::new())]);
        assert_eq!(names(&result.unwrap()), ["beta"]);
    }
}
#[tokio::test]
async fn pagination_preserves_duplicate_entries_without_inventing_name_policy() {
    for surface in SURFACES {
        let (result, trace) = enumerate(Case::Duplicate, surface).await;
        assert_eq!(trace, [None, Some("page-two".into())]);
        let tools = result.unwrap();
        assert_eq!(names(&tools), ["alpha", "alpha"]);
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.description.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }
}
#[tokio::test]
async fn pagination_later_transport_failure_is_not_partial_success() {
    for surface in SURFACES {
        let (result, trace) = enumerate(Case::LaterIoFailure, surface).await;
        assert_eq!(trace, [None, Some("page-two".into())]);
        assert!(
            matches!(result, Err(McpError::ProtocolError { message }) if message.contains("Failed to list tools")),
            "{surface:?}"
        );
    }
}
