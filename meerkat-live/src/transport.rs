//! Live WebSocket transport — bridges browser/test clients to `LiveAdapterHost`.
//!
//! Simple axum WebSocket server. Token-based channel auth.
//!
//! # Wire protocol
//!
//! Connect: `GET {LIVE_WS_PATH}?token={token}&channel={channel_id}&format={format}`
//!
//! - `token` (required): single-use token material minted by the transport
//!   and recorded through generated machine authority before it is returned,
//!   valid for [`TOKEN_TTL`] after issue.
//! - `channel` (required): the channel id the caller intends to bind to.
//!   G38: pinning the token to a specific `(token, channel_id)` tuple
//!   tightens the bearer-token misuse surface — a token leaked from one
//!   `live/open` cannot be replayed against a different channel.
//!   The generated token admission authority rejects channel mismatches.
//! - `format` (required when sending binary frames): negotiates the binary
//!   payload encoding. Currently the only accepted value is `pcm_24k_mono`
//!   (16-bit signed little-endian PCM, 24 kHz, mono). Text-only sessions may
//!   omit this parameter; in that case any binary frame is rejected.
//!
//! Frames:
//!
//! - `Text`: JSON-encoded [`LiveInputChunk`]. Invalid JSON fails closed after
//!   generated close authority accepts the channel close.
//! - `Binary`: raw audio bytes in the negotiated format. If no format was
//!   negotiated, the socket fails closed after generated close authority
//!   accepts the channel close.
//! - `Close`/`Ping`: handled by axum.
//! - Anything else (e.g. unsolicited `Pong`): fails closed after generated
//!   close authority accepts the channel close.
//!
//! Outbound frames are JSON-encoded, public-safe [`LiveAdapterObservation`]
//! values. Byte-bearing canonical user-content transcript events remain
//! internal; clients receive a redacted `user_content_committed` ordering
//! receipt after the host has applied them.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::host::{
    LiveAdapterHost, LiveAdapterHostError, LiveChannelCloseCommitAuthority,
    LiveChannelCloseObservation, LiveChannelId, LiveChannelStatusCommitAuthority,
    LiveChannelStatusObservation, ObservationOutcome, ObservationRouting,
};
use crate::wire_input::{live_input_chunk_decode_rejection, live_input_chunk_from_wire};
use axum::Router;
use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket, close_code};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use meerkat_contracts::{LiveInputChunkWire, WireLiveAdapterObservation};
use meerkat_core::live_adapter::{
    LiveAdapterError, LiveAdapterErrorCode, LiveAdapterObservation, LiveConfigRejectionReason,
    LiveInputChunk,
};
use meerkat_core::types::SessionId;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

pub const LIVE_WS_PATH: &str = "/live/ws";

/// Time-to-live for a minted but unconsumed token.
pub const TOKEN_TTL: Duration = Duration::from_secs(60);

/// Explicit direct-WebSocket aggregate message ceiling. Direct WS supports
/// text and raw PCM audio, not inline images; 2 MiB is already more than forty
/// seconds of negotiated mono PCM while allowing every advertised connection
/// to reserve its full codec allocation within a bounded process pool.
pub const LIVE_WS_MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024;

/// Explicit per-frame ceiling. Valid large inputs may be sent as one frame or
/// fragmented, so the frame and aggregate ceilings intentionally match.
pub const LIVE_WS_MAX_FRAME_BYTES: usize = LIVE_WS_MAX_MESSAGE_BYTES;

/// Maximum upgraded direct-WebSocket connections owned by one process state.
const LIVE_WS_MAX_CONNECTIONS: usize = 32;

/// Listener-level socket/header ownership. This protects the HTTP phase before
/// the router can acquire an upgraded-WebSocket permit. It is deliberately a
/// separate, slightly larger pool so ordinary rejected HTTP requests cannot
/// consume the 32 full raw-message reservations.
const LIVE_WS_MAX_HTTP_CONNECTIONS: usize = 64;
const LIVE_WS_MAX_HTTP_HEADER_BYTES: usize = 32 * 1024;
const LIVE_WS_HTTP_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const LIVE_WS_TOKEN_ADMISSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Process-wide ingress budget. A maximum text frame can coexist with its
/// owned wire strings and decoded payload; three raw-message equivalents bound
/// that peak. Binary ingress requires only the received frame plus one owned
/// `Vec`, and is charged at two equivalents below.
const LIVE_WS_PROCESS_INGRESS_MEMORY_BUDGET_BYTES: usize = LIVE_WS_MAX_MESSAGE_BYTES * 3;
const LIVE_WS_TEXT_MEMORY_MULTIPLIER: usize = 3;
const LIVE_WS_BINARY_MEMORY_MULTIPLIER: usize = 2;
const LIVE_WS_MIN_INGRESS_CHARGE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
enum LiveWsIngressKind {
    Text,
    Binary,
}

#[derive(Clone)]
struct LiveWsAdmission {
    connection_semaphore: Arc<Semaphore>,
    raw_message_semaphore: Arc<Semaphore>,
    ingress_memory_semaphore: Arc<Semaphore>,
    max_connections: usize,
    max_raw_message_bytes: usize,
    max_ingress_bytes: usize,
}

struct LiveWsConnectionPermit {
    _connection_permit: OwnedSemaphorePermit,
    _raw_message_permit: OwnedSemaphorePermit,
}

struct LiveWsIngressPermit {
    _memory_permit: OwnedSemaphorePermit,
}

#[derive(Clone)]
struct LiveWsHttpAdmission {
    connection_semaphore: Arc<Semaphore>,
}

impl LiveWsHttpAdmission {
    fn production() -> Self {
        static PROCESS_ADMISSION: OnceLock<LiveWsHttpAdmission> = OnceLock::new();
        PROCESS_ADMISSION
            .get_or_init(|| Self {
                connection_semaphore: Arc::new(Semaphore::new(LIVE_WS_MAX_HTTP_CONNECTIONS)),
            })
            .clone()
    }

    fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.connection_semaphore)
            .try_acquire_owned()
            .ok()
    }
}

/// Accepted socket wrapper that owns process-global listener capacity through
/// an eventual HTTP upgrade and enforces a bounded header size/deadline before
/// Hyper can wait forever on a partial request.
struct AdmittedLiveWsIo {
    inner: tokio::net::TcpStream,
    _connection_permit: OwnedSemaphorePermit,
    header_deadline: Pin<Box<tokio::time::Sleep>>,
    header_complete: bool,
    header_bytes: usize,
    header_window: u32,
}

impl AdmittedLiveWsIo {
    fn new(inner: tokio::net::TcpStream, connection_permit: OwnedSemaphorePermit) -> Self {
        Self {
            inner,
            _connection_permit: connection_permit,
            header_deadline: Box::pin(tokio::time::sleep(LIVE_WS_HTTP_HEADER_TIMEOUT)),
            header_complete: false,
            header_bytes: 0,
            header_window: 0,
        }
    }

    fn observe_header_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        for byte in bytes {
            self.header_bytes = self.header_bytes.saturating_add(1);
            self.header_window = (self.header_window << 8) | u32::from(*byte);
            if self.header_window == u32::from_be_bytes(*b"\r\n\r\n") {
                self.header_complete = true;
                return Ok(());
            }
            if self.header_bytes >= LIVE_WS_MAX_HTTP_HEADER_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "live WebSocket HTTP header exceeds bounded size",
                ));
            }
        }
        Ok(())
    }
}

impl AsyncRead for AdmittedLiveWsIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.header_complete {
            return Pin::new(&mut self.inner).poll_read(cx, buf);
        }
        if self.header_deadline.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "live WebSocket HTTP header deadline exceeded",
            )));
        }
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        // Bound each pre-header read so Hyper never receives an unbounded
        // attacker-controlled chunk before this wrapper can enforce the cap.
        let mut scratch = [0_u8; 4096];
        let max_read = scratch.len().min(buf.remaining()).min(
            LIVE_WS_MAX_HTTP_HEADER_BYTES
                .saturating_sub(self.header_bytes)
                .saturating_add(1),
        );
        let mut scratch_buf = ReadBuf::new(&mut scratch[..max_read]);
        match Pin::new(&mut self.inner).poll_read(cx, &mut scratch_buf) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => {
                let filled = scratch_buf.filled();
                self.observe_header_bytes(filled)?;
                buf.put_slice(filled);
                Poll::Ready(Ok(()))
            }
        }
    }
}

impl AsyncWrite for AdmittedLiveWsIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

struct LiveWsHttpListener {
    listener: tokio::net::TcpListener,
    admission: LiveWsHttpAdmission,
}

impl LiveWsHttpListener {
    fn new(listener: tokio::net::TcpListener) -> Self {
        Self {
            listener,
            admission: LiveWsHttpAdmission::production(),
        }
    }
}

impl axum::serve::Listener for LiveWsHttpListener {
    type Io = AdmittedLiveWsIo;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, address)) => {
                    let Some(permit) = self.admission.try_acquire() else {
                        tracing::warn!(%address, "rejecting live HTTP socket at process limit");
                        drop(stream);
                        continue;
                    };
                    return (AdmittedLiveWsIo::new(stream, permit), address);
                }
                Err(error) => {
                    tracing::warn!(%error, "live WebSocket listener accept failed");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

#[derive(Debug, thiserror::Error)]
enum LiveWsAdmissionError {
    #[error("live WebSocket connection admission is saturated (maximum {max_connections})")]
    ConnectionBackpressured { max_connections: usize },
    #[error(
        "live WebSocket ingress is backpressured: requested {requested_bytes} bytes from a {max_ingress_bytes}-byte budget"
    )]
    IngressBackpressured {
        max_ingress_bytes: usize,
        requested_bytes: usize,
    },
    #[error(
        "live WebSocket ingress charge {requested_bytes} bytes exceeds the {max_ingress_bytes}-byte budget"
    )]
    IngressTooLarge {
        max_ingress_bytes: usize,
        requested_bytes: usize,
    },
    #[error("live WebSocket admission is closed")]
    Closed,
}

impl LiveWsAdmission {
    fn production() -> Self {
        static PROCESS_ADMISSION: OnceLock<LiveWsAdmission> = OnceLock::new();
        PROCESS_ADMISSION
            .get_or_init(|| {
                Self::new(
                    LIVE_WS_MAX_CONNECTIONS,
                    LIVE_WS_PROCESS_INGRESS_MEMORY_BUDGET_BYTES,
                )
            })
            .clone()
    }

    fn new(max_connections: usize, max_ingress_bytes: usize) -> Self {
        let max_raw_message_bytes = max_connections.saturating_mul(LIVE_WS_MAX_MESSAGE_BYTES);
        Self {
            connection_semaphore: Arc::new(Semaphore::new(max_connections)),
            raw_message_semaphore: Arc::new(Semaphore::new(max_raw_message_bytes)),
            ingress_memory_semaphore: Arc::new(Semaphore::new(max_ingress_bytes)),
            max_connections,
            max_raw_message_bytes,
            max_ingress_bytes,
        }
    }

    fn try_admit_connection(
        &self,
        max_message_bytes: usize,
    ) -> Result<LiveWsConnectionPermit, LiveWsAdmissionError> {
        let connection_permit = Arc::clone(&self.connection_semaphore)
            .try_acquire_owned()
            .map_err(|error| match error {
                tokio::sync::TryAcquireError::NoPermits => {
                    LiveWsAdmissionError::ConnectionBackpressured {
                        max_connections: self.max_connections,
                    }
                }
                tokio::sync::TryAcquireError::Closed => LiveWsAdmissionError::Closed,
            })?;
        if max_message_bytes > self.max_raw_message_bytes {
            return Err(LiveWsAdmissionError::IngressTooLarge {
                max_ingress_bytes: self.max_raw_message_bytes,
                requested_bytes: max_message_bytes,
            });
        }
        let permits = u32::try_from(max_message_bytes).map_err(|_| {
            LiveWsAdmissionError::IngressTooLarge {
                max_ingress_bytes: self.max_raw_message_bytes,
                requested_bytes: max_message_bytes,
            }
        })?;
        let raw_message_permit = Arc::clone(&self.raw_message_semaphore)
            .try_acquire_many_owned(permits)
            .map_err(|error| match error {
                tokio::sync::TryAcquireError::NoPermits => {
                    LiveWsAdmissionError::ConnectionBackpressured {
                        max_connections: self.max_connections,
                    }
                }
                tokio::sync::TryAcquireError::Closed => LiveWsAdmissionError::Closed,
            })?;
        Ok(LiveWsConnectionPermit {
            _connection_permit: connection_permit,
            _raw_message_permit: raw_message_permit,
        })
    }

    fn try_admit_ingress(
        &self,
        kind: LiveWsIngressKind,
        message_bytes: usize,
    ) -> Result<LiveWsIngressPermit, LiveWsAdmissionError> {
        let requested_bytes = live_ws_ingress_memory_charge_bytes(kind, message_bytes);
        if requested_bytes > self.max_ingress_bytes {
            return Err(LiveWsAdmissionError::IngressTooLarge {
                max_ingress_bytes: self.max_ingress_bytes,
                requested_bytes,
            });
        }
        let permits =
            u32::try_from(requested_bytes).map_err(|_| LiveWsAdmissionError::IngressTooLarge {
                max_ingress_bytes: self.max_ingress_bytes,
                requested_bytes,
            })?;
        match Arc::clone(&self.ingress_memory_semaphore).try_acquire_many_owned(permits) {
            Ok(memory_permit) => Ok(LiveWsIngressPermit {
                _memory_permit: memory_permit,
            }),
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                Err(LiveWsAdmissionError::IngressBackpressured {
                    max_ingress_bytes: self.max_ingress_bytes,
                    requested_bytes,
                })
            }
            Err(tokio::sync::TryAcquireError::Closed) => Err(LiveWsAdmissionError::Closed),
        }
    }
}

fn live_ws_ingress_memory_charge_bytes(kind: LiveWsIngressKind, message_bytes: usize) -> usize {
    let multiplier = match kind {
        LiveWsIngressKind::Text => LIVE_WS_TEXT_MEMORY_MULTIPLIER,
        LiveWsIngressKind::Binary => LIVE_WS_BINARY_MEMORY_MULTIPLIER,
    };
    message_bytes
        .max(LIVE_WS_MIN_INGRESS_CHARGE_BYTES)
        .saturating_mul(multiplier)
}

#[derive(Debug, Clone, Copy)]
struct LiveWsTransportLimits {
    max_message_bytes: usize,
    max_frame_bytes: usize,
}

impl Default for LiveWsTransportLimits {
    fn default() -> Self {
        Self {
            max_message_bytes: LIVE_WS_MAX_MESSAGE_BYTES,
            max_frame_bytes: LIVE_WS_MAX_FRAME_BYTES,
        }
    }
}

/// Typed WS error frame sent to clients on transport-level failures.
#[derive(Debug, serde::Serialize)]
struct WsErrorFrame {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

pub(crate) fn scoped_command_rejection_from_host_error(
    error: &LiveAdapterHostError,
) -> Option<LiveAdapterObservation> {
    let LiveAdapterHostError::AdapterError(LiveAdapterError::ProviderError {
        code: code @ LiveAdapterErrorCode::ConfigRejected { .. },
        message,
    }) = error
    else {
        return None;
    };
    Some(LiveAdapterObservation::CommandRejected {
        code: code.clone(),
        message: message.clone(),
    })
}

async fn send_ws_ingress_rejection(
    socket: &mut WebSocket,
    admission: &LiveWsAdmission,
    error: &LiveWsAdmissionError,
) -> bool {
    let reason = LiveConfigRejectionReason::InputBackpressured {
        max_pending_bytes: u64::try_from(admission.max_ingress_bytes).unwrap_or(u64::MAX),
    };
    let observation = LiveAdapterObservation::CommandRejected {
        code: LiveAdapterErrorCode::ConfigRejected { reason },
        message: error.to_string(),
    };
    match serde_json::to_string(&WireLiveAdapterObservation::from(observation)) {
        Ok(json) => socket.send(WsMessage::Text(json.into())).await.is_ok(),
        Err(_) => false,
    }
}

fn direct_websocket_image_rejection() -> LiveAdapterObservation {
    LiveAdapterObservation::CommandRejected {
        code: LiveAdapterErrorCode::ConfigRejected {
            reason: LiveConfigRejectionReason::ImageInputTransportUnsupported {
                transport: "direct_websocket_use_live_send_input_rpc".to_string(),
            },
        },
        message: "Direct WebSocket image frames are unsupported; send the image with JSON-RPC live/send_input and wait for user_content_committed before dependent input".to_string(),
    }
}

/// Negotiated binary frame format for the WS upgrade query string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryFormat {
    /// 16-bit signed little-endian PCM, 24 kHz, mono.
    Pcm24kMono,
}

impl BinaryFormat {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "pcm_24k_mono" => Some(Self::Pcm24kMono),
            _ => None,
        }
    }

    fn sample_rate_hz(self) -> u32 {
        match self {
            Self::Pcm24kMono => 24_000,
        }
    }

    fn channels(self) -> u16 {
        match self {
            Self::Pcm24kMono => 1,
        }
    }
}

/// URL-safe token string with a validated invariant.
///
/// Constructor enforces that every byte is in the URL-safe unreserved
/// alphabet: `A-Z a-z 0-9 - _ . ~`. This means callers can interpolate
/// the value into URL query strings without percent-encoding.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LiveTokenString(String);

impl LiveTokenString {
    /// Construct from an arbitrary string, validating the URL-safe alphabet.
    pub fn new(s: impl Into<String>) -> Result<Self, TokenParseError> {
        let s = s.into();
        if s.is_empty() {
            return Err(TokenParseError::Empty);
        }
        for (idx, b) in s.bytes().enumerate() {
            let ok = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
            if !ok {
                return Err(TokenParseError::InvalidByte { idx, byte: b });
            }
        }
        Ok(Self(s))
    }

    /// Mint a fresh random token. UUIDv4 hex with hyphens — guaranteed URL-safe.
    pub(crate) fn random() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LiveTokenString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Errors that can arise when parsing a `LiveTokenString`.
#[derive(Debug, thiserror::Error)]
pub enum TokenParseError {
    #[error("token is empty")]
    Empty,
    #[error("token contains non-URL-safe byte 0x{byte:02x} at index {idx}")]
    InvalidByte { idx: usize, byte: u8 },
}

/// Generated authority projection for WebSocket token admission failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveWsTokenAdmissionRejection {
    TokenNotFound,
    TokenExpired,
    TokenChannelMismatch,
    TokenAlreadyConsumed,
    ChannelNotBound,
}

impl std::fmt::Display for LiveWsTokenAdmissionRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::TokenNotFound => "token not found",
            Self::TokenExpired => "token expired",
            Self::TokenChannelMismatch => "token bound to a different channel",
            Self::TokenAlreadyConsumed => "token already consumed",
            Self::ChannelNotBound => "channel not bound",
        };
        f.write_str(reason)
    }
}

/// Public error class emitted by generated WebSocket token admission
/// authority. The transport maps this to a WebSocket close only after the
/// generated effect exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveWsTokenAdmissionPublicErrorClass {
    InvalidToken,
}

impl LiveWsTokenAdmissionPublicErrorClass {
    fn as_wire_error(self) -> &'static str {
        match self {
            Self::InvalidToken => "invalid_token",
        }
    }
}

/// Generated authority output for a recorded WebSocket token issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveWsTokenIssue {
    pub token: LiveTokenString,
    pub expires_at_ms: u64,
    pub sequence: u64,
}

/// Generated authority output for WebSocket token admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveWsTokenAdmission {
    pub channel_id: LiveChannelId,
    pub admitted: bool,
    pub rejection: Option<LiveWsTokenAdmissionRejection>,
    pub public_error_class: Option<LiveWsTokenAdmissionPublicErrorClass>,
    pub sequence: u64,
}

/// Typed close feedback sink for live transports.
///
/// Transports own sockets and peer connections only. When a transport observes
/// a close/terminal condition, it must submit the host close observation
/// through this seam so generated machine authority owns active-channel
/// lifecycle cleanup. If generated close cleanup has already committed before
/// the transport drains a staged terminal observation, the transport uses that
/// committed fact and must not mint a second close observation.
#[async_trait::async_trait]
pub trait LiveChannelCloseFeedback: Send + Sync {
    async fn record_live_channel_closed(
        &self,
        channel_id: &LiveChannelId,
        observation: &LiveChannelCloseObservation,
    ) -> Result<LiveChannelCloseCommitAuthority, String>;
}

#[cfg(test)]
pub(crate) struct GeneratedTestMachineLiveChannelCloseFeedback {
    host: Arc<LiveAdapterHost>,
}

#[cfg(test)]
impl GeneratedTestMachineLiveChannelCloseFeedback {
    pub(crate) fn new(host: Arc<LiveAdapterHost>) -> Self {
        Self { host }
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl LiveChannelCloseFeedback for GeneratedTestMachineLiveChannelCloseFeedback {
    async fn record_live_channel_closed(
        &self,
        channel_id: &LiveChannelId,
        observation: &LiveChannelCloseObservation,
    ) -> Result<LiveChannelCloseCommitAuthority, String> {
        if observation.channel_id() != channel_id.as_str() {
            return Err(format!(
                "generated test close feedback channel mismatch: observed {}, requested {}",
                observation.channel_id(),
                channel_id
            ));
        }
        self.host
            .close_commit_authority_from_generated_test_machine(observation)
            .await
            .map_err(|err| err.to_string())
    }
}

/// Typed status feedback sink for live transports.
///
/// Transports observe provider status, but generated MeerkatMachine authority
/// decides when that observation can become the host fact used by command
/// admission and public status projection.
#[async_trait::async_trait]
pub trait LiveChannelStatusFeedback: Send + Sync {
    async fn record_live_channel_status(
        &self,
        channel_id: &LiveChannelId,
        observation: &LiveChannelStatusObservation,
    ) -> Result<LiveChannelStatusCommitAuthority, String>;
}

#[cfg(test)]
pub(crate) struct GeneratedTestMachineLiveChannelStatusFeedback {
    host: Arc<LiveAdapterHost>,
}

#[cfg(test)]
impl GeneratedTestMachineLiveChannelStatusFeedback {
    pub(crate) fn new(host: Arc<LiveAdapterHost>) -> Self {
        Self { host }
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl LiveChannelStatusFeedback for GeneratedTestMachineLiveChannelStatusFeedback {
    async fn record_live_channel_status(
        &self,
        channel_id: &LiveChannelId,
        observation: &LiveChannelStatusObservation,
    ) -> Result<LiveChannelStatusCommitAuthority, String> {
        if observation.channel_id() != channel_id.as_str() {
            return Err(format!(
                "generated test status feedback channel mismatch: observed {}, requested {}",
                observation.channel_id(),
                channel_id
            ));
        }
        self.host
            .status_commit_authority_from_generated_test_machine(observation)
            .await
            .map_err(|err| err.to_string())
    }
}

/// Typed token authority seam for live WebSocket upgrades.
///
/// Production implementations route these calls into generated MeerkatMachine
/// inputs/effects. The WebSocket transport owns sockets only; it does not own
/// token binding, expiry, or consume/admission facts.
#[async_trait::async_trait]
pub trait LiveWsTokenAuthority: Send + Sync {
    async fn record_live_ws_token_issued(
        &self,
        session_id: &SessionId,
        channel_id: &LiveChannelId,
        token: &LiveTokenString,
        issued_at_ms: u64,
        ttl_ms: u64,
    ) -> Result<LiveWsTokenIssue, String>;

    async fn resolve_live_ws_token_admission(
        &self,
        channel_id: &LiveChannelId,
        token: &str,
        observed_at_ms: u64,
    ) -> Result<LiveWsTokenAdmission, String>;
}

/// Shared state for the live WebSocket transport.
///
/// Composable — any surface creates one of these with a shared
/// `LiveAdapterHost` and either mounts the router or starts a listener.
pub struct LiveWsState {
    host: Arc<LiveAdapterHost>,
    close_feedback: Arc<dyn LiveChannelCloseFeedback>,
    status_feedback: Arc<dyn LiveChannelStatusFeedback>,
    token_authority: Arc<dyn LiveWsTokenAuthority>,
    token_ttl: Duration,
    /// Shared across every production state, router, listener, and connection
    /// through the process singleton. Tests may inject narrower isolated or
    /// explicitly shared capacities without weakening production defaults.
    admission: LiveWsAdmission,
    transport_limits: LiveWsTransportLimits,
}

impl LiveWsState {
    pub fn new(
        host: Arc<LiveAdapterHost>,
        close_feedback: Arc<dyn LiveChannelCloseFeedback>,
        status_feedback: Arc<dyn LiveChannelStatusFeedback>,
        token_authority: Arc<dyn LiveWsTokenAuthority>,
    ) -> Self {
        Self::with_token_ttl(
            host,
            close_feedback,
            status_feedback,
            token_authority,
            TOKEN_TTL,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_token_authority(
        host: Arc<LiveAdapterHost>,
        token_authority: Arc<dyn LiveWsTokenAuthority>,
    ) -> Self {
        let close_feedback = Arc::new(GeneratedTestMachineLiveChannelCloseFeedback::new(
            Arc::clone(&host),
        ));
        let status_feedback = Arc::new(GeneratedTestMachineLiveChannelStatusFeedback::new(
            Arc::clone(&host),
        ));
        Self::new(host, close_feedback, status_feedback, token_authority)
    }

    /// Construct with a custom token TTL — primarily for tests.
    pub fn with_token_ttl(
        host: Arc<LiveAdapterHost>,
        close_feedback: Arc<dyn LiveChannelCloseFeedback>,
        status_feedback: Arc<dyn LiveChannelStatusFeedback>,
        token_authority: Arc<dyn LiveWsTokenAuthority>,
        token_ttl: Duration,
    ) -> Self {
        Self {
            host,
            close_feedback,
            status_feedback,
            token_authority,
            token_ttl,
            admission: LiveWsAdmission::production(),
            transport_limits: LiveWsTransportLimits::default(),
        }
    }

    #[cfg(test)]
    fn with_test_transport_admission(
        mut self,
        max_connections: usize,
        max_ingress_bytes: usize,
        max_message_bytes: usize,
        max_frame_bytes: usize,
    ) -> Self {
        self.admission = LiveWsAdmission::new(max_connections, max_ingress_bytes);
        self.transport_limits = LiveWsTransportLimits {
            max_message_bytes,
            max_frame_bytes,
        };
        self
    }

    #[cfg(test)]
    fn with_test_shared_transport_admission(
        mut self,
        admission: LiveWsAdmission,
        max_message_bytes: usize,
        max_frame_bytes: usize,
    ) -> Self {
        self.admission = admission;
        self.transport_limits = LiveWsTransportLimits {
            max_message_bytes,
            max_frame_bytes,
        };
        self
    }

    pub fn host(&self) -> &Arc<LiveAdapterHost> {
        &self.host
    }

    async fn close_channel_with_generated_feedback(&self, channel_id: &LiveChannelId) -> bool {
        // Runtime-initiated terminal paths can commit generated close authority
        // before the WS pump drains the staged terminal observation. In that
        // case the committed host status is the generated close handoff result;
        // asking close feedback for a second decision would route through an
        // already-cleared active binding.
        match self.host.generated_close_has_committed(channel_id).await {
            Ok(true) => return true,
            Ok(false) => {}
            Err(LiveAdapterHostError::ChannelNotFound(_)) => return false,
            Err(err) => {
                tracing::warn!(
                    channel = %channel_id,
                    error = %err,
                    "live transport generated close status check failed"
                );
                return false;
            }
        }

        let observation = match self
            .host
            .reserve_channel_close_observation(channel_id)
            .await
        {
            Ok(observation) => observation,
            Err(LiveAdapterHostError::ChannelNotFound(_)) => return false,
            Err(err) => {
                tracing::warn!(
                    channel = %channel_id,
                    error = %err,
                    "live transport host close observation reservation failed"
                );
                return false;
            }
        };
        if let Err(err) = self.host.prepare_channel_physical_close(&observation).await {
            tracing::warn!(
                channel = %channel_id,
                error = %err,
                "live transport physical adapter close failed before generated feedback"
            );
            return false;
        }
        let authority = match self
            .close_feedback
            .record_live_channel_closed(channel_id, &observation)
            .await
        {
            Ok(authority) => authority,
            Err(err) => {
                tracing::warn!(
                    channel = %channel_id,
                    error = %err,
                    "generated live close feedback rejected transport close"
                );
                return false;
            }
        };
        if let Err(err) = self
            .host
            .commit_channel_close_observation(&observation, &authority)
            .await
        {
            tracing::warn!(
                channel = %channel_id,
                error = %err,
                "live transport host close commit failed after generated feedback"
            );
            return false;
        }
        true
    }

    async fn commit_status_with_generated_feedback(
        &self,
        channel_id: &LiveChannelId,
        observation: &LiveAdapterObservation,
    ) -> bool {
        let status = match LiveAdapterHost::classify_observation(observation) {
            ObservationRouting::UpdateStatus(status) => status,
            _ => return true,
        };
        if status.is_terminal() {
            return true;
        }

        let status_observation = match self
            .host
            .reserve_channel_status_observation(channel_id, status)
            .await
        {
            Ok(observation) => observation,
            Err(LiveAdapterHostError::ChannelNotFound(_)) => return false,
            Err(err) => {
                tracing::warn!(
                    channel = %channel_id,
                    error = %err,
                    "live transport host status observation reservation failed"
                );
                return false;
            }
        };
        let authority = match self
            .status_feedback
            .record_live_channel_status(channel_id, &status_observation)
            .await
        {
            Ok(authority) => authority,
            Err(err) => {
                tracing::warn!(
                    channel = %channel_id,
                    error = %err,
                    "generated live status feedback rejected transport status"
                );
                return false;
            }
        };
        if let Err(err) = self
            .host
            .commit_channel_status_observation(&status_observation, &authority)
            .await
        {
            tracing::warn!(
                channel = %channel_id,
                error = %err,
                "live transport host status commit failed after generated feedback"
            );
            return false;
        }
        true
    }

    pub fn token_ttl(&self) -> Duration {
        self.token_ttl
    }

    /// Mint random token material and record the authoritative binding in
    /// MeerkatMachine before returning it.
    pub async fn mint_token(
        &self,
        session_id: &SessionId,
        channel_id: LiveChannelId,
    ) -> Result<LiveTokenString, String> {
        let token = LiveTokenString::random();
        let issued_at_ms = live_ws_now_ms()?;
        let ttl_ms = live_ws_duration_ms(self.token_ttl)?;
        let issue = self
            .token_authority
            .record_live_ws_token_issued(session_id, &channel_id, &token, issued_at_ms, ttl_ms)
            .await?;
        Ok(issue.token)
    }

    async fn resolve_token_admission(
        &self,
        token: &str,
        expected_channel: &LiveChannelId,
    ) -> Result<LiveWsTokenAdmission, String> {
        let observed_at_ms = live_ws_now_ms()?;
        tokio::time::timeout(
            LIVE_WS_TOKEN_ADMISSION_TIMEOUT,
            self.token_authority.resolve_live_ws_token_admission(
                expected_channel,
                token,
                observed_at_ms,
            ),
        )
        .await
        .map_err(|_| "live WebSocket token admission deadline exceeded".to_string())?
    }
}

fn live_ws_now_ms() -> Result<u64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system time is before Unix epoch: {err}"))?;
    u64::try_from(elapsed.as_millis())
        .map_err(|_| "system time milliseconds overflow u64".to_string())
}

fn live_ws_duration_ms(duration: Duration) -> Result<u64, String> {
    u64::try_from(duration.as_millis())
        .map_err(|_| "live WebSocket token TTL milliseconds overflow u64".to_string())
}

#[derive(serde::Deserialize)]
pub struct WsConnectParams {
    pub token: String,
    /// Channel id this token claims to bind to. G38: required so
    /// generated token admission can verify the token was minted for the same
    /// channel the client is attempting to attach.
    pub channel: String,
    /// Negotiated binary format. Optional — required only if the client will
    /// send binary frames.
    #[serde(default)]
    pub format: Option<String>,
}

/// Build an axum `Router` with the live WebSocket endpoint.
///
/// Mount this on any axum app:
/// ```ignore
/// let app = your_app.merge(live_ws_router(state));
/// ```
pub fn live_ws_router(state: Arc<LiveWsState>) -> Router {
    Router::new()
        .route(LIVE_WS_PATH, get(ws_upgrade))
        .layer(axum::middleware::map_response(close_non_upgraded_http))
        .with_state(state)
}

async fn close_non_upgraded_http(
    mut response: axum::response::Response,
) -> axum::response::Response {
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        response.headers_mut().insert(
            axum::http::header::CONNECTION,
            axum::http::HeaderValue::from_static("close"),
        );
    }
    response
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    Query(params): Query<WsConnectParams>,
    State(state): State<Arc<LiveWsState>>,
) -> axum::response::Response {
    let WsConnectParams {
        token,
        channel,
        format,
    } = params;
    let binary_format = format.as_deref().and_then(BinaryFormat::parse);
    // Note: an explicitly-supplied but unparseable format is treated the same
    // as "no format negotiated" — the upgrade still succeeds (text frames
    // remain usable) but binary frames will be rejected by the handler with a
    // close frame, surfacing the misconfiguration cleanly.
    let expected_channel = LiveChannelId::new(channel);
    let limits = state.transport_limits;
    // Reserve one complete raw codec message before upgrade. Tungstenite may
    // allocate while assembling fragments, before the application receives a
    // `Message`; per-upgrade custody is therefore the first truthful owner.
    let connection_permit = match state
        .admission
        .try_admit_connection(limits.max_message_bytes)
    {
        Ok(permit) => permit,
        Err(error) => {
            tracing::warn!(error = %error, "rejecting live WebSocket upgrade at process connection limit");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "live WebSocket connection admission is saturated",
            )
                .into_response();
        }
    };
    ws.max_message_size(limits.max_message_bytes)
        .max_frame_size(limits.max_frame_bytes)
        .on_upgrade(move |socket| {
            handle_live_socket(
                socket,
                token,
                expected_channel,
                binary_format,
                state,
                connection_permit,
            )
        })
        .into_response()
}

async fn close_with(socket: &mut WebSocket, code: u16, reason: &str) {
    let _ = socket
        .send(WsMessage::Close(Some(CloseFrame {
            code,
            reason: reason.to_owned().into(),
        })))
        .await;
}

fn observation_requires_generated_close(observation: &LiveAdapterObservation) -> bool {
    match observation {
        LiveAdapterObservation::Error { .. } => true,
        LiveAdapterObservation::StatusChanged { status } => status.is_terminal(),
        _ => false,
    }
}

/// Decide whether an adapter observation is safe and meaningful on a public
/// live transport.
///
/// `RealtimeTranscriptEvent::UserContentFinal` is an internal canonical-state
/// command and may contain inline image bytes. The host must apply it, but WS
/// and WebRTC clients receive only the host-synthesized
/// `ObservationOutcome::UserContentCommitted` receipt. Raw adapter receipts
/// are filtered because provider acknowledgement is not durable commit
/// authority. Receipt visibility is therefore an acknowledgement that
/// canonical persistence has completed. WebRTC
/// clients can therefore use it as the required barrier before sending RTP
/// audio that depends on the image context.
pub(crate) fn should_publish_observation(observation: &LiveAdapterObservation) -> bool {
    !matches!(
        observation,
        LiveAdapterObservation::Error { .. }
            | LiveAdapterObservation::UserContentCommitted { .. }
            | LiveAdapterObservation::RealtimeTranscript {
                event: meerkat_core::RealtimeTranscriptEvent::UserContentFinal { .. }
            }
    )
}

async fn handle_live_socket(
    mut socket: WebSocket,
    token: String,
    expected_channel: LiveChannelId,
    binary_format: Option<BinaryFormat>,
    state: Arc<LiveWsState>,
    _connection_permit: LiveWsConnectionPermit,
) {
    let admission = match state
        .resolve_token_admission(&token, &expected_channel)
        .await
    {
        Ok(admission) => admission,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "live WebSocket token authority unavailable; failing closed without public token class"
            );
            drop(socket);
            return;
        }
    };
    if !admission.admitted {
        let Some(public_error_class) = admission.public_error_class else {
            tracing::warn!(
                channel = %expected_channel,
                sequence = admission.sequence,
                "live WebSocket token admission rejected without generated public class; failing closed"
            );
            drop(socket);
            return;
        };
        let Some(rejection) = admission.rejection else {
            tracing::warn!(
                channel = %expected_channel,
                sequence = admission.sequence,
                "live WebSocket token admission rejected without generated rejection reason; failing closed"
            );
            drop(socket);
            return;
        };
        let public_error = public_error_class.as_wire_error();
        let reason = rejection.to_string();
        let err_json = serde_json::to_string(&WsErrorFrame {
            error: public_error.into(),
            reason: Some(reason),
        })
        .unwrap_or_default();
        let _ = socket.send(WsMessage::Text(err_json.into())).await;
        close_with(&mut socket, close_code::POLICY, public_error).await;
        return;
    }
    let channel_id = admission.channel_id;

    tracing::info!(channel = %channel_id, "live WebSocket connected");

    // G2 (P1): the observation future MUST be pinned across `select!`
    // iterations. Recreating it inline as `next_observation_raw(&channel_id)
    // => …` made it cancel-on-loss, so under continuous inbound mic audio
    // the recv arm wins every iteration and the observation future never
    // accumulates enough poll progress to resolve — a starvation regression
    // distinct from `biased;` ordering. We box+pin once and re-arm only
    // after an observation is consumed (or an error tears the loop down).
    let mut observation_fut = Box::pin(state.host.next_observation_raw(&channel_id));
    let mut close_feedback_recorded = false;

    loop {
        // No `biased;` — fair scheduling prevents continuous mic-audio inbound
        // frames from starving the observation arm (assistant output, tool
        // observations, terminal close).
        tokio::select! {
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        // The WebSocket codec has already bounded the one raw
                        // message allocation. Reserve the conservative process
                        // peak before serde can allocate owned wire strings or
                        // base64 decode can allocate the payload. Admission is
                        // non-blocking; saturation is a scoped command rejection
                        // and this socket remains available for retry.
                        let _ingress_permit =
                            match state.admission.try_admit_ingress(
                                LiveWsIngressKind::Text,
                                text.len(),
                            ) {
                                Ok(permit) => permit,
                                Err(error) => {
                                    if !send_ws_ingress_rejection(
                                        &mut socket,
                                        &state.admission,
                                        &error,
                                    )
                                    .await
                                    {
                                        break;
                                    }
                                    continue;
                                }
                            };
                        // D243: deserialize the wire-contract `LiveInputChunkWire`
                        // and run the shared `live_input_chunk_from_wire`
                        // conversion owner (`crate::wire_input`) that the RPC
                        // `live/send_input` path also uses for text/audio. Inline
                        // images are rejected at this transport boundary and must
                        // use JSON-RPC; malformed supported envelopes fail closed.
                        let chunk = match serde_json::from_str::<LiveInputChunkWire>(text.as_str()) {
                            Ok(wire @ LiveInputChunkWire::Image { .. }) => {
                                let observation = direct_websocket_image_rejection();
                                if let Ok(json) = serde_json::to_string(
                                    &WireLiveAdapterObservation::from(observation),
                                ) {
                                    let _ = socket.send(WsMessage::Text(json.into())).await;
                                }
                                drop(wire);
                                continue;
                            }
                            Ok(wire) => match live_input_chunk_from_wire(wire) {
                                Ok(chunk) => Some(chunk),
                                Err(convert_err) => {
                                    if let Some(code) =
                                        live_input_chunk_decode_rejection(&convert_err)
                                    {
                                        let observation =
                                            LiveAdapterObservation::CommandRejected {
                                                code,
                                                message: convert_err.to_string(),
                                            };
                                        if let Ok(json) = serde_json::to_string(
                                            &WireLiveAdapterObservation::from(observation),
                                        ) {
                                            let _ = socket.send(WsMessage::Text(json.into())).await;
                                        }
                                        continue;
                                    }
                                    tracing::warn!(
                                        channel = %channel_id,
                                        error = %convert_err,
                                        "WS input chunk failed wire validation; closing"
                                    );
                                    None
                                }
                            },
                            Err(parse_err) => {
                                tracing::warn!(
                                    channel = %channel_id,
                                    error = %parse_err,
                                    "invalid WS text frame; closing"
                                );
                                None
                            }
                        };
                        match chunk {
                            Some(chunk) => {
                                if let Err(err) = state.host.send_input(&channel_id, chunk).await {
                                    tracing::warn!(channel = %channel_id, error = %err, "send_input failed");
                                    if let Some(observation) =
                                        scoped_command_rejection_from_host_error(&err)
                                    {
                                        if let Ok(json) = serde_json::to_string(
                                            &WireLiveAdapterObservation::from(observation),
                                        ) {
                                            let _ = socket.send(WsMessage::Text(json.into())).await;
                                        }
                                        continue;
                                    }
                                    // D153: populate the typed `reason` with the
                                    // host error's stable class so clients route
                                    // on the code, not the prose `error`.
                                    let err_json = serde_json::to_string(&WsErrorFrame {
                                        error: err.to_string(),
                                        reason: Some(err.reason_code().to_string()),
                                    }).unwrap_or_default();
                                    let _ = socket.send(WsMessage::Text(err_json.into())).await;
                                }
                            }
                            None => {
                                if !state.close_channel_with_generated_feedback(&channel_id).await {
                                    break;
                                }
                                close_feedback_recorded = true;
                                break;
                            }
                        }
                    }
                    Some(Ok(WsMessage::Binary(data))) => {
                        let Some(fmt) = binary_format else {
                            tracing::warn!(
                                channel = %channel_id,
                                "binary frame received before format negotiation; closing"
                            );
                            if !state.close_channel_with_generated_feedback(&channel_id).await {
                                break;
                            }
                            close_feedback_recorded = true;
                            break;
                        };
                        // Reserve the raw-frame + owned-Vec peak before
                        // `to_vec` copies any bytes. Like text admission, this
                        // uses `try_acquire` and keeps the peer alive on pressure.
                        let _ingress_permit =
                            match state.admission.try_admit_ingress(
                                LiveWsIngressKind::Binary,
                                data.len(),
                            ) {
                                Ok(permit) => permit,
                                Err(error) => {
                                    if !send_ws_ingress_rejection(
                                        &mut socket,
                                        &state.admission,
                                        &error,
                                    )
                                    .await
                                    {
                                        break;
                                    }
                                    continue;
                                }
                            };
                        let chunk = LiveInputChunk::Audio {
                            data: data.to_vec(),
                            sample_rate_hz: fmt.sample_rate_hz(),
                            channels: fmt.channels(),
                        };
                        if let Err(err) = state.host.send_input(&channel_id, chunk).await {
                            tracing::warn!(channel = %channel_id, error = %err, "binary send_input failed");
                            if let Some(observation) =
                                scoped_command_rejection_from_host_error(&err)
                            {
                                if let Ok(json) = serde_json::to_string(
                                    &WireLiveAdapterObservation::from(observation),
                                ) {
                                    let _ = socket.send(WsMessage::Text(json.into())).await;
                                }
                                continue;
                            }
                            // D153: populate the typed `reason` with the host
                            // error's stable class (see text-frame path above).
                            let err_json = serde_json::to_string(&WsErrorFrame {
                                error: err.to_string(),
                                reason: Some(err.reason_code().to_string()),
                            }).unwrap_or_default();
                            let _ = socket.send(WsMessage::Text(err_json.into())).await;
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    Some(Ok(WsMessage::Ping(_))) => {
                        // axum responds to Ping with Pong automatically.
                    }
                    Some(Ok(other)) => {
                        tracing::warn!(
                            channel = %channel_id,
                            kind = ?std::mem::discriminant(&other),
                            "unsupported WS frame; closing"
                        );
                        if !state.close_channel_with_generated_feedback(&channel_id).await {
                            break;
                        }
                        close_feedback_recorded = true;
                        break;
                    }
                    Some(Err(err)) => {
                        tracing::warn!(channel = %channel_id, error = %err, "WS recv error");
                        break;
                    }
                }
            }

            observation = &mut observation_fut => {
                // Re-arm the pinned observation future for the next loop
                // iteration before processing this one — the new future
                // must already be pinned by the time the next select!
                // iteration polls it.
                observation_fut = Box::pin(state.host.next_observation_raw(&channel_id));
                // Wave-3 RPC pump migration: split the convenience wrapper
                // into `next_observation_raw` + `apply_observation` so the
                // pump can react to the typed `ObservationOutcome` after
                // generated close authority has accepted terminal facts.
                match observation {
                    Ok(Some(obs)) => {
                        // R6-2 (P2): route the WS write through the typed
                        // wire mirror at the public boundary so future core
                        // variants surface as `observation: "unknown"`
                        // (R3-6's fail-loud sentinel) rather than leaking
                        // raw new tags at this seam. The conversion is
                        // total (every core variant has an explicit arm)
                        // and byte-compatible for known variants — see
                        // `wire_live_adapter_observation_byte_compatible_with_core_for_audio_chunk`
                        // / `_command_rejected` in
                        // `meerkat-contracts/src/wire/live.rs`.
                        let close_observation = observation_requires_generated_close(&obs);
                        let publish_observation = should_publish_observation(&obs);

                        if close_observation {
                            if !state.close_channel_with_generated_feedback(&channel_id).await {
                                break;
                            }
                            close_feedback_recorded = true;
                        } else if !state
                            .commit_status_with_generated_feedback(&channel_id, &obs)
                            .await
                        {
                            break;
                        }

                        // Apply to canonical state and inspect the typed outcome
                        // only after generated close authority has accepted any
                        // terminal observation. Public WS frames are sent after
                        // this point so terminality is not observable before the
                        // machine-owned close fact is accepted.
                        let outcome = match state.host.apply_observation(&channel_id, &obs).await {
                            Ok(outcome) => outcome,
                            Err(err) => {
                                tracing::warn!(
                                    channel = %channel_id,
                                    error = %err,
                                    "apply_observation failed; closing channel before public observation"
                                );
                                break;
                            }
                        };

                        let send_ok = if publish_observation {
                            // Convert only public observations. Besides
                            // preventing serialization, this avoids cloning
                            // inline image bytes from the internal canonical
                            // user-content event into a wire value at all.
                            match serde_json::to_string(&WireLiveAdapterObservation::from(
                                obs.clone(),
                            )) {
                                Ok(json) => socket.send(WsMessage::Text(json.into())).await.is_ok(),
                                Err(error) => {
                                    tracing::warn!(
                                        channel = %channel_id,
                                        error = %error,
                                        "public live observation serialization failed; closing"
                                    );
                                    false
                                }
                            }
                        } else {
                            true
                        };

                        if !send_ok {
                            break;
                        }

                        match outcome {
                            ObservationOutcome::UserContentCommitted { observation } => {
                                let send_ok = match serde_json::to_string(
                                    &WireLiveAdapterObservation::from(observation),
                                ) {
                                    Ok(json) => socket
                                        .send(WsMessage::Text(json.into()))
                                        .await
                                        .is_ok(),
                                    Err(_) => false,
                                };
                                if !send_ok {
                                    break;
                                }
                            }
                            ObservationOutcome::Terminal { code } => {
                                tracing::info!(
                                    channel = %channel_id,
                                    ?code,
                                    "live WebSocket reached terminal observation after generated close authority"
                                );
                                break;
                            }
                            // R5-9: typed scoped command rejection. The
                            // JSON observation has already been forwarded
                            // to the client above (the same forward path
                            // every observation takes); we deliberately
                            // do NOT close the WS — the channel survives
                            // so the client can retry with a supported
                            // input shape. Logged at info level so the
                            // operator can see scoped failures without
                            // an alarm.
                            ObservationOutcome::CommandRejected { code, message } => {
                                tracing::info!(
                                    channel = %channel_id,
                                    ?code,
                                    %message,
                                    "live command rejected; channel remains open"
                                );
                            }
                            _ => {}
                        }
                        if close_observation {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }
    }

    tracing::info!(channel = %channel_id, "live WebSocket disconnected");
    if !close_feedback_recorded {
        state
            .close_channel_with_generated_feedback(&channel_id)
            .await;
    }
}

/// Start the live WebSocket listener on a pre-bound TCP listener.
pub async fn serve_live_ws_listener(
    listener: tokio::net::TcpListener,
    state: Arc<LiveWsState>,
) -> Result<(), std::io::Error> {
    let app = live_ws_router(state);
    axum::serve(LiveWsHttpListener::new(listener), app).await
}

// TODO(wire-handlers): `mint_token` now returns `LiveTokenString` (URL-safe
// invariant). The caller in `meerkat-rpc/src/handlers/live.rs` interpolates
// it via `format!("…?token={token}")`; that continues to work because
// `LiveTokenString` implements `Display`. When wire-handlers next touches
// that file, prefer storing the typed value end-to-end (e.g. in
// `LiveTransportBootstrap::Websocket { token: LiveTokenString, … }`) rather
// than collapsing to `String` immediately.

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::host::{
        LiveProjectionError, LiveProjectionSink, LiveTranscriptIdentity, NoOpProjectionSink,
    };
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn raw_user_content_event_and_adapter_receipt_are_filtered() {
        use meerkat_core::types::{ContentBlock, ImageData};

        let internal = LiveAdapterObservation::RealtimeTranscript {
            event: meerkat_core::RealtimeTranscriptEvent::UserContentFinal {
                idempotency_key: "image-request-1".into(),
                item_id: "item_image".into(),
                previous_item_id: None,
                content_index: 0,
                content: vec![ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: ImageData::Inline {
                        data: "sensitive-base64-payload".into(),
                    },
                }],
            },
        };
        let receipt = LiveAdapterObservation::UserContentCommitted {
            idempotency_key: "image-request-1".into(),
            item_id: "item_image".into(),
            previous_item_id: None,
            content_index: 0,
            media_type: "image/png".into(),
        };

        assert!(!should_publish_observation(&internal));
        assert!(!should_publish_observation(&receipt));
        let public_json = serde_json::to_string(&WireLiveAdapterObservation::from(receipt))
            .expect("receipt should serialize");
        assert!(public_json.contains("user_content_committed"));
        assert!(!public_json.contains("sensitive-base64-payload"));
    }

    type IssuedLiveWsToken = (SessionId, LiveChannelId, LiveTokenString, u64, u64);

    #[derive(Default)]
    struct ScriptedLiveWsTokenAuthority {
        issued: tokio::sync::Mutex<Vec<IssuedLiveWsToken>>,
        admissions: tokio::sync::Mutex<VecDeque<LiveWsTokenAdmission>>,
    }

    impl ScriptedLiveWsTokenAuthority {
        fn new(admissions: impl IntoIterator<Item = LiveWsTokenAdmission>) -> Arc<Self> {
            Arc::new(Self {
                issued: tokio::sync::Mutex::new(Vec::new()),
                admissions: tokio::sync::Mutex::new(admissions.into_iter().collect()),
            })
        }

        fn admitting(channel_id: LiveChannelId) -> Arc<Self> {
            Self::new([generated_admission(channel_id)])
        }

        fn rejecting(
            channel_id: LiveChannelId,
            rejection: LiveWsTokenAdmissionRejection,
        ) -> Arc<Self> {
            Self::new([generated_rejection(channel_id, rejection)])
        }
    }

    #[async_trait::async_trait]
    impl LiveWsTokenAuthority for ScriptedLiveWsTokenAuthority {
        async fn record_live_ws_token_issued(
            &self,
            session_id: &SessionId,
            channel_id: &LiveChannelId,
            token: &LiveTokenString,
            issued_at_ms: u64,
            ttl_ms: u64,
        ) -> Result<LiveWsTokenIssue, String> {
            self.issued.lock().await.push((
                session_id.clone(),
                channel_id.clone(),
                token.clone(),
                issued_at_ms,
                ttl_ms,
            ));
            Ok(LiveWsTokenIssue {
                token: token.clone(),
                expires_at_ms: 0,
                sequence: 0,
            })
        }

        async fn resolve_live_ws_token_admission(
            &self,
            _channel_id: &LiveChannelId,
            _token: &str,
            _observed_at_ms: u64,
        ) -> Result<LiveWsTokenAdmission, String> {
            self.admissions
                .lock()
                .await
                .pop_front()
                .ok_or_else(|| "missing scripted generated token admission".to_string())
        }
    }

    #[derive(Default)]
    struct RejectingCloseFeedback {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LiveChannelCloseFeedback for RejectingCloseFeedback {
        async fn record_live_channel_closed(
            &self,
            _channel_id: &LiveChannelId,
            _observation: &LiveChannelCloseObservation,
        ) -> Result<LiveChannelCloseCommitAuthority, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err("active channel owner should not be requested after generated close commit".into())
        }
    }

    #[derive(Default)]
    struct TerminalRecordingProjectionSink {
        reject_user_transcript: std::sync::atomic::AtomicBool,
        terminal_errors: StdMutex<
            Vec<(
                SessionId,
                meerkat_core::live_adapter::LiveAdapterErrorCode,
                String,
            )>,
        >,
    }

    #[async_trait::async_trait]
    impl LiveProjectionSink for TerminalRecordingProjectionSink {
        async fn append_user_transcript(
            &self,
            _session_id: &SessionId,
            _text: &str,
            _identity: LiveTranscriptIdentity<'_>,
        ) -> Result<(), LiveProjectionError> {
            if self.reject_user_transcript.load(Ordering::SeqCst) {
                return Err(LiveProjectionError::Rejected(
                    "scripted canonical projection rejection".to_string(),
                ));
            }
            Ok(())
        }

        async fn append_assistant_text_delta(
            &self,
            _session_id: &SessionId,
            _delta: &str,
            _identity: LiveTranscriptIdentity<'_>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn append_assistant_transcript_delta(
            &self,
            _session_id: &SessionId,
            _delta: &str,
            _identity: LiveTranscriptIdentity<'_>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn append_assistant_text_final(
            &self,
            _session_id: &SessionId,
            _text: &str,
            _identity: LiveTranscriptIdentity<'_>,
            _stop_reason: meerkat_core::types::StopReason,
            _usage: meerkat_core::types::Usage,
            _response_id: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn append_assistant_transcript_final(
            &self,
            _session_id: &SessionId,
            _text: &str,
            _identity: LiveTranscriptIdentity<'_>,
            _stop_reason: meerkat_core::types::StopReason,
            _usage: meerkat_core::types::Usage,
            _response_id: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn truncate_assistant_transcript(
            &self,
            _session_id: &SessionId,
            _provider_item_id: Option<&str>,
            _previous_item_id: Option<&str>,
            _content_index: Option<u32>,
            _response_id: Option<&str>,
            _text: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn signal_turn_interrupt(
            &self,
            _session_id: &SessionId,
            _response_id: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn signal_output_audio_degraded(
            &self,
            _session_id: &SessionId,
            _dropped: u64,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn signal_turn_completed(
            &self,
            _session_id: &SessionId,
            _stop_reason: meerkat_core::types::StopReason,
            _usage: meerkat_core::types::Usage,
            _response_id: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn signal_terminal_error(
            &self,
            session_id: &SessionId,
            code: meerkat_core::live_adapter::LiveAdapterErrorCode,
            message: &str,
        ) -> Result<(), LiveProjectionError> {
            self.terminal_errors.lock().unwrap().push((
                session_id.clone(),
                code,
                message.to_string(),
            ));
            Ok(())
        }

        async fn append_realtime_transcript(
            &self,
            _session_id: &SessionId,
            _event: &meerkat_core::RealtimeTranscriptEvent,
        ) -> Result<meerkat_core::RealtimeTranscriptApplyOutcome, LiveProjectionError> {
            Ok(meerkat_core::RealtimeTranscriptApplyOutcome::default())
        }
    }

    fn generated_admission(channel_id: LiveChannelId) -> LiveWsTokenAdmission {
        LiveWsTokenAdmission {
            channel_id,
            admitted: true,
            rejection: None,
            public_error_class: None,
            sequence: 1,
        }
    }

    fn generated_rejection(
        channel_id: LiveChannelId,
        rejection: LiveWsTokenAdmissionRejection,
    ) -> LiveWsTokenAdmission {
        LiveWsTokenAdmission {
            channel_id,
            admitted: false,
            rejection: Some(rejection),
            public_error_class: Some(LiveWsTokenAdmissionPublicErrorClass::InvalidToken),
            sequence: 1,
        }
    }

    fn test_state_with_authority(
        host: Arc<LiveAdapterHost>,
        authority: Arc<dyn LiveWsTokenAuthority>,
    ) -> LiveWsState {
        LiveWsState::new_for_test_with_token_authority(host, authority)
    }

    #[test]
    fn token_string_accepts_url_safe_alphabet() {
        LiveTokenString::new("abcXYZ-_.~0123").unwrap();
        // UUID hex-with-hyphens shape:
        LiveTokenString::new("550e8400-e29b-41d4-a716-446655440000").unwrap();
    }

    #[test]
    fn token_string_rejects_unsafe_bytes() {
        assert!(matches!(
            LiveTokenString::new("ab cd"),
            Err(TokenParseError::InvalidByte { .. })
        ));
        assert!(matches!(
            LiveTokenString::new("ab/cd"),
            Err(TokenParseError::InvalidByte { .. })
        ));
        assert!(matches!(
            LiveTokenString::new("ab%20cd"),
            Err(TokenParseError::InvalidByte { .. })
        ));
        assert!(matches!(
            LiveTokenString::new(""),
            Err(TokenParseError::Empty)
        ));
    }

    #[tokio::test]
    async fn mint_token_records_issue_through_authority() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let authority = ScriptedLiveWsTokenAuthority::default();
        let authority = Arc::new(authority);
        let state = test_state_with_authority(host, authority.clone());
        let session_id = SessionId::new();
        let channel_id = LiveChannelId::new("test_ch");
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();
        assert!(!token.as_str().is_empty());
        let issued = authority.issued.lock().await;
        assert_eq!(issued.len(), 1);
        assert_eq!(issued[0].0, session_id);
        assert_eq!(issued[0].1, channel_id);
        assert_eq!(issued[0].2, token);
        assert_eq!(issued[0].4, u64::try_from(TOKEN_TTL.as_millis()).unwrap());
    }

    #[tokio::test]
    async fn token_ttl_is_transport_configuration_only() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let state = LiveWsState::with_token_ttl(
            Arc::clone(&host),
            Arc::new(GeneratedTestMachineLiveChannelCloseFeedback::new(
                Arc::clone(&host),
            )),
            Arc::new(GeneratedTestMachineLiveChannelStatusFeedback::new(
                Arc::clone(&host),
            )),
            ScriptedLiveWsTokenAuthority::new([]),
            Duration::from_millis(40),
        );
        assert_eq!(state.token_ttl(), Duration::from_millis(40));
    }

    #[tokio::test]
    async fn websocket_roundtrip_with_token() {
        // R5 (Option A): the test exercises the full input roundtrip path —
        // upgrade → generated token admission → `host.send_input` → adapter.send_command.
        // Without an attached & Ready adapter, `send_input` errors with
        // `NoAdapter`, the server emits a JSON error frame and may tear the
        // socket down before the client's subsequent Close is observed,
        // surfacing as a Linux broken-pipe failure on BuildBuddy. Attach an
        // idle (Ready) stub so `send_input` succeeds; the server then idles
        // on `next_observation_raw` (pending forever) until the client sends
        // Close. The test no longer relies on any server-initiated close
        // frame as a success signal.
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        host.attach_adapter(&channel_id, Arc::new(IdleAdapter))
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(
            &channel_id,
            meerkat_core::live_adapter::LiveAdapterStatus::Ready,
        )
        .await
        .unwrap();

        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (mut write, _read) = futures::StreamExt::split(ws_stream);

        use futures::SinkExt;
        use tokio_tungstenite::tungstenite::Message;
        let input = serde_json::json!({"kind": "text", "text": "hello"});
        write
            .send(Message::Text(input.to_string().into()))
            .await
            .unwrap();

        write.send(Message::Close(None)).await.unwrap();
        server_handle.abort();
    }

    // -- Wave-3 RPC pump migration regression --

    /// Adapter that never produces an observation — `next_observation` is
    /// `pending` forever. Used by tests that drive client→server frame
    /// behavior and need the observation arm of the WS pump's `select!` to
    /// stay quiet (otherwise the un-attached channel surfaces `NoAdapter`
    /// and the pump exits before the test's frame is processed).
    struct IdleAdapter;

    #[async_trait::async_trait]
    impl meerkat_core::live_adapter::LiveAdapter for IdleAdapter {
        async fn send_command(
            &self,
            _command: meerkat_core::live_adapter::LiveAdapterCommand,
        ) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }

        async fn next_observation(
            &self,
        ) -> Result<
            Option<meerkat_core::live_adapter::LiveAdapterObservation>,
            meerkat_core::live_adapter::LiveAdapterError,
        > {
            std::future::pending().await
        }

        fn status(&self) -> meerkat_core::live_adapter::LiveAdapterStatus {
            meerkat_core::live_adapter::LiveAdapterStatus::Ready
        }

        async fn close(&self) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }
    }

    struct BlockingInputAdapter {
        started: Arc<Semaphore>,
        release: Arc<Semaphore>,
        completed: Arc<Semaphore>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::live_adapter::LiveAdapter for BlockingInputAdapter {
        async fn send_command(
            &self,
            command: meerkat_core::live_adapter::LiveAdapterCommand,
        ) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            if matches!(
                command,
                meerkat_core::live_adapter::LiveAdapterCommand::SendInput { .. }
            ) {
                self.started.add_permits(1);
                let permit = self
                    .release
                    .acquire()
                    .await
                    .map_err(|_| meerkat_core::live_adapter::LiveAdapterError::Closed)?;
                permit.forget();
                self.completed.add_permits(1);
            }
            Ok(())
        }

        async fn next_observation(
            &self,
        ) -> Result<
            Option<meerkat_core::live_adapter::LiveAdapterObservation>,
            meerkat_core::live_adapter::LiveAdapterError,
        > {
            std::future::pending().await
        }

        fn status(&self) -> meerkat_core::live_adapter::LiveAdapterStatus {
            meerkat_core::live_adapter::LiveAdapterStatus::Ready
        }

        async fn close(&self) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }
    }

    struct CountingInputAdapter {
        send_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::live_adapter::LiveAdapter for CountingInputAdapter {
        async fn send_command(
            &self,
            command: meerkat_core::live_adapter::LiveAdapterCommand,
        ) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            if matches!(
                command,
                meerkat_core::live_adapter::LiveAdapterCommand::SendInput { .. }
            ) {
                self.send_count.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }

        async fn next_observation(
            &self,
        ) -> Result<
            Option<meerkat_core::live_adapter::LiveAdapterObservation>,
            meerkat_core::live_adapter::LiveAdapterError,
        > {
            std::future::pending().await
        }

        fn status(&self) -> meerkat_core::live_adapter::LiveAdapterStatus {
            meerkat_core::live_adapter::LiveAdapterStatus::Ready
        }

        async fn close(&self) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }
    }

    async fn open_ready_test_channel(
        host: &Arc<LiveAdapterHost>,
        adapter: Arc<dyn meerkat_core::live_adapter::LiveAdapter>,
    ) -> (SessionId, LiveChannelId) {
        let session_id = SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .expect("test channel should open");
        host.attach_adapter(&channel_id, adapter)
            .await
            .expect("test adapter should attach");
        host.commit_status_with_generated_test_machine_authority(
            &channel_id,
            meerkat_core::live_adapter::LiveAdapterStatus::Ready,
        )
        .await
        .expect("test channel should become ready");
        (session_id, channel_id)
    }

    struct RejectFirstInputAdapter {
        send_count: Arc<std::sync::atomic::AtomicUsize>,
        ready_emitted: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl meerkat_core::live_adapter::LiveAdapter for RejectFirstInputAdapter {
        async fn send_command(
            &self,
            command: meerkat_core::live_adapter::LiveAdapterCommand,
        ) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            if matches!(
                command,
                meerkat_core::live_adapter::LiveAdapterCommand::SendInput { .. }
            ) {
                let index = self
                    .send_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if index == 0 {
                    let reason = meerkat_core::live_adapter::LiveConfigRejectionReason::ImageInputUnsupportedMime {
                        mime_type: "image/gif".to_string(),
                    };
                    return Err(
                        meerkat_core::live_adapter::LiveAdapterError::ProviderError {
                            code:
                                meerkat_core::live_adapter::LiveAdapterErrorCode::ConfigRejected {
                                    reason: reason.clone(),
                                },
                            message: reason.to_string(),
                        },
                    );
                }
            }
            Ok(())
        }

        async fn next_observation(
            &self,
        ) -> Result<
            Option<meerkat_core::live_adapter::LiveAdapterObservation>,
            meerkat_core::live_adapter::LiveAdapterError,
        > {
            if !self
                .ready_emitted
                .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                return Ok(Some(
                    meerkat_core::live_adapter::LiveAdapterObservation::Ready,
                ));
            }
            std::future::pending().await
        }

        fn status(&self) -> meerkat_core::live_adapter::LiveAdapterStatus {
            meerkat_core::live_adapter::LiveAdapterStatus::Ready
        }

        async fn close(&self) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }
    }

    /// Single-shot adapter that emits one scripted observation then returns
    /// `Ok(None)` to terminate the pump. Used by the wave-3 regression tests
    /// to drive the WS pump's `Terminal` short-circuit branch.
    struct ScriptedAdapter {
        observation: tokio::sync::Mutex<Option<meerkat_core::live_adapter::LiveAdapterObservation>>,
    }

    impl ScriptedAdapter {
        fn new(observation: meerkat_core::live_adapter::LiveAdapterObservation) -> Self {
            Self {
                observation: tokio::sync::Mutex::new(Some(observation)),
            }
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::live_adapter::LiveAdapter for ScriptedAdapter {
        async fn send_command(
            &self,
            _command: meerkat_core::live_adapter::LiveAdapterCommand,
        ) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }

        async fn next_observation(
            &self,
        ) -> Result<
            Option<meerkat_core::live_adapter::LiveAdapterObservation>,
            meerkat_core::live_adapter::LiveAdapterError,
        > {
            let mut slot = self.observation.lock().await;
            // Yield once, then idle forever so the pump survives until the
            // server drops the channel after generated close cleanup.
            if let Some(obs) = slot.take() {
                Ok(Some(obs))
            } else {
                std::future::pending().await
            }
        }

        fn status(&self) -> meerkat_core::live_adapter::LiveAdapterStatus {
            meerkat_core::live_adapter::LiveAdapterStatus::Ready
        }

        async fn close(&self) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn websocket_never_publishes_observation_when_canonical_apply_fails() {
        use futures::StreamExt as _;
        use meerkat_core::live_adapter::LiveAdapterObservation;
        use tokio_tungstenite::tungstenite::Message;

        let sink = Arc::new(TerminalRecordingProjectionSink::default());
        sink.reject_user_transcript.store(true, Ordering::SeqCst);
        let sink_for_host: Arc<dyn LiveProjectionSink> = sink;
        let host = Arc::new(LiveAdapterHost::new(sink_for_host));
        let session_id = SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        host.attach_adapter(
            &channel_id,
            Arc::new(ScriptedAdapter::new(
                LiveAdapterObservation::UserTranscriptFinal {
                    provider_item_id: Some("provider-user-1".to_string()),
                    previous_item_id: None,
                    content_index: Some(0),
                    text: "must-not-publish".to_string(),
                },
            )),
        )
        .await
        .unwrap();

        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (_write, mut read) = ws_stream.split();
        let mut saw_uncommitted = false;
        let disconnected = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(message) = read.next().await {
                match message {
                    Ok(Message::Text(text)) => {
                        if text.contains("must-not-publish") {
                            saw_uncommitted = true;
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => return true,
                    Ok(_) => {}
                }
            }
            true
        })
        .await
        .unwrap_or(false);

        assert!(
            disconnected,
            "canonical apply failure must disconnect the WS"
        );
        assert!(
            !saw_uncommitted,
            "provider observation must not publish when canonical apply failed"
        );
        server_handle.abort();
    }

    #[test]
    fn production_websocket_admission_is_process_shared_across_states() {
        let first = LiveWsAdmission::production();
        let second = LiveWsAdmission::production();
        assert!(Arc::ptr_eq(
            &first.connection_semaphore,
            &second.connection_semaphore
        ));
        assert!(Arc::ptr_eq(
            &first.raw_message_semaphore,
            &second.raw_message_semaphore
        ));
        assert!(Arc::ptr_eq(
            &first.ingress_memory_semaphore,
            &second.ingress_memory_semaphore
        ));
    }

    #[test]
    fn all_advertised_websocket_connections_reserve_a_full_raw_message() {
        let admission = LiveWsAdmission::new(
            LIVE_WS_MAX_CONNECTIONS,
            LIVE_WS_PROCESS_INGRESS_MEMORY_BUDGET_BYTES,
        );
        let mut permits = Vec::new();
        for _ in 0..LIVE_WS_MAX_CONNECTIONS {
            permits.push(
                admission
                    .try_admit_connection(LIVE_WS_MAX_MESSAGE_BYTES)
                    .expect("every advertised connection must fit a full raw codec message"),
            );
        }
        assert_eq!(admission.raw_message_semaphore.available_permits(), 0);
        assert!(matches!(
            admission.try_admit_connection(LIVE_WS_MAX_MESSAGE_BYTES),
            Err(LiveWsAdmissionError::ConnectionBackpressured { .. })
        ));
        drop(permits);
        assert_eq!(
            admission.raw_message_semaphore.available_permits(),
            LIVE_WS_MAX_CONNECTIONS * LIVE_WS_MAX_MESSAGE_BYTES
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_token_authority_releases_connection_and_raw_message_admission() {
        let admission = LiveWsAdmission::new(1, LIVE_WS_PROCESS_INGRESS_MEMORY_BUDGET_BYTES);
        let connection_permit = admission
            .try_admit_connection(LIVE_WS_MAX_MESSAGE_BYTES)
            .expect("first upgraded connection admission");

        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let channel_id = LiveChannelId::new("stalled_token_authority");
        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let authority_guard = authority.admissions.lock().await;
        let state = Arc::new(
            test_state_with_authority(host, authority.clone())
                .with_test_shared_transport_admission(
                    admission.clone(),
                    LIVE_WS_MAX_MESSAGE_BYTES,
                    LIVE_WS_MAX_FRAME_BYTES,
                ),
        );

        let task = tokio::spawn(async move {
            let _connection_permit = connection_permit;
            state
                .resolve_token_admission("blocked-token", &channel_id)
                .await
        });
        tokio::task::yield_now().await;
        assert_eq!(admission.connection_semaphore.available_permits(), 0);
        assert_eq!(admission.raw_message_semaphore.available_permits(), 0);

        tokio::time::advance(LIVE_WS_TOKEN_ADMISSION_TIMEOUT + Duration::from_millis(1)).await;
        let error = task
            .await
            .expect("stalled authority task must finish at its deadline")
            .expect_err("stalled authority must fail closed");
        assert!(error.contains("deadline exceeded"));
        assert_eq!(admission.connection_semaphore.available_permits(), 1);
        assert_eq!(
            admission.raw_message_semaphore.available_permits(),
            LIVE_WS_MAX_MESSAGE_BYTES
        );
        drop(authority_guard);
    }

    #[tokio::test]
    async fn listener_header_scanner_handles_split_terminator_and_rejects_oversize() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let connect = tokio::spawn(tokio::net::TcpStream::connect(address));
        let (accepted, _) = listener.accept().await.unwrap();
        let _client = connect.await.unwrap().unwrap();
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = semaphore.try_acquire_owned().unwrap();
        let mut io = AdmittedLiveWsIo::new(accepted, permit);

        io.observe_header_bytes(b"GET /live/ws HTTP/1.1\r\nHost: test\r\n\r")
            .unwrap();
        assert!(!io.header_complete);
        io.observe_header_bytes(b"\nbody").unwrap();
        assert!(io.header_complete);

        io.header_complete = false;
        io.header_bytes = 0;
        io.header_window = 0;
        let oversized = vec![b'x'; LIVE_WS_MAX_HTTP_HEADER_BYTES];
        let error = io
            .observe_header_bytes(&oversized)
            .expect_err("unterminated oversized header must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn websocket_ingress_charge_bounds_text_decode_and_binary_copy_peaks() {
        assert_eq!(
            live_ws_ingress_memory_charge_bytes(LiveWsIngressKind::Text, 1),
            LIVE_WS_MIN_INGRESS_CHARGE_BYTES * LIVE_WS_TEXT_MEMORY_MULTIPLIER
        );
        assert_eq!(
            live_ws_ingress_memory_charge_bytes(LiveWsIngressKind::Binary, 1),
            LIVE_WS_MIN_INGRESS_CHARGE_BYTES * LIVE_WS_BINARY_MEMORY_MULTIPLIER
        );
        assert_eq!(
            live_ws_ingress_memory_charge_bytes(LiveWsIngressKind::Text, 100_000),
            100_000 * LIVE_WS_TEXT_MEMORY_MULTIPLIER
        );
    }

    #[tokio::test]
    async fn websocket_connection_admission_is_shared_across_routers_and_releases_on_close() {
        use futures::SinkExt as _;
        use tokio_tungstenite::tungstenite::{Error as TungsteniteError, Message};

        let shared_admission = LiveWsAdmission::new(1, LIVE_WS_MIN_INGRESS_CHARGE_BYTES * 3);

        let first_host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let (first_session, first_channel) =
            open_ready_test_channel(&first_host, Arc::new(IdleAdapter)).await;
        let first_authority = ScriptedLiveWsTokenAuthority::admitting(first_channel.clone());
        let first_state = Arc::new(
            test_state_with_authority(first_host, first_authority)
                .with_test_shared_transport_admission(
                    shared_admission.clone(),
                    LIVE_WS_MAX_MESSAGE_BYTES,
                    LIVE_WS_MAX_FRAME_BYTES,
                ),
        );
        let first_token = first_state
            .mint_token(&first_session, first_channel.clone())
            .await
            .unwrap();

        let second_host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let (second_session, second_channel) =
            open_ready_test_channel(&second_host, Arc::new(IdleAdapter)).await;
        let second_authority = ScriptedLiveWsTokenAuthority::admitting(second_channel.clone());
        let second_state = Arc::new(
            test_state_with_authority(second_host, second_authority)
                .with_test_shared_transport_admission(
                    shared_admission.clone(),
                    LIVE_WS_MAX_MESSAGE_BYTES,
                    LIVE_WS_MAX_FRAME_BYTES,
                ),
        );
        let second_token = second_state
            .mint_token(&second_session, second_channel.clone())
            .await
            .unwrap();

        let first_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let first_addr = first_listener.local_addr().unwrap();
        let first_server_state = Arc::clone(&first_state);
        let first_server = tokio::spawn(async move {
            serve_live_ws_listener(first_listener, first_server_state).await
        });
        let second_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let second_addr = second_listener.local_addr().unwrap();
        let second_server_state = Arc::clone(&second_state);
        let second_server = tokio::spawn(async move {
            serve_live_ws_listener(second_listener, second_server_state).await
        });

        let first_url =
            format!("ws://{first_addr}{LIVE_WS_PATH}?token={first_token}&channel={first_channel}");
        let (mut first_socket, _) = tokio_tungstenite::connect_async(&first_url).await.unwrap();
        assert_eq!(shared_admission.connection_semaphore.available_permits(), 0);

        let second_url = format!(
            "ws://{second_addr}{LIVE_WS_PATH}?token={second_token}&channel={second_channel}"
        );
        let rejection = tokio::time::timeout(
            Duration::from_secs(2),
            tokio_tungstenite::connect_async(&second_url),
        )
        .await
        .expect("saturated router must reject without parking")
        .expect_err("second router must share the connection ceiling");
        assert!(matches!(
            rejection,
            TungsteniteError::Http(response)
                if response.status() == StatusCode::SERVICE_UNAVAILABLE
        ));

        first_socket.send(Message::Close(None)).await.unwrap();
        drop(first_socket);
        tokio::time::timeout(Duration::from_secs(2), async {
            while shared_admission.connection_semaphore.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("closing one router's socket must release the process permit");

        // Connection rejection happens before token authority, so the same
        // second token remains usable once unrelated load releases.
        let (mut second_socket, _) = tokio_tungstenite::connect_async(&second_url).await.unwrap();
        second_socket.send(Message::Close(None)).await.unwrap();

        first_server.abort();
        second_server.abort();
    }

    #[tokio::test]
    async fn websocket_ingress_backpressure_is_scoped_cross_connection_and_retryable() {
        use futures::{SinkExt as _, StreamExt as _};
        use meerkat_contracts::{WireLiveAdapterErrorCode, WireLiveConfigRejectionReason};
        use tokio_tungstenite::tungstenite::Message;

        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let started = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let completed = Arc::new(Semaphore::new(0));
        let (first_session, first_channel) = open_ready_test_channel(
            &host,
            Arc::new(BlockingInputAdapter {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                completed: Arc::clone(&completed),
            }),
        )
        .await;
        let second_send_count = Arc::new(AtomicUsize::new(0));
        let (second_session, second_channel) = open_ready_test_channel(
            &host,
            Arc::new(CountingInputAdapter {
                send_count: Arc::clone(&second_send_count),
            }),
        )
        .await;
        let authority = ScriptedLiveWsTokenAuthority::new([
            generated_admission(first_channel.clone()),
            generated_admission(second_channel.clone()),
        ]);
        let frame = serde_json::json!({"kind": "text", "text": "hello"}).to_string();
        let ingress_capacity =
            live_ws_ingress_memory_charge_bytes(LiveWsIngressKind::Text, frame.len());
        let state = Arc::new(
            test_state_with_authority(host, authority).with_test_transport_admission(
                2,
                ingress_capacity,
                LIVE_WS_MAX_MESSAGE_BYTES,
                LIVE_WS_MAX_FRAME_BYTES,
            ),
        );
        let first_token = state
            .mint_token(&first_session, first_channel.clone())
            .await
            .unwrap();
        let second_token = state
            .mint_token(&second_session, second_channel.clone())
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_state = Arc::clone(&state);
        let server =
            tokio::spawn(async move { serve_live_ws_listener(listener, server_state).await });

        let first_url =
            format!("ws://{addr}{LIVE_WS_PATH}?token={first_token}&channel={first_channel}");
        let (first_socket, _) = tokio_tungstenite::connect_async(&first_url).await.unwrap();
        let (mut first_write, _first_read) = first_socket.split();
        first_write
            .send(Message::Text(frame.clone().into()))
            .await
            .unwrap();
        let first_started = tokio::time::timeout(Duration::from_secs(2), started.acquire())
            .await
            .expect("first send_input should start")
            .expect("started semaphore should remain open");
        first_started.forget();
        assert_eq!(
            state.admission.ingress_memory_semaphore.available_permits(),
            0
        );

        let second_url =
            format!("ws://{addr}{LIVE_WS_PATH}?token={second_token}&channel={second_channel}");
        let (second_socket, _) = tokio_tungstenite::connect_async(&second_url).await.unwrap();
        let (mut second_write, mut second_read) = second_socket.split();
        second_write
            .send(Message::Text(frame.clone().into()))
            .await
            .unwrap();
        let rejection = tokio::time::timeout(Duration::from_secs(2), second_read.next())
            .await
            .expect("backpressure response must not park")
            .expect("socket should remain open")
            .expect("backpressure frame should be valid");
        let Message::Text(rejection) = rejection else {
            panic!("expected typed text rejection");
        };
        let observation: WireLiveAdapterObservation =
            serde_json::from_str(rejection.as_str()).expect("typed rejection should deserialize");
        assert!(matches!(
            observation,
            WireLiveAdapterObservation::CommandRejected {
                code: WireLiveAdapterErrorCode::ConfigRejected {
                    reason: WireLiveConfigRejectionReason::InputBackpressured {
                        max_pending_bytes,
                    },
                },
                ..
            } if max_pending_bytes == u64::try_from(ingress_capacity).unwrap()
        ));
        assert_eq!(second_send_count.load(Ordering::SeqCst), 0);

        release.add_permits(1);
        let first_completed = tokio::time::timeout(Duration::from_secs(2), completed.acquire())
            .await
            .expect("first send_input should complete")
            .expect("completed semaphore should remain open");
        first_completed.forget();
        tokio::time::timeout(Duration::from_secs(2), async {
            while state.admission.ingress_memory_semaphore.available_permits() != ingress_capacity {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("ingress admission should release after host.send_input returns");

        second_write
            .send(Message::Text(frame.into()))
            .await
            .expect("the backpressured socket must survive for retry");
        tokio::time::timeout(Duration::from_secs(2), async {
            while second_send_count.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("retry should reach the second adapter after release");

        first_write.send(Message::Close(None)).await.unwrap();
        second_write.send(Message::Close(None)).await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn websocket_image_is_transport_rejected_and_socket_survives_retry() {
        use futures::{SinkExt as _, StreamExt as _};
        use meerkat_contracts::{WireLiveAdapterErrorCode, WireLiveConfigRejectionReason};
        use tokio_tungstenite::tungstenite::Message;

        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let send_count = Arc::new(AtomicUsize::new(0));
        let (session_id, channel_id) = open_ready_test_channel(
            &host,
            Arc::new(CountingInputAdapter {
                send_count: Arc::clone(&send_count),
            }),
        )
        .await;
        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_state = Arc::clone(&state);
        let server =
            tokio::spawn(async move { serve_live_ws_listener(listener, server_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (mut write, mut read) = socket.split();
        write
            .send(Message::Text(
                r#"{"kind":"image","idempotency_key":"bad-base64-1","mime":"image/png","data":"@@@"}"#
                    .into(),
            ))
            .await
            .unwrap();
        let rejection = tokio::time::timeout(Duration::from_secs(2), read.next())
            .await
            .expect("direct WebSocket image should reject promptly")
            .expect("socket must remain connected")
            .expect("rejection frame should be valid");
        let Message::Text(rejection) = rejection else {
            panic!("expected typed text rejection");
        };
        let rejection: WireLiveAdapterObservation =
            serde_json::from_str(rejection.as_str()).expect("typed rejection should deserialize");
        assert!(matches!(
            rejection,
            WireLiveAdapterObservation::CommandRejected {
                code: WireLiveAdapterErrorCode::ConfigRejected {
                    reason: WireLiveConfigRejectionReason::ImageInputTransportUnsupported {
                        ref transport,
                    },
                },
                ..
            } if transport == "direct_websocket_use_live_send_input_rpc"
        ));
        assert_eq!(send_count.load(Ordering::SeqCst), 0);

        write
            .send(Message::Text(
                r#"{"kind":"text","text":"retry on same socket"}"#.into(),
            ))
            .await
            .expect("socket must remain writable after scoped rejection");
        tokio::time::timeout(Duration::from_secs(2), async {
            while send_count.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("valid retry should reach adapter on the same socket");
        assert_eq!(
            state.host().channel_status(&channel_id).await.unwrap(),
            meerkat_core::live_adapter::LiveAdapterStatus::Ready
        );

        write.send(Message::Close(None)).await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn websocket_explicit_frame_limit_closes_oversized_peer_and_releases_connection() {
        use futures::{SinkExt as _, StreamExt as _};
        use tokio_tungstenite::tungstenite::Message;

        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let (session_id, channel_id) = open_ready_test_channel(&host, Arc::new(IdleAdapter)).await;
        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state =
            Arc::new(
                test_state_with_authority(Arc::clone(&host), authority)
                    .with_test_transport_admission(1, 1024 * 1024, 128, 128),
            );
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_state = Arc::clone(&state);
        let server =
            tokio::spawn(async move { serve_live_ws_listener(listener, server_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        socket
            .send(Message::Text("x".repeat(256).into()))
            .await
            .unwrap();
        let termination = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("oversized frame must terminate the peer promptly");
        assert!(
            termination.is_none()
                || matches!(termination, Some(Ok(Message::Close(_))) | Some(Err(_))),
            "oversized frame must not be delivered as an application message"
        );
        drop(socket);
        tokio::time::timeout(Duration::from_secs(2), async {
            while state.admission.connection_semaphore.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("oversized peer teardown must release connection admission");

        server.abort();
    }

    #[tokio::test]
    async fn websocket_disconnects_after_generated_terminal_error_close() {
        use meerkat_core::live_adapter::{LiveAdapterErrorCode, LiveAdapterObservation};

        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        host.attach_adapter(
            &channel_id,
            Arc::new(ScriptedAdapter::new(LiveAdapterObservation::Error {
                code: LiveAdapterErrorCode::ProviderError,
                message: "scripted terminal failure".into(),
            })),
        )
        .await
        .unwrap();

        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        use futures::StreamExt;
        use tokio_tungstenite::tungstenite::Message;
        let (_write, mut read) = ws_stream.split();

        // Terminal adapter error classes are not public WS result classes.
        // The transport closes only after generated close authority commits.
        let mut saw_terminal_error_observation = false;
        let mut disconnected = false;
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if text.contains("\"observation\"") && text.contains("provider_error") {
                        saw_terminal_error_observation = true;
                    }
                }
                Ok(Message::Close(_)) => {
                    disconnected = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => {
                    disconnected = true;
                    break;
                }
            }
        }

        assert!(
            !saw_terminal_error_observation,
            "terminal adapter error class must not be published without generated public authority"
        );
        assert!(
            disconnected,
            "terminal adapter error should disconnect the WS"
        );
        let status = state.host().channel_status(&channel_id).await.unwrap();
        assert_eq!(
            status,
            meerkat_core::live_adapter::LiveAdapterStatus::Closed,
            "generated close authority must commit before terminal disconnect"
        );

        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_applies_runtime_preclosed_terminal_error_without_new_feedback() {
        use meerkat_core::live_adapter::{
            LiveAdapterErrorCode, LiveAdapterStatus, LiveConfigRejectionReason,
        };

        let sink = Arc::new(TerminalRecordingProjectionSink::default());
        let sink_for_host: Arc<dyn LiveProjectionSink> = sink.clone();
        let host = Arc::new(LiveAdapterHost::new(sink_for_host));
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        host.attach_adapter(&channel_id, Arc::new(IdleAdapter))
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(
            &channel_id,
            LiveAdapterStatus::Ready,
        )
        .await
        .unwrap();

        let close_observation = host
            .signal_terminal_error_observed(
                &channel_id,
                LiveAdapterErrorCode::ConfigRejected {
                    reason: LiveConfigRejectionReason::Other {
                        detail: "runtime-preclosed terminal".into(),
                    },
                },
            )
            .await
            .unwrap();
        host.prepare_channel_physical_close(&close_observation)
            .await
            .unwrap();
        let close_authority = host
            .close_commit_authority_from_generated_test_machine(&close_observation)
            .await
            .unwrap();
        host.commit_channel_close_observation(&close_observation, &close_authority)
            .await
            .unwrap();
        assert_eq!(
            host.channel_status(&channel_id).await.unwrap(),
            LiveAdapterStatus::Closed
        );

        let close_feedback = Arc::new(RejectingCloseFeedback::default());
        let close_feedback_for_state: Arc<dyn LiveChannelCloseFeedback> = close_feedback.clone();
        let state = Arc::new(LiveWsState::with_token_ttl(
            Arc::clone(&host),
            close_feedback_for_state,
            Arc::new(GeneratedTestMachineLiveChannelStatusFeedback::new(
                Arc::clone(&host),
            )),
            ScriptedLiveWsTokenAuthority::admitting(channel_id.clone()),
            TOKEN_TTL,
        ));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        use futures::StreamExt;
        let (_write, mut read) = ws_stream.split();

        while let Some(msg) = read.next().await {
            match msg {
                Ok(tokio_tungstenite::tungstenite::Message::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            }
        }

        assert_eq!(
            close_feedback.calls.load(Ordering::SeqCst),
            0,
            "transport must not request a second close authority after generated close committed"
        );
        let terminals = sink.terminal_errors.lock().unwrap();
        assert_eq!(
            terminals.len(),
            1,
            "preclosed synthetic terminal observation must still reach the projection sink"
        );
        assert_eq!(terminals[0].0, session_id);
        assert!(matches!(
            terminals[0].1,
            LiveAdapterErrorCode::ConfigRejected { .. }
        ));

        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_commits_generated_close_before_public_closed_status() {
        use meerkat_core::live_adapter::{LiveAdapterObservation, LiveAdapterStatus};

        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        host.attach_adapter(
            &channel_id,
            Arc::new(ScriptedAdapter::new(
                LiveAdapterObservation::StatusChanged {
                    status: LiveAdapterStatus::Closed,
                },
            )),
        )
        .await
        .unwrap();

        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        use futures::StreamExt;
        use tokio_tungstenite::tungstenite::Message;
        let (_write, mut read) = ws_stream.split();

        let mut saw_closed_status = false;
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let observation: WireLiveAdapterObservation =
                        serde_json::from_str(&text).expect("wire observation");
                    if matches!(
                        observation,
                        WireLiveAdapterObservation::StatusChanged {
                            status: meerkat_contracts::WireLiveAdapterStatus::Closed
                        }
                    ) {
                        let status = state.host().channel_status(&channel_id).await.unwrap();
                        assert_eq!(
                            status,
                            LiveAdapterStatus::Closed,
                            "generated close authority must commit before public closed status"
                        );
                        saw_closed_status = true;
                        break;
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            }
        }

        assert!(
            saw_closed_status,
            "client should receive closed status only after generated close commit"
        );

        server_handle.abort();
    }

    /// R5-9: a `LiveAdapterObservation::CommandRejected` flowing
    /// through the WS pump is forwarded to the client as JSON but does
    /// NOT close the WebSocket. The channel survives so the client can
    /// retry with a supported input shape.
    #[tokio::test]
    async fn websocket_forwards_command_rejected_without_closing() {
        use meerkat_core::live_adapter::{LiveAdapterErrorCode, LiveAdapterObservation};

        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        host.attach_adapter(
            &channel_id,
            Arc::new(ScriptedAdapter::new(
                LiveAdapterObservation::CommandRejected {
                    code: LiveAdapterErrorCode::ConfigRejected {
                        reason: meerkat_core::live_adapter::LiveConfigRejectionReason::ImageInputNotImplemented,
                    },
                    message: "image_input_not_implemented".into(),
                },
            )),
        )
        .await
        .unwrap();

        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        use futures::StreamExt;
        use tokio_tungstenite::tungstenite::Message;
        let (_write, mut read) = ws_stream.split();

        // Read the first JSON frame — must be the typed CommandRejected
        // observation. After that the WS must remain open (no Close
        // frame within a short window). The ScriptedAdapter idles
        // forever after delivering its single observation, so the pump
        // sits on `next_observation()` and never closes naturally.
        let mut saw_command_rejected = false;
        let mut saw_close = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(300);
        while let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) {
            match tokio::time::timeout(remaining, read.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    if text.contains("command_rejected")
                        && text.contains("image_input_not_implemented")
                    {
                        saw_command_rejected = true;
                    }
                }
                Ok(Some(Ok(Message::Close(_)))) => {
                    saw_close = true;
                    break;
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(_))) | Ok(None) => break,
                Err(_) => break, // timeout — desired path: WS still open
            }
        }

        assert!(
            saw_command_rejected,
            "client must receive the typed CommandRejected observation JSON"
        );
        assert!(
            !saw_close,
            "R5-9: WS must NOT close after a CommandRejected observation"
        );

        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_surfaces_immediate_adapter_rejection_and_accepts_next_command() {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        let send_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        host.attach_adapter(
            &channel_id,
            Arc::new(RejectFirstInputAdapter {
                send_count: Arc::clone(&send_count),
                ready_emitted: std::sync::atomic::AtomicBool::new(false),
            }),
        )
        .await
        .unwrap();

        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });
        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (mut write, mut read) = ws_stream.split();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match read.next().await {
                    Some(Ok(Message::Text(text)))
                        if matches!(
                            serde_json::from_str::<WireLiveAdapterObservation>(&text),
                            Ok(WireLiveAdapterObservation::Ready)
                        ) =>
                    {
                        break;
                    }
                    Some(Ok(_)) => continue,
                    other => panic!("expected adapter ready before input, got {other:?}"),
                }
            }
        })
        .await
        .expect("adapter must become ready");

        write
            .send(Message::Text(
                r#"{"kind":"text","text":"adapter rejects first command"}"#.into(),
            ))
            .await
            .unwrap();
        let observation = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match read.next().await {
                    Some(Ok(Message::Text(text))) => {
                        let observation: WireLiveAdapterObservation =
                            serde_json::from_str(&text).expect("typed wire observation");
                        if matches!(
                            observation,
                            WireLiveAdapterObservation::CommandRejected { .. }
                        ) {
                            break observation;
                        }
                    }
                    other => panic!("expected command rejection without disconnect, got {other:?}"),
                }
            }
        })
        .await
        .expect("immediate adapter rejection must be delivered");
        assert!(matches!(
            observation,
            WireLiveAdapterObservation::CommandRejected {
                code: meerkat_contracts::WireLiveAdapterErrorCode::ConfigRejected {
                    reason: meerkat_contracts::WireLiveConfigRejectionReason::ImageInputUnsupportedMime {
                        ref mime_type
                    }
                },
                ..
            } if mime_type == "image/gif"
        ));

        write
            .send(Message::Text(
                r#"{"kind":"text","text":"channel still alive"}"#.into(),
            ))
            .await
            .expect("the socket must remain writable after scoped rejection");
        tokio::time::timeout(Duration::from_secs(2), async {
            while send_count.load(std::sync::atomic::Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the adapter must receive the next command on the same channel");
        assert_eq!(
            state.host().channel_status(&channel_id).await.unwrap(),
            meerkat_core::live_adapter::LiveAdapterStatus::Ready
        );

        let _ = write.send(Message::Close(None)).await;
        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_rejects_invalid_token() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let channel_id = LiveChannelId::new("does_not_exist");
        let authority = ScriptedLiveWsTokenAuthority::rejecting(
            channel_id,
            LiveWsTokenAdmissionRejection::TokenNotFound,
        );
        let state = Arc::new(test_state_with_authority(host, authority));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        // G38: a non-matching channel must still be supplied for the upgrade
        // to parse — the token itself is bogus, so generated admission returns
        // not-found and the handler closes the socket with `invalid_token`.
        let url = format!("ws://{addr}{LIVE_WS_PATH}?token=bogus&channel=does_not_exist");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (_write, mut read) = futures::StreamExt::split(ws_stream);

        use futures::StreamExt;
        if let Some(Ok(msg)) = read.next().await {
            // Either the JSON error frame or the Close frame may arrive first;
            // any mention of `invalid_token` is acceptable.
            let _ = msg;
        }

        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_token_authority_error_fails_closed_without_public_class() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let state = Arc::new(test_state_with_authority(
            host,
            ScriptedLiveWsTokenAuthority::new([]),
        ));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token=bogus&channel=does_not_exist");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (_write, mut read) = futures::StreamExt::split(ws_stream);

        use futures::StreamExt;
        use tokio_tungstenite::tungstenite::Message;
        if let Ok(Some(Ok(msg))) =
            tokio::time::timeout(Duration::from_millis(500), read.next()).await
        {
            match msg {
                Message::Text(text) => assert!(
                    !text.contains("invalid_token"),
                    "authority errors must not invent a public token class"
                ),
                Message::Close(Some(frame)) => assert!(
                    !frame.reason.contains("invalid_token"),
                    "authority errors must not invent a public token close reason"
                ),
                _ => {}
            }
        }

        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_missing_generated_public_class_fails_closed() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let channel_id = LiveChannelId::new("does_not_exist");
        let state = Arc::new(test_state_with_authority(
            host,
            ScriptedLiveWsTokenAuthority::new([LiveWsTokenAdmission {
                channel_id,
                admitted: false,
                rejection: Some(LiveWsTokenAdmissionRejection::TokenNotFound),
                public_error_class: None,
                sequence: 1,
            }]),
        ));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token=bogus&channel=does_not_exist");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (_write, mut read) = futures::StreamExt::split(ws_stream);

        use futures::StreamExt;
        use tokio_tungstenite::tungstenite::Message;
        if let Ok(Some(Ok(msg))) =
            tokio::time::timeout(Duration::from_millis(500), read.next()).await
        {
            match msg {
                Message::Text(text) => assert!(
                    !text.contains("invalid_token"),
                    "missing generated public class must not invent invalid_token"
                ),
                Message::Close(Some(frame)) => assert!(
                    !frame.reason.contains("invalid_token"),
                    "missing generated public class must not invent invalid_token close reason"
                ),
                _ => {}
            }
        }

        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_rejects_token_with_wrong_channel() {
        // G38 end-to-end: token minted for channel A is presented with
        // channel B in the WS upgrade — handler must refuse with
        // `invalid_token` and close.
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let session_a = meerkat_core::types::SessionId::new();
        let session_b = meerkat_core::types::SessionId::new();
        let channel_a = host
            .open_channel_with_generated_test_machine_authority(session_a.clone())
            .await
            .unwrap();
        let channel_b = host
            .open_channel_with_generated_test_machine_authority(session_b)
            .await
            .unwrap();

        let authority = ScriptedLiveWsTokenAuthority::rejecting(
            channel_b.clone(),
            LiveWsTokenAdmissionRejection::TokenChannelMismatch,
        );
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_a, channel_a.clone())
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        // Present channel B with token bound to A.
        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_b}");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        use futures::StreamExt;
        use tokio_tungstenite::tungstenite::Message;
        let (_write, mut read) = ws_stream.split();

        let mut saw_invalid_token = false;
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if text.contains("invalid_token") {
                        saw_invalid_token = true;
                    }
                }
                Ok(Message::Close(Some(frame))) => {
                    assert!(
                        frame.reason.contains("invalid_token"),
                        "unexpected close reason: {}",
                        frame.reason
                    );
                    saw_invalid_token = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(
            saw_invalid_token,
            "channel mismatch must surface as invalid_token to the client"
        );

        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_rejects_binary_without_format() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        // Attach an idle adapter so the observation arm of the pump's
        // select! stays pending instead of immediately resolving with
        // `Err(NoAdapter)` and closing the loop before the test sends.
        host.attach_adapter(&channel_id, Arc::new(IdleAdapter))
            .await
            .unwrap();
        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let (mut write, mut read) = ws_stream.split();

        write
            .send(Message::Binary(vec![0u8; 32].into()))
            .await
            .unwrap();

        let disconnected = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Close(_)) | Err(_) => return true,
                    Ok(_) => continue,
                }
            }
            true
        })
        .await
        .unwrap_or(false);
        assert!(
            disconnected,
            "expected transport disconnect for un-negotiated binary"
        );
        let status = state.host().channel_status(&channel_id).await.unwrap();
        assert_eq!(
            status,
            meerkat_core::live_adapter::LiveAdapterStatus::Closed,
            "generated close authority must commit before disconnecting un-negotiated binary"
        );

        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_closes_on_invalid_text_frame() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        // See `websocket_rejects_binary_without_format` for why the test
        // attaches an idle adapter.
        host.attach_adapter(&channel_id, Arc::new(IdleAdapter))
            .await
            .unwrap();
        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let (mut write, mut read) = ws_stream.split();

        write.send(Message::Text("not json".into())).await.unwrap();

        let disconnected = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Close(_)) | Err(_) => return true,
                    Ok(_) => continue,
                }
            }
            true
        })
        .await
        .unwrap_or(false);
        assert!(
            disconnected,
            "expected transport disconnect for invalid JSON"
        );
        let status = state.host().channel_status(&channel_id).await.unwrap();
        assert_eq!(
            status,
            meerkat_core::live_adapter::LiveAdapterStatus::Closed,
            "generated close authority must commit before disconnecting invalid JSON"
        );

        server_handle.abort();
    }

    /// D243: a frame whose JSON envelope is well-formed but whose base64
    /// `data` field fails wire validation (the same validation RPC
    /// `live/send_input` runs via `live_input_chunk_from_wire`) is rejected on
    /// the direct WS path exactly like a malformed-envelope frame — the channel
    /// fails closed instead of fail-open accepting the bad chunk.
    #[tokio::test]
    async fn websocket_closes_on_wire_validation_failure() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        host.attach_adapter(&channel_id, Arc::new(IdleAdapter))
            .await
            .unwrap();
        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let (mut write, mut read) = ws_stream.split();

        // Well-formed `LiveInputChunkWire::Audio` envelope, but `data` is not
        // valid base64 — `live_input_chunk_from_wire` rejects it.
        let frame = serde_json::json!({
            "kind": "audio",
            "data": "not valid base64!!!",
            "sample_rate_hz": 24000,
            "channels": 1
        })
        .to_string();
        write.send(Message::Text(frame.into())).await.unwrap();

        let disconnected = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Close(_)) | Err(_) => return true,
                    Ok(_) => continue,
                }
            }
            true
        })
        .await
        .unwrap_or(false);
        assert!(
            disconnected,
            "expected transport disconnect for wire-validation failure"
        );
        let status = state.host().channel_status(&channel_id).await.unwrap();
        assert_eq!(
            status,
            meerkat_core::live_adapter::LiveAdapterStatus::Closed,
            "generated close authority must commit before disconnecting on wire-validation failure"
        );

        server_handle.abort();
    }

    /// Adapter that delivers a single scripted observation after a short
    /// internal delay, then idles. Used to exercise the WS pump select!
    /// arms under inbound mic-audio saturation: a `biased;` arm ordering
    /// would let inbound binary frames starve the observation arm so the
    /// `TurnCompleted` never reaches the client.
    struct DelayedObservationAdapter {
        observation: tokio::sync::Mutex<Option<meerkat_core::live_adapter::LiveAdapterObservation>>,
        delay: Duration,
    }

    impl DelayedObservationAdapter {
        fn new(
            observation: meerkat_core::live_adapter::LiveAdapterObservation,
            delay: Duration,
        ) -> Self {
            Self {
                observation: tokio::sync::Mutex::new(Some(observation)),
                delay,
            }
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::live_adapter::LiveAdapter for DelayedObservationAdapter {
        async fn send_command(
            &self,
            _command: meerkat_core::live_adapter::LiveAdapterCommand,
        ) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }

        async fn next_observation(
            &self,
        ) -> Result<
            Option<meerkat_core::live_adapter::LiveAdapterObservation>,
            meerkat_core::live_adapter::LiveAdapterError,
        > {
            // Drop-safe: sleep BEFORE consuming the observation slot. The WS
            // pump's `select!` polls this future every loop iteration and
            // drops it whenever the other arm wins; consuming first would
            // lose the scripted observation if the future is dropped during
            // the sleep.
            tokio::time::sleep(self.delay).await;
            let mut slot = self.observation.lock().await;
            if let Some(obs) = slot.take() {
                Ok(Some(obs))
            } else {
                std::future::pending().await
            }
        }

        fn status(&self) -> meerkat_core::live_adapter::LiveAdapterStatus {
            meerkat_core::live_adapter::LiveAdapterStatus::Ready
        }

        async fn close(&self) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }
    }

    /// G2 (P1) regression: a `biased;` ordering in the WS pump's `select!`
    /// causes continuous inbound mic audio to starve the observation arm,
    /// so server-side observations (assistant output, `TurnCompleted`,
    /// errors) never reach the client. With fair scheduling, even under
    /// saturating binary input the observation arm must be picked within
    /// a bounded timeout.
    #[tokio::test]
    async fn observation_arm_not_starved_by_saturating_mic_audio() {
        use meerkat_core::live_adapter::LiveAdapterObservation;
        use meerkat_core::types::{StopReason, Usage};

        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        host.attach_adapter(
            &channel_id,
            Arc::new(DelayedObservationAdapter::new(
                LiveAdapterObservation::TurnCompleted {
                    response_id: Some("resp_1".into()),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                },
                Duration::from_millis(50),
            )),
        )
        .await
        .unwrap();
        host.commit_status_with_generated_test_machine_authority(
            &channel_id,
            meerkat_core::live_adapter::LiveAdapterStatus::Ready,
        )
        .await
        .unwrap();

        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!(
            "ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}&format=pcm_24k_mono"
        );
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let (mut write, mut read) = ws_stream.split();

        // Saturator: continuously push binary mic frames as fast as the
        // socket accepts them. Models the real-world condition where a
        // browser tab streams microphone audio at ~50 frames/sec and the
        // server-side select! must still poll the observation arm.
        let saturator = tokio::spawn(async move {
            let frame = vec![0u8; 960]; // ~20ms @ 24 kHz / 16-bit / mono
            loop {
                if write
                    .send(Message::Binary(frame.clone().into()))
                    .await
                    .is_err()
                {
                    break;
                }
                // Yield without sleeping — the test wants the saturator to
                // win the recv arm whenever scheduling is biased.
                tokio::task::yield_now().await;
            }
        });

        // Bounded timeout: under fair scheduling + a pinned observation
        // future, the `TurnCompleted` JSON forwarded by the observation
        // arm must reach the client well within the bound. 1500 ms is
        // generous; the regression (biased ordering and/or recreating
        // the observation future inside the select! arm) typically
        // misses it indefinitely.
        let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
        let mut saw_turn_completed = false;
        while let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) {
            match tokio::time::timeout(remaining, read.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    if text.contains("turn_completed") || text.contains("\"resp_1\"") {
                        saw_turn_completed = true;
                        break;
                    }
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(_))) | Ok(None) => break,
                Err(_) => break,
            }
        }

        saturator.abort();
        server_handle.abort();

        assert!(
            saw_turn_completed,
            "observation arm starved by mic-audio saturation — biased ordering or unpinned observation future regression"
        );
    }

    /// R6-2 (P2): the WS pump serializes observations through the typed
    /// `WireLiveAdapterObservation` mirror (not the raw core enum) at the
    /// public boundary. Verifies the forwarded JSON deserializes as
    /// `WireLiveAdapterObservation` — a future core variant added without
    /// an explicit forward arm in the wire `From` impl will surface as
    /// the `observation: "unknown"` sentinel rather than leaking a raw
    /// new tag through this seam.
    #[tokio::test]
    async fn websocket_forwards_observation_through_wire_mirror() {
        use meerkat_contracts::WireLiveAdapterObservation;
        use meerkat_core::live_adapter::LiveAdapterObservation;

        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        host.attach_adapter(
            &channel_id,
            Arc::new(ScriptedAdapter::new(LiveAdapterObservation::Ready)),
        )
        .await
        .unwrap();
        host.commit_status_with_generated_test_machine_authority(
            &channel_id,
            meerkat_core::live_adapter::LiveAdapterStatus::Ready,
        )
        .await
        .unwrap();

        let authority = ScriptedLiveWsTokenAuthority::admitting(channel_id.clone());
        let state = Arc::new(test_state_with_authority(host, authority));
        let token = state
            .mint_token(&session_id, channel_id.clone())
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}&channel={channel_id}");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        use futures::StreamExt;
        use tokio_tungstenite::tungstenite::Message;
        let (_write, mut read) = ws_stream.split();

        let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
        let mut typed_observation = None;
        while let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) {
            match tokio::time::timeout(remaining, read.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    // The WS write must produce JSON that deserializes as
                    // the typed wire mirror. Routing through the raw core
                    // type would be byte-equal today but would silently
                    // bypass the `Unknown { debug }` floor for future core
                    // variants.
                    match serde_json::from_str::<WireLiveAdapterObservation>(&text) {
                        Ok(obs) => {
                            typed_observation = Some(obs);
                            break;
                        }
                        Err(err) => panic!(
                            "WS forwarded JSON must deserialize as WireLiveAdapterObservation; got error {err} on payload: {text}"
                        ),
                    }
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(_))) | Ok(None) => break,
                Err(_) => break,
            }
        }

        server_handle.abort();

        match typed_observation {
            Some(WireLiveAdapterObservation::Ready) => {}
            other => panic!("expected wire-mirror Ready, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // D153: send_input failure WsErrorFrame carries a typed reason code
    // -----------------------------------------------------------------

    #[test]
    fn send_input_failure_frame_populates_typed_reason() {
        // A representative send_input failure: the channel is gone. The host
        // error is typed; the WS frame must carry its stable reason class in
        // the `reason` field (previously left `None`, forcing clients to
        // reparse the prose `error`).
        let host_err = LiveAdapterHostError::ChannelNotFound(LiveChannelId::new("ch-1"));
        let frame = WsErrorFrame {
            error: host_err.to_string(),
            reason: Some(host_err.reason_code().to_string()),
        };
        assert_eq!(frame.reason.as_deref(), Some("channel_not_found"));

        // The serialized frame must include the populated `reason` (the
        // `skip_serializing_if = Option::is_none` would have dropped it under
        // the old `reason: None` shape).
        let json = serde_json::to_string(&frame).unwrap();
        assert!(
            json.contains("\"reason\":\"channel_not_found\""),
            "serialized WsErrorFrame must carry the typed reason: {json}"
        );

        // Each host error variant maps to a distinct, stable reason class so
        // clients can route without parsing the message. Two structurally
        // different not-ready errors share the not-ready class, which is
        // correct; the point is the codes are stable and exhaustive.
        let cases = [
            (
                LiveAdapterHostError::NoAdapter(LiveChannelId::new("c")),
                "no_adapter",
            ),
            (
                LiveAdapterHostError::OpenNotAuthorized,
                "open_not_authorized",
            ),
            (
                LiveAdapterHostError::CloseNotAuthorized,
                "close_not_authorized",
            ),
            (
                LiveAdapterHostError::StatusNotAuthorized,
                "status_not_authorized",
            ),
            (
                LiveAdapterHostError::UnsupportedCommand("x"),
                "unsupported_command",
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.reason_code(), expected, "reason_code for {err:?}");
        }
    }
}
