//! Lightweight async JSON-RPC 2.0 client over TCP (JSONL protocol).
//!
//! Connects to a remote JSON-RPC server, sends requests with auto-incrementing
//! IDs, and forwards notifications to a caller-supplied channel. Responses are
//! routed back to the originating `request()` future even when interleaved with
//! notifications from the server.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::{Mutex, mpsc, oneshot};

/// Default timeout for a single JSON-RPC request.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A pending-request map shared between the background reader and callers.
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

/// Errors surfaced by [`RpcClient`].
#[derive(Debug)]
pub enum RpcError {
    /// The server returned a JSON-RPC error object.
    JsonRpc { code: i64, message: String },
    /// The TCP connection was lost or the client has been shut down.
    Disconnected,
    /// The request timed out waiting for a response.
    Timeout,
    /// A transport-level I/O or serialisation error.
    Transport(String),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::JsonRpc { code, message } => {
                write!(f, "JSON-RPC error {code}: {message}")
            }
            RpcError::Disconnected => f.write_str("disconnected"),
            RpcError::Timeout => f.write_str("request timed out"),
            RpcError::Transport(msg) => write!(f, "transport error: {msg}"),
        }
    }
}

impl std::error::Error for RpcError {}

/// Async JSON-RPC 2.0 client over a TCP/JSONL transport.
///
/// Multiple requests can be in flight concurrently. Notifications from the
/// server are forwarded to the `mpsc::UnboundedSender<Value>` provided at
/// construction time.
pub struct RpcClient {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    next_id: Arc<AtomicU64>,
    connected: Arc<AtomicBool>,
    pending: PendingMap,
    request_timeout: Duration,
    /// Handle to the background reader task so we can abort on close.
    reader_handle: Option<tokio::task::JoinHandle<()>>,
}

impl RpcClient {
    /// Open a TCP connection to `addr` (e.g. `"127.0.0.1:9500"` or
    /// `"tcp://127.0.0.1:9500"`) and return a ready-to-use client.
    ///
    /// Notifications received from the server are forwarded to `notification_tx`.
    pub async fn connect(
        addr: &str,
        notification_tx: mpsc::UnboundedSender<Value>,
    ) -> Result<Self, RpcError> {
        let tcp_addr = addr.strip_prefix("tcp://").unwrap_or(addr);
        let stream = TcpStream::connect(tcp_addr)
            .await
            .map_err(|e| RpcError::Transport(e.to_string()))?;

        let (read_half, write_half) = stream.into_split();

        let connected = Arc::new(AtomicBool::new(true));
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        let reader_handle = {
            let connected = connected.clone();
            let pending = pending.clone();
            tokio::spawn(reader_loop(read_half, pending, notification_tx, connected))
        };

        Ok(Self {
            writer: Arc::new(Mutex::new(write_half)),
            next_id: Arc::new(AtomicU64::new(1)),
            connected,
            pending,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            reader_handle: Some(reader_handle),
        })
    }

    /// Override the per-request timeout (default: 30 s).
    pub fn set_request_timeout(&mut self, timeout: Duration) {
        self.request_timeout = timeout;
    }

    /// Returns `true` if the underlying TCP connection is still alive.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    /// Send a JSON-RPC request and wait for the matching response.
    ///
    /// Notifications that arrive while waiting are transparently forwarded to
    /// the notification channel — they never block this method.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        if !self.is_connected() {
            return Err(RpcError::Disconnected);
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": id,
        });

        let mut line =
            serde_json::to_string(&request).map_err(|e| RpcError::Transport(e.to_string()))?;
        line.push('\n');

        {
            let mut writer = self.writer.lock().await;
            if let Err(e) = writer.write_all(line.as_bytes()).await {
                self.remove_pending(id).await;
                return Err(RpcError::Transport(e.to_string()));
            }
            if let Err(e) = writer.flush().await {
                self.remove_pending(id).await;
                return Err(RpcError::Transport(e.to_string()));
            }
        }

        match tokio::time::timeout(self.request_timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                // oneshot sender dropped — reader died or connection lost.
                Err(RpcError::Disconnected)
            }
            Err(_) => {
                self.remove_pending(id).await;
                Err(RpcError::Timeout)
            }
        }
    }

    /// Cleanly shut down the client: abort the reader task and drop the
    /// connection.
    pub async fn close(mut self) {
        self.connected.store(false, Ordering::Release);
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
            let _ = handle.await;
        }
        cancel_all_pending(&self.pending).await;
    }

    /// Remove a single pending entry (used on write-failure or timeout).
    async fn remove_pending(&self, id: u64) {
        let mut map = self.pending.lock().await;
        map.remove(&id);
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        self.connected.store(false, Ordering::Release);
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
    }
}

// ── Background reader task ───────────────────────────────────────────────────

async fn reader_loop(
    read_half: tokio::net::tcp::OwnedReadHalf,
    pending: PendingMap,
    notification_tx: mpsc::UnboundedSender<Value>,
    connected: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                // EOF — remote closed.
                break;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let parsed: Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(_) => continue, // skip malformed lines
                };
                route_message(parsed, &pending, &notification_tx).await;
            }
            Err(_) => {
                // I/O error — connection broken.
                break;
            }
        }
    }

    // Mark disconnected and cancel all pending requests.
    connected.store(false, Ordering::Release);
    cancel_all_pending(&pending).await;
}

/// Route a parsed JSON object to either a pending request or the notification
/// channel.
async fn route_message(
    msg: Value,
    pending: &PendingMap,
    notification_tx: &mpsc::UnboundedSender<Value>,
) {
    // If the message has an `id` field it is a response; otherwise treat it as
    // a notification.
    if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
        let result = if let Some(err) = msg.get("error") {
            let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
            let message = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error")
                .to_string();
            Err(RpcError::JsonRpc { code, message })
        } else {
            Ok(msg.get("result").cloned().unwrap_or(Value::Null))
        };

        let mut map = pending.lock().await;
        if let Some(tx) = map.remove(&id) {
            let _ = tx.send(result);
        }
    } else {
        // No `id` — it is a notification.
        let _ = notification_tx.send(msg);
    }
}

/// Cancel all pending requests with a [`RpcError::Disconnected`] error.
async fn cancel_all_pending(pending: &PendingMap) {
    let mut map = pending.lock().await;
    for (_, tx) in map.drain() {
        let _ = tx.send(Err(RpcError::Disconnected));
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// Spin up a mock server that echoes back a result equal to the request id.
    async fn mock_echo_server() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let req: Value = serde_json::from_str(line.trim()).unwrap();
                        let id = req.get("id").unwrap().clone();
                        let response = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "echo_id": id }
                        });
                        let mut resp_line = serde_json::to_string(&response).unwrap();
                        resp_line.push('\n');
                        write_half.write_all(resp_line.as_bytes()).await.unwrap();
                        write_half.flush().await.unwrap();
                    }
                    Err(_) => break,
                }
            }
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn request_ids_auto_increment() {
        let (addr, server) = mock_echo_server().await;
        let (ntf_tx, _ntf_rx) = mpsc::unbounded_channel();
        let client = RpcClient::connect(&addr.to_string(), ntf_tx).await.unwrap();

        let r1 = client.request("ping", serde_json::json!({})).await.unwrap();
        let r2 = client.request("ping", serde_json::json!({})).await.unwrap();

        assert_eq!(r1["echo_id"], 1);
        assert_eq!(r2["echo_id"], 2);

        client.close().await;
        server.abort();
    }

    #[tokio::test]
    async fn response_routed_to_correct_pending_request() {
        // Server that delays the first response so the second arrives first.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            let mut requests = Vec::new();
            let mut line = String::new();

            // Read two requests.
            for _ in 0..2 {
                line.clear();
                reader.read_line(&mut line).await.unwrap();
                let req: Value = serde_json::from_str(line.trim()).unwrap();
                requests.push(req);
            }

            // Reply in reverse order.
            for req in requests.into_iter().rev() {
                let id = req.get("id").unwrap().clone();
                let method = req.get("method").unwrap().as_str().unwrap().to_string();
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "method": method }
                });
                let mut resp_line = serde_json::to_string(&response).unwrap();
                resp_line.push('\n');
                write_half.write_all(resp_line.as_bytes()).await.unwrap();
                write_half.flush().await.unwrap();
            }
        });

        let (ntf_tx, _ntf_rx) = mpsc::unbounded_channel();
        let client = Arc::new(RpcClient::connect(&addr.to_string(), ntf_tx).await.unwrap());

        // Fire both requests concurrently.
        let c1 = client.clone();
        let c2 = client.clone();
        let (r1, r2) = tokio::join!(
            async move { c1.request("first", serde_json::json!({})).await },
            async move { c2.request("second", serde_json::json!({})).await },
        );

        // Each future must get its own response despite reversed delivery.
        assert_eq!(r1.unwrap()["method"], "first");
        assert_eq!(r2.unwrap()["method"], "second");

        server.abort();
    }

    #[tokio::test]
    async fn notifications_forwarded_to_channel() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();

            // Wait for one request.
            reader.read_line(&mut line).await.unwrap();
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let id = req.get("id").unwrap().clone();

            // Send a notification *before* the response.
            let notification = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "session/event",
                "params": { "kind": "progress", "value": 42 }
            });
            let mut ntf_line = serde_json::to_string(&notification).unwrap();
            ntf_line.push('\n');
            write_half.write_all(ntf_line.as_bytes()).await.unwrap();

            // Now send the actual response.
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": "ok"
            });
            let mut resp_line = serde_json::to_string(&response).unwrap();
            resp_line.push('\n');
            write_half.write_all(resp_line.as_bytes()).await.unwrap();
            write_half.flush().await.unwrap();
        });

        let (ntf_tx, mut ntf_rx) = mpsc::unbounded_channel();
        let client = RpcClient::connect(&addr.to_string(), ntf_tx).await.unwrap();

        let result = client.request("ping", serde_json::json!({})).await.unwrap();
        assert_eq!(result, "ok");

        // The notification should have been forwarded.
        let notification = ntf_rx.try_recv().unwrap();
        assert_eq!(notification["method"], "session/event");
        assert_eq!(notification["params"]["value"], 42);

        client.close().await;
        server.abort();
    }

    #[tokio::test]
    async fn disconnect_detected_on_server_close() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            // Close immediately.
            drop(stream);
        });

        let (ntf_tx, _ntf_rx) = mpsc::unbounded_channel();
        let client = RpcClient::connect(&addr.to_string(), ntf_tx).await.unwrap();

        // Give the reader loop time to notice EOF.
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(!client.is_connected());
        server.abort();
    }
}
