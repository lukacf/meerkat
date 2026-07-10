//! JSONL transport over AsyncBufRead/AsyncWrite.
//!
//! Provides a generic `JsonlTransport` that reads JSON-RPC requests
//! line-by-line and writes responses/notifications as JSONL.

use std::io::Write;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::protocol::{RpcMessage, RpcNotification, RpcRequest, RpcResponse};

/// Maximum JSONL frame size, excluding the trailing line-feed byte.
///
/// A 20 MiB binary image expands to roughly 27 MiB when base64-encoded. The
/// 64 MiB ceiling leaves ample room for the surrounding JSON-RPC envelope and
/// future metadata while preventing a peer from growing the line buffer
/// without bound.
pub const JSONL_MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// Frames at or below this threshold use a reserved control pool and can never
/// queue behind bulk traffic. Larger frames retain that small reservation and
/// grow non-blocking bulk reservations as bytes are copied from the bounded
/// `BufReader` window.
pub(crate) const JSONL_SMALL_FRAME_BYTES: usize = 64 * 1024;
const JSONL_FRAME_MEMORY_MULTIPLIER: usize = 3;
const JSONL_SMALL_FRAME_PROCESS_BUDGET_BYTES: usize = 32 * 1024 * 1024;
const JSONL_BULK_FRAME_PROCESS_BUDGET_BYTES: usize = 224 * 1024 * 1024;
const JSONL_FRAME_BASE_PROGRESS_TIMEOUT: Duration = Duration::from_secs(10);
const JSONL_FRAME_MIN_PROGRESS_BYTES_PER_SECOND: usize = 64 * 1024;
const JSONL_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct JsonlFrameAdmission {
    small_semaphore: Arc<Semaphore>,
    bulk_semaphore: Arc<Semaphore>,
}

impl JsonlFrameAdmission {
    pub(crate) fn production() -> Self {
        static PROCESS_ADMISSION: OnceLock<JsonlFrameAdmission> = OnceLock::new();
        PROCESS_ADMISSION
            .get_or_init(|| {
                Self::new(
                    JSONL_SMALL_FRAME_PROCESS_BUDGET_BYTES,
                    JSONL_BULK_FRAME_PROCESS_BUDGET_BYTES,
                )
            })
            .clone()
    }

    pub(crate) fn new(small_budget_bytes: usize, bulk_budget_bytes: usize) -> Self {
        Self {
            small_semaphore: Arc::new(Semaphore::new(small_budget_bytes)),
            bulk_semaphore: Arc::new(Semaphore::new(bulk_budget_bytes)),
        }
    }

    #[cfg(test)]
    pub(crate) fn shares_process_owner_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.small_semaphore, &other.small_semaphore)
            && Arc::ptr_eq(&self.bulk_semaphore, &other.bulk_semaphore)
    }

    #[cfg(test)]
    pub(crate) fn available_small_bytes(&self) -> usize {
        self.small_semaphore.available_permits()
    }
}

#[derive(Default)]
pub(crate) struct JsonlFrameMemoryPermit {
    small_permit: Option<OwnedSemaphorePermit>,
    bulk_permits: Vec<OwnedSemaphorePermit>,
    bulk_charged_bytes: usize,
}

impl JsonlFrameMemoryPermit {
    fn try_grow(
        &mut self,
        admission: &JsonlFrameAdmission,
        target_frame_bytes: usize,
    ) -> Result<(), TransportError> {
        let small_charge = JSONL_SMALL_FRAME_BYTES * JSONL_FRAME_MEMORY_MULTIPLIER;
        if self.small_permit.is_none() && target_frame_bytes > 0 {
            let permits = u32::try_from(small_charge).map_err(|_| {
                TransportError::FrameAdmissionBackpressured {
                    requested_bytes: small_charge,
                }
            })?;
            self.small_permit = Some(
                Arc::clone(&admission.small_semaphore)
                    .try_acquire_many_owned(permits)
                    .map_err(|_| TransportError::FrameAdmissionBackpressured {
                        requested_bytes: small_charge,
                    })?,
            );
        }

        let total_charge = target_frame_bytes
            .max(JSONL_SMALL_FRAME_BYTES)
            .saturating_mul(JSONL_FRAME_MEMORY_MULTIPLIER);
        let target_bulk_charge = total_charge.saturating_sub(small_charge);
        let additional = target_bulk_charge.saturating_sub(self.bulk_charged_bytes);
        if additional > 0 {
            let permits = u32::try_from(additional).map_err(|_| {
                TransportError::FrameAdmissionBackpressured {
                    requested_bytes: additional,
                }
            })?;
            let permit = Arc::clone(&admission.bulk_semaphore)
                .try_acquire_many_owned(permits)
                .map_err(|_| TransportError::FrameAdmissionBackpressured {
                    requested_bytes: additional,
                })?;
            self.bulk_permits.push(permit);
            self.bulk_charged_bytes = target_bulk_charge;
        }
        Ok(())
    }
}

pub(crate) struct AdmittedJsonlMessage {
    pub(crate) message: RpcMessage,
    /// Original non-newline wire length. Consumers use this before handing a
    /// parsed callback response across an internal channel, even when ignored
    /// fields were deliberately discarded during deserialization.
    pub(crate) wire_bytes: usize,
    pub(crate) _frame_memory_permit: JsonlFrameMemoryPermit,
}

/// Allocation-free field-presence probe used to distinguish requests from
/// responses before deserializing directly into the final `RawValue`-backed
/// envelope. Parsing through `serde_json::Value` first duplicates a maximum
/// frame's complete params tree and then serializes it again into `RawValue`,
/// multiplying pre-admission memory on every TCP connection.
#[derive(Default)]
struct JsonFieldPresence(bool);

impl<'de> Deserialize<'de> for JsonFieldPresence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
        Ok(Self(true))
    }
}

#[derive(Deserialize)]
struct RpcMessageKindProbe {
    #[serde(default)]
    method: JsonFieldPresence,
}

/// Inbound client callback errors never expose `data` to a consumer. Parse it
/// as `IgnoredAny` so pathological JSON trees cannot expand into an allocated
/// `serde_json::Value` before callback response admission.
#[derive(Deserialize)]
struct InboundRpcResponse {
    jsonrpc: String,
    id: Option<crate::protocol::RpcId>,
    #[serde(default)]
    result: Option<Box<serde_json::value::RawValue>>,
    #[serde(default)]
    error: Option<InboundRpcError>,
}

#[derive(Deserialize)]
struct InboundRpcError {
    code: i32,
    message: String,
    #[serde(default, rename = "data")]
    _data: Option<serde::de::IgnoredAny>,
}

impl From<InboundRpcResponse> for RpcResponse {
    fn from(value: InboundRpcResponse) -> Self {
        Self::from_inbound_parts(
            value.jsonrpc,
            value.id,
            value.result,
            value.error.map(|error| crate::protocol::RpcError {
                code: error.code,
                message: error.message,
                data: None,
            }),
        )
    }
}

/// Errors that can occur during transport operations.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to parse JSON.
    #[error("JSON parse error: {0}")]
    Parse(#[from] serde_json::Error),
    /// A JSONL frame exceeded the configured byte ceiling.
    #[error(
        "JSONL frame exceeds the {max_bytes}-byte limit (read at least {observed_bytes} bytes before newline)"
    )]
    FrameTooLarge {
        /// Maximum accepted frame size, excluding the trailing line feed.
        max_bytes: usize,
        /// Number of non-newline bytes observed before rejecting the frame.
        observed_bytes: usize,
    },
    /// Process-wide incremental frame memory is saturated. This never waits in
    /// a FIFO semaphore queue; the offending connection fails closed.
    #[error("JSONL frame memory admission is saturated for {requested_bytes} additional bytes")]
    FrameAdmissionBackpressured { requested_bytes: usize },
    /// A partial frame failed the size-aware minimum-progress deadline.
    #[error("JSONL frame did not make bounded progress before its deadline")]
    FrameProgressTimeout,
    /// The peer stopped draining an outbound frame. The connection is failed
    /// closed so one writer cannot retain admission permits indefinitely.
    #[error("JSONL transport write exceeded its bounded deadline")]
    WriteTimeout,
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
    max_frame_bytes: usize,
    admitted_frame: Vec<u8>,
    admitted_frame_memory: JsonlFrameMemoryPermit,
    admitted_frame_started_at: Option<tokio::time::Instant>,
}

impl<R: AsyncBufRead + Unpin, W: TransportWriter> JsonlTransport<R, W> {
    /// Create a new JSONL transport.
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            max_frame_bytes: JSONL_MAX_FRAME_BYTES,
            admitted_frame: Vec::new(),
            admitted_frame_memory: JsonlFrameMemoryPermit::default(),
            admitted_frame_started_at: None,
        }
    }

    #[cfg(test)]
    fn with_max_frame_bytes(reader: R, writer: W, max_frame_bytes: usize) -> Self {
        Self {
            reader,
            writer,
            max_frame_bytes,
            admitted_frame: Vec::new(),
            admitted_frame_memory: JsonlFrameMemoryPermit::default(),
            admitted_frame_started_at: None,
        }
    }

    /// Cancellation-safe admitted read used by the multiplexing RPC server.
    /// Bytes and permits live on `self` across canceled polls. Small frames use
    /// a reserved pool; bulk growth acquires additional process bytes with
    /// `try_acquire` before copying from the reader's fixed-size buffer.
    pub(crate) async fn read_message_admitted(
        &mut self,
        admission: &JsonlFrameAdmission,
    ) -> Result<Option<AdmittedJsonlMessage>, TransportError> {
        loop {
            let available = if let Some(started_at) = self.admitted_frame_started_at {
                let earned = Duration::from_secs(
                    u64::try_from(
                        self.admitted_frame
                            .len()
                            .div_ceil(JSONL_FRAME_MIN_PROGRESS_BYTES_PER_SECOND),
                    )
                    .unwrap_or(u64::MAX),
                );
                let deadline = started_at + JSONL_FRAME_BASE_PROGRESS_TIMEOUT + earned;
                tokio::time::timeout_at(deadline, self.reader.fill_buf())
                    .await
                    .map_err(|_| TransportError::FrameProgressTimeout)??
            } else {
                self.reader.fill_buf().await?
            };

            if available.is_empty() {
                if self.admitted_frame.is_empty() {
                    return Ok(None);
                }
                return self.finish_admitted_frame();
            }

            let newline = available.iter().position(|byte| *byte == b'\n');
            let copied_bytes = newline.unwrap_or(available.len());
            let consumed_bytes = copied_bytes.saturating_add(usize::from(newline.is_some()));
            let target_len = self.admitted_frame.len().saturating_add(copied_bytes);
            if target_len > self.max_frame_bytes {
                self.reset_admitted_frame();
                return Err(TransportError::FrameTooLarge {
                    max_bytes: self.max_frame_bytes,
                    observed_bytes: target_len,
                });
            }

            self.admitted_frame_memory.try_grow(admission, target_len)?;
            self.admitted_frame
                .try_reserve_exact(copied_bytes)
                .map_err(|error| {
                    TransportError::Io(std::io::Error::other(format!(
                        "failed to reserve bounded JSONL frame memory: {error}"
                    )))
                })?;
            self.admitted_frame
                .extend_from_slice(&available[..copied_bytes]);
            self.reader.consume(consumed_bytes);
            if self.admitted_frame_started_at.is_none() && !self.admitted_frame.is_empty() {
                self.admitted_frame_started_at = Some(tokio::time::Instant::now());
            }

            if newline.is_some() {
                if self.admitted_frame.iter().all(u8::is_ascii_whitespace) {
                    self.reset_admitted_frame();
                    continue;
                }
                return self.finish_admitted_frame();
            }
        }
    }

    fn finish_admitted_frame(&mut self) -> Result<Option<AdmittedJsonlMessage>, TransportError> {
        // `mem::take` is essential: `Vec::clear` would retain a maximum-frame
        // allocation on every idle connection after its permit was released.
        let frame = std::mem::take(&mut self.admitted_frame);
        let frame_memory = std::mem::take(&mut self.admitted_frame_memory);
        self.admitted_frame_started_at = None;
        let wire_bytes = frame.len();
        match parse_rpc_message_frame(&frame) {
            Ok(Some(message)) => Ok(Some(AdmittedJsonlMessage {
                message,
                wire_bytes,
                _frame_memory_permit: frame_memory,
            })),
            Ok(None) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn reset_admitted_frame(&mut self) {
        self.admitted_frame = Vec::new();
        self.admitted_frame_memory = JsonlFrameMemoryPermit::default();
        self.admitted_frame_started_at = None;
    }

    /// Read the next JSON-RPC message (request or response) from the transport.
    ///
    /// Returns `Ok(None)` on EOF. Skips empty lines.
    /// Discriminates by field presence: `method` → Request, otherwise → Response.
    pub async fn read_message(&mut self) -> Result<Option<RpcMessage>, TransportError> {
        let mut frame = Vec::new();
        loop {
            frame.clear();

            // Include one extra byte so an unterminated or oversized frame can
            // be distinguished from a frame exactly at the ceiling followed by
            // `\n`. `take` bounds what `read_until` may append to `frame`.
            let read_limit = self.max_frame_bytes.saturating_add(1);
            let mut bounded_reader = (&mut self.reader).take(read_limit as u64);
            let bytes_read = bounded_reader.read_until(b'\n', &mut frame).await?;
            if bytes_read == 0 {
                return Ok(None); // EOF
            }

            if frame.last() == Some(&b'\n') {
                frame.pop();
            }
            if frame.len() > self.max_frame_bytes {
                return Err(TransportError::FrameTooLarge {
                    max_bytes: self.max_frame_bytes,
                    observed_bytes: frame.len(),
                });
            }

            let line = std::str::from_utf8(&frame).map_err(|error| {
                TransportError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
            })?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue; // skip empty lines
            }
            return parse_rpc_message_frame(trimmed.as_bytes());
        }
    }

    /// Write a JSON-RPC request as a single JSONL line (for server→client callbacks).
    pub async fn write_request(&mut self, request: &RpcRequest) -> Result<(), TransportError> {
        let mut bytes = serde_json::to_vec(request)?;
        bytes.push(b'\n');
        self.write_bytes_bounded(bytes).await
    }

    /// Write a JSON-RPC response as a single JSONL line.
    pub async fn write_response(&mut self, response: &RpcResponse) -> Result<(), TransportError> {
        let mut bytes = serde_json::to_vec(response)?;
        bytes.push(b'\n');
        self.write_bytes_bounded(bytes).await
    }

    /// Write a JSON-RPC notification as a single JSONL line.
    pub async fn write_notification(
        &mut self,
        notification: &RpcNotification,
    ) -> Result<(), TransportError> {
        let mut bytes = serde_json::to_vec(notification)?;
        bytes.push(b'\n');
        self.write_bytes_bounded(bytes).await
    }

    async fn write_bytes_bounded(&mut self, bytes: Vec<u8>) -> Result<(), TransportError> {
        tokio::time::timeout(JSONL_WRITE_TIMEOUT, self.writer.write_message(bytes))
            .await
            .map_err(|_| TransportError::WriteTimeout)?
    }
}

fn parse_rpc_message_frame(frame: &[u8]) -> Result<Option<RpcMessage>, TransportError> {
    let line = std::str::from_utf8(frame).map_err(|error| {
        TransportError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // Probe field presence without materializing the params tree, then
    // deserialize directly into the final RawValue-backed envelope.
    let probe: RpcMessageKindProbe = serde_json::from_str(trimmed)?;
    if probe.method.0 {
        let request: RpcRequest = serde_json::from_str(trimmed)?;
        return Ok(Some(RpcMessage::Request(request)));
    }
    let response: InboundRpcResponse = serde_json::from_str(trimmed)?;
    Ok(Some(RpcMessage::Response(response.into())))
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

    fn make_transport_with_limit(
        input: &str,
        max_frame_bytes: usize,
    ) -> JsonlTransport<BufReader<std::io::Cursor<Vec<u8>>>, Vec<u8>> {
        let cursor = std::io::Cursor::new(input.as_bytes().to_vec());
        let reader = BufReader::new(cursor);
        let writer = Vec::<u8>::new();
        JsonlTransport::with_max_frame_bytes(reader, writer, max_frame_bytes)
    }

    /// Unwrap an `RpcMessage` as a `Request`, panicking otherwise.
    fn unwrap_request(msg: RpcMessage) -> RpcRequest {
        match msg {
            RpcMessage::Request(req) => req,
            RpcMessage::Response(_) => panic!("expected Request, got Response"),
        }
    }

    #[test]
    fn message_kind_probe_tracks_presence_without_materializing_params() {
        let request: RpcMessageKindProbe =
            serde_json::from_str(r#"{"method":null,"params":{"payload":"attacker-controlled"}}"#)
                .expect("probe request envelope");
        assert!(request.method.0, "even a null method field is present");

        let response: RpcMessageKindProbe =
            serde_json::from_str(r#"{"result":{"payload":"provider-controlled"}}"#)
                .expect("probe response envelope");
        assert!(!response.method.0);
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
        let notif = RpcNotification::new("event", serde_json::json!({"delta": "hi"})).unwrap();
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
    async fn frame_exactly_at_byte_limit_is_accepted() {
        const LIMIT: usize = 256;
        let prefix =
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"boundary\",\"params\":{\"padding\":\"";
        let suffix = "\"}}";
        let padding = "x".repeat(LIMIT - prefix.len() - suffix.len());
        let frame = format!("{prefix}{padding}{suffix}");
        assert_eq!(frame.len(), LIMIT);

        let mut transport = make_transport_with_limit(&format!("{frame}\n"), LIMIT);
        let request = unwrap_request(transport.read_message().await.unwrap().unwrap());

        assert_eq!(request.method, "boundary");
    }

    #[tokio::test]
    async fn frame_one_byte_over_limit_returns_precise_error() {
        const LIMIT: usize = 128;
        let input = format!("{}\n", "x".repeat(LIMIT + 1));
        let mut transport = make_transport_with_limit(&input, LIMIT);

        let error = transport.read_message().await.unwrap_err();

        assert!(matches!(
            error,
            TransportError::FrameTooLarge {
                max_bytes: LIMIT,
                observed_bytes,
            } if observed_bytes == LIMIT + 1
        ));
    }

    #[tokio::test]
    async fn unterminated_frame_one_byte_over_limit_returns_precise_error() {
        const LIMIT: usize = 128;
        let input = "x".repeat(LIMIT + 1);
        let mut transport = make_transport_with_limit(&input, LIMIT);

        let error = transport.read_message().await.unwrap_err();

        assert!(matches!(
            error,
            TransportError::FrameTooLarge {
                max_bytes: LIMIT,
                observed_bytes,
            } if observed_bytes == LIMIT + 1
        ));
    }

    #[tokio::test]
    async fn multiple_writes_do_not_corrupt() {
        let mut transport = make_transport("");

        let resp1 = RpcResponse::success(Some(RpcId::Num(1)), serde_json::json!("first"));
        let resp2 = RpcResponse::success(Some(RpcId::Num(2)), serde_json::json!("second"));
        let notif = RpcNotification::new("event", serde_json::json!({"n": 3})).unwrap();

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

    #[tokio::test]
    async fn admitted_read_is_cancellation_safe_across_partial_frame() {
        use tokio::io::AsyncWriteExt as _;

        let (mut client, server) = tokio::io::duplex(4096);
        let frame = br#"{"jsonrpc":"2.0","id":1,"method":"session/create","params":{}}"#;
        let split = frame.len() / 2;
        client.write_all(&frame[..split]).await.unwrap();

        let mut transport = JsonlTransport::new(BufReader::new(server), Vec::<u8>::new());
        let admission = JsonlFrameAdmission::new(JSONL_SMALL_FRAME_BYTES * 3, 0);
        {
            let read = transport.read_message_admitted(&admission);
            tokio::pin!(read);
            tokio::select! {
                biased;
                _ = &mut read => panic!("partial frame unexpectedly completed"),
                () = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
        assert_eq!(transport.admitted_frame.len(), split);

        client.write_all(&frame[split..]).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        let admitted = transport
            .read_message_admitted(&admission)
            .await
            .unwrap()
            .expect("completed frame");
        assert_eq!(admitted.wire_bytes, frame.len());
        assert!(matches!(admitted.message, RpcMessage::Request(_)));
    }

    #[tokio::test]
    async fn admitted_frame_moves_allocation_and_releases_process_bytes_on_drop() {
        let frame = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"test\",\"params\":{{\"padding\":\"{}\"}}}}\n",
            "x".repeat(1024)
        );
        let budget = JSONL_SMALL_FRAME_BYTES * JSONL_FRAME_MEMORY_MULTIPLIER;
        let admission = JsonlFrameAdmission::new(budget, 0);
        let mut transport = make_transport(&frame);
        let admitted = transport
            .read_message_admitted(&admission)
            .await
            .unwrap()
            .expect("admitted frame");
        assert_eq!(admission.available_small_bytes(), 0);
        assert_eq!(transport.admitted_frame.capacity(), 0);
        drop(admitted);
        assert_eq!(admission.available_small_bytes(), budget);
    }

    #[test]
    fn reserved_small_pool_admits_128_partial_frames_without_fifo_waiters() {
        let one_frame_charge = JSONL_SMALL_FRAME_BYTES * JSONL_FRAME_MEMORY_MULTIPLIER;
        let admission = JsonlFrameAdmission::new(one_frame_charge * 128, 0);
        let mut permits = Vec::new();
        for _ in 0..128 {
            let mut permit = JsonlFrameMemoryPermit::default();
            permit
                .try_grow(&admission, 1)
                .expect("all advertised idle/slow RPC peers fit the reserved pool");
            permits.push(permit);
        }
        let mut rejected = JsonlFrameMemoryPermit::default();
        assert!(matches!(
            rejected.try_grow(&admission, 1),
            Err(TransportError::FrameAdmissionBackpressured { .. })
        ));
        drop(permits);
        assert_eq!(admission.available_small_bytes(), one_frame_charge * 128);
    }

    #[tokio::test]
    async fn inbound_callback_error_data_is_validated_but_never_materialized() {
        let input = format!(
            "{}\n",
            r#"{"jsonrpc":"2.0","id":"srv-1","error":{"code":-32000,"message":"failed","data":{"nested":[{"attacker":"payload"}]}}}"#
        );
        let mut transport = make_transport(&input);
        let RpcMessage::Response(response) = transport.read_message().await.unwrap().unwrap()
        else {
            panic!("expected response");
        };
        let error = response.error.expect("callback error");
        assert_eq!(error.code, -32000);
        assert!(error.data.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn outbound_write_has_a_hard_deadline() {
        struct PendingWriter;

        #[async_trait]
        impl TransportWriter for PendingWriter {
            async fn write_message(&mut self, _bytes: Vec<u8>) -> Result<(), TransportError> {
                std::future::pending().await
            }
        }

        let reader = BufReader::new(std::io::Cursor::new(Vec::<u8>::new()));
        let mut transport = JsonlTransport::new(reader, PendingWriter);
        let task = tokio::spawn(async move {
            let response = RpcResponse::error(Some(RpcId::Num(1)), -32603, "bounded");
            transport.write_response(&response).await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(JSONL_WRITE_TIMEOUT + Duration::from_secs(1)).await;
        assert!(matches!(
            task.await.unwrap(),
            Err(TransportError::WriteTimeout)
        ));
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

        let notif =
            RpcNotification::new("test/event", serde_json::json!({"key": "value"})).unwrap();
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

        let notif = RpcNotification::new("test", serde_json::json!(null)).unwrap();
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
