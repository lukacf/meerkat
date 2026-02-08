//! JSONL transport over AsyncBufRead/AsyncWrite.
//!
//! Provides a generic `JsonlTransport` that reads JSON-RPC requests
//! line-by-line and writes responses/notifications as JSONL.

use crate::protocol::{RpcNotification, RpcRequest, RpcResponse};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

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

/// JSONL transport for reading JSON-RPC requests and writing responses/notifications.
///
/// Generic over reader and writer types for testability.
pub struct JsonlTransport<R, W> {
    reader: R,
    writer: W,
}

impl<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin> JsonlTransport<R, W> {
    /// Create a new JSONL transport.
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }

    /// Read the next JSON-RPC request from the transport.
    ///
    /// Returns `Ok(None)` on EOF. Skips empty lines.
    pub async fn read_message(&mut self) -> Result<Option<RpcRequest>, TransportError> {
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
            let request: RpcRequest = serde_json::from_str(trimmed)?;
            return Ok(Some(request));
        }
    }

    /// Write a JSON-RPC response as a single JSONL line.
    pub async fn write_response(&mut self, response: &RpcResponse) -> Result<(), TransportError> {
        let json = serde_json::to_string(response)?;
        self.writer.write_all(json.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// Write a JSON-RPC notification as a single JSONL line.
    pub async fn write_notification(
        &mut self,
        notification: &RpcNotification,
    ) -> Result<(), TransportError> {
        let json = serde_json::to_string(notification)?;
        self.writer.write_all(json.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
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

    #[tokio::test]
    async fn read_single_request() {
        let input =
            r#"{"jsonrpc":"2.0","id":1,"method":"session/create","params":{"model":"test"}}"#;
        let input_line = format!("{input}\n");
        let mut transport = make_transport(&input_line);

        let msg = transport.read_message().await.unwrap();
        assert!(msg.is_some());
        let req = msg.unwrap();
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

        let r1 = transport.read_message().await.unwrap().unwrap();
        assert_eq!(r1.method, "a");

        let r2 = transport.read_message().await.unwrap().unwrap();
        assert_eq!(r2.method, "b");

        let r3 = transport.read_message().await.unwrap().unwrap();
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

        let r1 = transport.read_message().await.unwrap().unwrap();
        assert_eq!(r1.method, "first");

        let r2 = transport.read_message().await.unwrap().unwrap();
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
}
