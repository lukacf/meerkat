//! JSONL transport over AsyncBufRead/AsyncWrite.
//!
//! Provides a generic `JsonlTransport` that reads JSON-RPC requests
//! line-by-line and writes responses/notifications as JSONL.

use std::io::Write;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::protocol::{RpcMessage, RpcNotification, RpcRequest, RpcResponse};

/// Errors that can occur during transport operations.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to parse JSON.
    #[error("JSON parse error: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Trait for writing serialized JSONL messages to the transport output.
#[async_trait]
pub trait TransportWriter: Send {
    /// Write a complete message (including trailing newline) and flush.
    /// Returns only after bytes are durably written (not merely enqueued).
    async fn write_message(&mut self, bytes: Vec<u8>) -> Result<(), TransportError>;
}

/// Blanket impl: any AsyncWrite + Unpin + Send gets TransportWriter for free.
#[async_trait]
impl<W: AsyncWrite + Unpin + Send> TransportWriter for W {
    async fn write_message(&mut self, bytes: Vec<u8>) -> Result<(), TransportError> {
        self.write_all(&bytes).await?;
        self.flush().await?;
        Ok(())
    }
}

/// Blocking writer — delegates to a `Write` impl via `spawn_blocking`.
/// Production: wraps `std::io::Stdout`. Tests: wraps any `Write + Send`.
pub struct BlockingWriter {
    inner: Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
}

impl BlockingWriter {
    /// Create a blocking writer for stdout.
    pub fn stdout() -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(Box::new(std::io::stdout()))),
        }
    }

    /// Create a blocking writer wrapping any `Write`.
    pub fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(writer)),
        }
    }
}

#[async_trait]
impl TransportWriter for BlockingWriter {
    async fn write_message(&mut self, bytes: Vec<u8>) -> Result<(), TransportError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut w = inner
                .lock()
                .map_err(|_| std::io::Error::other("writer mutex poisoned"))?;
            w.write_all(&bytes)?;
            w.flush()?;
            Ok::<(), std::io::Error>(())
        })
        .await
        .map_err(|e| TransportError::Io(std::io::Error::other(e)))??;
        Ok(())
    }
}

/// JSONL transport for reading JSON-RPC requests and writing responses/notifications.
///
/// Generic over reader and writer types for testability.
pub struct JsonlTransport<R, W> {
    reader: R,
    writer: W,
}

impl<R: AsyncBufRead + Unpin, W: TransportWriter> JsonlTransport<R, W> {
    /// Create a new JSONL transport.
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }

    /// Read the next JSON-RPC message (request or response) from the transport.
    ///
    /// Returns `Ok(None)` on EOF. Skips empty lines.
    /// Discriminates by field presence: `method` → Request, otherwise → Response.
    pub async fn read_message(&mut self) -> Result<Option<RpcMessage>, TransportError> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes_read = self.reader.read_line(&mut line).await?;
            if bytes_read == 0 {
                return Ok(None); // EOF
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue; // skip empty lines
            }
            // Peek at the JSON to determine if it's a request or response.
            let value: serde_json::Value = serde_json::from_str(trimmed)?;
            if value.get("method").is_some() {
                let request: RpcRequest = serde_json::from_value(value)?;
                return Ok(Some(RpcMessage::Request(request)));
            }
            let response: RpcResponse = serde_json::from_value(value)?;
            return Ok(Some(RpcMessage::Response(response)));
        }
    }

    /// Write a JSON-RPC request as a single JSONL line (for server→client callbacks).
    pub async fn write_request(&mut self, request: &RpcRequest) -> Result<(), TransportError> {
        let mut bytes = serde_json::to_vec(request)?;
        bytes.push(b'\n');
        self.writer.write_message(bytes).await
    }

    /// Write a JSON-RPC response as a single JSONL line.
    pub async fn write_response(&mut self, response: &RpcResponse) -> Result<(), TransportError> {
        let mut bytes = serde_json::to_vec(response)?;
        bytes.push(b'\n');
        self.writer.write_message(bytes).await
    }

    /// Write a JSON-RPC notification as a single JSONL line.
    pub async fn write_notification(
        &mut self,
        notification: &RpcNotification,
    ) -> Result<(), TransportError> {
        let mut bytes = serde_json::to_vec(notification)?;
        bytes.push(b'\n');
        self.writer.write_message(bytes).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::protocol::{RpcId, RpcNotification, RpcResponse};
    use tokio::io::BufReader;

    /// Helper to create a transport from an input string and a Vec<u8> writer.
    fn make_transport(input: &str) -> JsonlTransport<BufReader<std::io::Cursor<Vec<u8>>>, Vec<u8>> {
        let cursor = std::io::Cursor::new(input.as_bytes().to_vec());
        let reader = BufReader::new(cursor);
        let writer = Vec::<u8>::new();
        JsonlTransport::new(reader, writer)
    }

    /// Unwrap an `RpcMessage` as a `Request`, panicking otherwise.
    fn unwrap_request(msg: RpcMessage) -> RpcRequest {
        match msg {
            RpcMessage::Request(req) => req,
            RpcMessage::Response(_) => panic!("expected Request, got Response"),
        }
    }

    #[tokio::test]
    async fn read_single_request() {
        let input =
            r#"{"jsonrpc":"2.0","id":1,"method":"session/create","params":{"model":"test"}}"#;
        let input_line = format!("{input}\n");
        let mut transport = make_transport(&input_line);

        let msg = transport.read_message().await.unwrap();
        assert!(msg.is_some());
        let req = unwrap_request(msg.unwrap());
        assert_eq!(req.id, Some(RpcId::Num(1)));
        assert_eq!(req.method, "session/create");
    }

    #[tokio::test]
    async fn read_multiple_requests() {
        let line1 = r#"{"jsonrpc":"2.0","id":1,"method":"a"}"#;
        let line2 = r#"{"jsonrpc":"2.0","id":2,"method":"b"}"#;
        let line3 = r#"{"jsonrpc":"2.0","id":3,"method":"c"}"#;
        let input = format!("{line1}\n{line2}\n{line3}\n");
        let mut transport = make_transport(&input);

        let r1 = unwrap_request(transport.read_message().await.unwrap().unwrap());
        assert_eq!(r1.method, "a");

        let r2 = unwrap_request(transport.read_message().await.unwrap().unwrap());
        assert_eq!(r2.method, "b");

        let r3 = unwrap_request(transport.read_message().await.unwrap().unwrap());
        assert_eq!(r3.method, "c");

        // After all lines, should get None (EOF)
        let r4 = transport.read_message().await.unwrap();
        assert!(r4.is_none());
    }

    #[tokio::test]
    async fn write_response_single_jsonl_line() {
        let mut transport = make_transport("");
        let resp = RpcResponse::success(Some(RpcId::Num(1)), serde_json::json!({"ok": true}));
        transport.write_response(&resp).await.unwrap();

        let output = String::from_utf8(transport.writer.clone()).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(output.ends_with('\n'));

        // Verify the line is valid JSON
        let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["result"]["ok"], true);
    }

    #[tokio::test]
    async fn write_notification_single_jsonl_line() {
        let mut transport = make_transport("");
        let notif = RpcNotification::new("event", serde_json::json!({"delta": "hi"}));
        transport.write_notification(&notif).await.unwrap();

        let output = String::from_utf8(transport.writer.clone()).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(output.ends_with('\n'));

        let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed["method"], "event");
        assert_eq!(parsed["params"]["delta"], "hi");
    }

    #[tokio::test]
    async fn empty_lines_are_skipped() {
        let input = format!(
            "\n\n{}\n\n{}\n\n",
            r#"{"jsonrpc":"2.0","id":1,"method":"first"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"second"}"#,
        );
        let mut transport = make_transport(&input);

        let r1 = unwrap_request(transport.read_message().await.unwrap().unwrap());
        assert_eq!(r1.method, "first");

        let r2 = unwrap_request(transport.read_message().await.unwrap().unwrap());
        assert_eq!(r2.method, "second");

        let r3 = transport.read_message().await.unwrap();
        assert!(r3.is_none());
    }

    #[tokio::test]
    async fn eof_returns_none() {
        let mut transport = make_transport("");
        let result = transport.read_message().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error() {
        let input = "this is not json\n";
        let mut transport = make_transport(input);

        let result = transport.read_message().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, TransportError::Parse(_)),
            "Expected Parse error, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn multiple_writes_do_not_corrupt() {
        let mut transport = make_transport("");

        let resp1 = RpcResponse::success(Some(RpcId::Num(1)), serde_json::json!("first"));
        let resp2 = RpcResponse::success(Some(RpcId::Num(2)), serde_json::json!("second"));
        let notif = RpcNotification::new("event", serde_json::json!({"n": 3}));

        transport.write_response(&resp1).await.unwrap();
        transport.write_response(&resp2).await.unwrap();
        transport.write_notification(&notif).await.unwrap();

        let output = String::from_utf8(transport.writer.clone()).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3);

        // Each line should be independently valid JSON
        let p1: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(p1["id"], 1);
        assert_eq!(p1["result"], "first");

        let p2: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(p2["id"], 2);
        assert_eq!(p2["result"], "second");

        let p3: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(p3["method"], "event");
        assert_eq!(p3["params"]["n"], 3);
    }

    #[tokio::test]
    async fn read_message_parses_request_and_response() {
        let req_line = r#"{"jsonrpc":"2.0","id":1,"method":"turn/start","params":{}}"#;
        let resp_line =
            r#"{"jsonrpc":"2.0","id":"srv-1","result":{"content":"ok","is_error":false}}"#;
        let input = format!("{req_line}\n{resp_line}\n");
        let mut transport = make_transport(&input);

        // First line has `method` → should be Request.
        let msg1 = transport.read_message().await.unwrap().unwrap();
        assert!(matches!(msg1, RpcMessage::Request(_)));

        // Second line has no `method` → should be Response.
        let msg2 = transport.read_message().await.unwrap().unwrap();
        assert!(matches!(msg2, RpcMessage::Response(_)));
    }

    // --- BlockingWriter tests ---

    /// Thin Write adapter wrapping a shared Vec<u8> sink.
    struct SharedSink(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SharedSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut inner = self
                .0
                .lock()
                .map_err(|_| std::io::Error::other("lock poisoned"))?;
            inner.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn blocking_writer_happy_path() {
        let sink = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let writer = BlockingWriter::new(Box::new(SharedSink(sink.clone())));
        let cursor = std::io::Cursor::new(Vec::new());
        let reader = BufReader::new(cursor);
        let mut transport = JsonlTransport::new(reader, writer);

        let notif = RpcNotification::new("test/event", serde_json::json!({"key": "value"}));
        transport.write_notification(&notif).await.unwrap();

        let bytes = sink.lock().unwrap();
        let output = String::from_utf8(bytes.clone()).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(output.ends_with('\n'));

        let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed["method"], "test/event");
        assert_eq!(parsed["params"]["key"], "value");
    }

    #[tokio::test]
    async fn blocking_writer_error_propagation() {
        /// Write impl that always returns BrokenPipe.
        struct BrokenPipeWriter;

        impl std::io::Write for BrokenPipeWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "pipe broken",
                ))
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut writer = BlockingWriter::new(Box::new(BrokenPipeWriter));
        let result = writer.write_message(b"hello\n".to_vec()).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            TransportError::Io(io_err) => {
                assert_eq!(io_err.kind(), std::io::ErrorKind::BrokenPipe);
            }
            other => panic!("Expected TransportError::Io with BrokenPipe, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn blocking_writer_backpressure() {
        /// Write impl that sleeps before succeeding.
        struct SlowWriter;

        impl std::io::Write for SlowWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                std::thread::sleep(std::time::Duration::from_millis(50));
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut writer = BlockingWriter::new(Box::new(SlowWriter));
        let start = std::time::Instant::now();
        writer.write_message(b"data\n".to_vec()).await.unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed >= std::time::Duration::from_millis(50),
            "Expected >= 50ms, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn would_block_writer_regression() {
        use std::pin::Pin;
        use std::task::{Context, Poll};

        /// AsyncWrite impl that always returns WouldBlock.
        struct WouldBlockWriter;

        impl tokio::io::AsyncWrite for WouldBlockWriter {
            fn poll_write(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                _buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "would block",
                )))
            }

            fn poll_flush(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }

            fn poll_shutdown(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }

        let cursor = std::io::Cursor::new(Vec::new());
        let reader = BufReader::new(cursor);
        let mut transport = JsonlTransport::new(reader, WouldBlockWriter);

        let notif = RpcNotification::new("test", serde_json::json!(null));
        let result = transport.write_notification(&notif).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            TransportError::Io(io_err) => {
                assert_eq!(io_err.kind(), std::io::ErrorKind::WouldBlock);
            }
            other => panic!("Expected TransportError::Io with WouldBlock, got: {other:?}"),
        }
    }
}
