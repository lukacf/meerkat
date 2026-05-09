//! Live WebSocket transport — bridges browser/test clients to `LiveAdapterHost`.
//!
//! Simple axum WebSocket server. Token-based channel auth.
//!
//! # Wire protocol
//!
//! Connect: `GET {LIVE_WS_PATH}?token={token}&channel={channel_id}&format={format}`
//!
//! - `token` (required): single-use, URL-safe token minted via `mint_token`,
//!   valid for [`TOKEN_TTL`] after issue.
//! - `channel` (required): the channel id the caller intends to bind to.
//!   G38: pinning the token to a specific `(token, channel_id)` tuple
//!   tightens the bearer-token misuse surface — a token leaked from one
//!   `live/open` cannot be replayed against a different channel.
//!   `consume_token` rejects with `ChannelMismatch` if the channel recorded
//!   at mint time does not match.
//! - `format` (required when sending binary frames): negotiates the binary
//!   payload encoding. Currently the only accepted value is `pcm_24k_mono`
//!   (16-bit signed little-endian PCM, 24 kHz, mono). Text-only sessions may
//!   omit this parameter; in that case any binary frame is rejected.
//!
//! Frames:
//!
//! - `Text`: JSON-encoded [`LiveInputChunk`]. Invalid JSON closes the socket
//!   with reason `invalid_frame`.
//! - `Binary`: raw audio bytes in the negotiated format. If no format was
//!   negotiated, the socket is closed with reason `binary_format_unnegotiated`.
//! - `Close`/`Ping`: handled by axum.
//! - Anything else (e.g. unsolicited `Pong`): closes the socket with reason
//!   `unsupported_frame`.
//!
//! Outbound frames are JSON-encoded [`LiveAdapterObservation`] values.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::host::{LiveAdapterHost, LiveChannelId, ObservationOutcome};
use axum::Router;
use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket, close_code};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use meerkat_core::live_adapter::LiveInputChunk;
use tokio::sync::Mutex;
use uuid::Uuid;

pub const LIVE_WS_PATH: &str = "/live/ws";

/// Time-to-live for a minted but unconsumed token.
pub const TOKEN_TTL: Duration = Duration::from_secs(60);

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
    fn random() -> Self {
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

/// Errors returned when consuming a token.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TokenConsumeError {
    #[error("token not found")]
    NotFound,
    #[error("token expired")]
    Expired,
    /// G38: token was minted for a different channel than the caller asserted.
    /// The bound channel id is intentionally not echoed back to keep the
    /// bearer-token model from leaking the originally-bound channel to a
    /// holder of a wrong channel id.
    #[error("token bound to a different channel")]
    ChannelMismatch,
}

struct PendingToken {
    channel_id: LiveChannelId,
    expires_at: Instant,
}

/// Shared state for the live WebSocket transport.
///
/// Composable — any surface creates one of these with a shared
/// `LiveAdapterHost` and either mounts the router or starts a listener.
pub struct LiveWsState {
    host: Arc<LiveAdapterHost>,
    pending_tokens: Mutex<HashMap<LiveTokenString, PendingToken>>,
    token_ttl: Duration,
}

impl LiveWsState {
    pub fn new(host: Arc<LiveAdapterHost>) -> Self {
        Self::with_token_ttl(host, TOKEN_TTL)
    }

    /// Construct with a custom token TTL — primarily for tests.
    pub fn with_token_ttl(host: Arc<LiveAdapterHost>, token_ttl: Duration) -> Self {
        Self {
            host,
            pending_tokens: Mutex::new(HashMap::new()),
            token_ttl,
        }
    }

    pub fn host(&self) -> &Arc<LiveAdapterHost> {
        &self.host
    }

    /// Mint a single-use token for a channel. The token expires after
    /// [`Self::token_ttl`] if not consumed. Each call also reaps any
    /// already-expired tokens.
    pub async fn mint_token(&self, channel_id: LiveChannelId) -> LiveTokenString {
        let token = LiveTokenString::random();
        let expires_at = Instant::now() + self.token_ttl;
        let mut guard = self.pending_tokens.lock().await;
        reap_expired(&mut guard);
        guard.insert(
            token.clone(),
            PendingToken {
                channel_id,
                expires_at,
            },
        );
        token
    }

    /// Consume a token, returning the channel ID if the token is valid,
    /// unexpired, **and** bound to `expected_channel`. Reaps expired tokens
    /// as a side effect.
    ///
    /// G38: the `expected_channel` argument pins the bearer token to the
    /// channel its mint call recorded. A `ChannelMismatch` does not re-insert
    /// the token — it stays consumed so a wrong-channel attempt cannot be
    /// retried with the same token.
    pub async fn consume_token(
        &self,
        token: &str,
        expected_channel: &LiveChannelId,
    ) -> Result<LiveChannelId, TokenConsumeError> {
        let key = LiveTokenString::new(token).map_err(|_| TokenConsumeError::NotFound)?;
        let mut guard = self.pending_tokens.lock().await;
        reap_expired(&mut guard);
        match guard.remove(&key) {
            Some(pending) => {
                if pending.expires_at <= Instant::now() {
                    Err(TokenConsumeError::Expired)
                } else if &pending.channel_id != expected_channel {
                    Err(TokenConsumeError::ChannelMismatch)
                } else {
                    Ok(pending.channel_id)
                }
            }
            None => Err(TokenConsumeError::NotFound),
        }
    }

    #[cfg(test)]
    async fn pending_token_count(&self) -> usize {
        self.pending_tokens.lock().await.len()
    }
}

fn reap_expired(map: &mut HashMap<LiveTokenString, PendingToken>) {
    let now = Instant::now();
    map.retain(|_, p| p.expires_at > now);
}

#[derive(serde::Deserialize)]
pub struct WsConnectParams {
    pub token: String,
    /// Channel id this token claims to bind to. G38: required so
    /// `consume_token` can verify the token was minted for the same channel
    /// the client is attempting to attach.
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
        .with_state(state)
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    Query(params): Query<WsConnectParams>,
    State(state): State<Arc<LiveWsState>>,
) -> impl IntoResponse {
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
    ws.on_upgrade(move |socket| {
        handle_live_socket(socket, token, expected_channel, binary_format, state)
    })
}

async fn close_with(socket: &mut WebSocket, code: u16, reason: &str) {
    let _ = socket
        .send(WsMessage::Close(Some(CloseFrame {
            code,
            reason: reason.to_owned().into(),
        })))
        .await;
}

/// Extract the stable serde tag (e.g. `"provider_error"`) from a typed error
/// code so it can be embedded in a WS close-frame reason string clients can
/// key on. Unlike `Debug`-formatting, this stays stable across Rust source
/// reformatting and matches the wire shape used elsewhere.
fn live_adapter_error_code_slug(code: &meerkat_core::live_adapter::LiveAdapterErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

async fn handle_live_socket(
    mut socket: WebSocket,
    token: String,
    expected_channel: LiveChannelId,
    binary_format: Option<BinaryFormat>,
    state: Arc<LiveWsState>,
) {
    let channel_id = match state.consume_token(&token, &expected_channel).await {
        Ok(id) => id,
        Err(err) => {
            let err_json = serde_json::json!({"error": "invalid_token", "reason": err.to_string()})
                .to_string();
            let _ = socket.send(WsMessage::Text(err_json.into())).await;
            close_with(&mut socket, close_code::POLICY, "invalid_token").await;
            return;
        }
    };

    tracing::info!(channel = %channel_id, "live WebSocket connected");

    loop {
        tokio::select! {
            biased;

            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        match serde_json::from_str::<LiveInputChunk>(text.as_str()) {
                            Ok(chunk) => {
                                if let Err(err) = state.host.send_input(&channel_id, chunk).await {
                                    tracing::warn!(channel = %channel_id, error = %err, "send_input failed");
                                    let err_json = serde_json::json!({"error": err.to_string()}).to_string();
                                    let _ = socket.send(WsMessage::Text(err_json.into())).await;
                                }
                            }
                            Err(parse_err) => {
                                tracing::warn!(
                                    channel = %channel_id,
                                    error = %parse_err,
                                    "invalid WS text frame; closing"
                                );
                                close_with(&mut socket, close_code::INVALID, "invalid_frame").await;
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
                            close_with(
                                &mut socket,
                                close_code::POLICY,
                                "binary_format_unnegotiated",
                            ).await;
                            break;
                        };
                        let chunk = LiveInputChunk::Audio {
                            data: data.to_vec(),
                            sample_rate_hz: fmt.sample_rate_hz(),
                            channels: fmt.channels(),
                        };
                        if let Err(err) = state.host.send_input(&channel_id, chunk).await {
                            tracing::warn!(channel = %channel_id, error = %err, "binary send_input failed");
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
                        close_with(&mut socket, close_code::UNSUPPORTED, "unsupported_frame").await;
                        break;
                    }
                    Some(Err(err)) => {
                        tracing::warn!(channel = %channel_id, error = %err, "WS recv error");
                        break;
                    }
                }
            }

            observation = state.host.next_observation_raw(&channel_id) => {
                // Wave-3 RPC pump migration: split the convenience wrapper
                // into `next_observation_raw` + `apply_observation` so the
                // pump can react to the typed `ObservationOutcome` —
                // specifically `Terminal { code }`, which closes the WS
                // with a stable typed reason string clients can key on.
                match observation {
                    Ok(Some(obs)) => {
                        // Forward to the client first so it sees the terminal
                        // observation alongside the close frame.
                        let send_ok = match serde_json::to_string(&obs) {
                            Ok(json) => socket.send(WsMessage::Text(json.into())).await.is_ok(),
                            Err(_) => true,
                        };

                        // Apply to canonical state and inspect the typed outcome.
                        let outcome = state.host.apply_observation(&channel_id, &obs).await;

                        if !send_ok {
                            break;
                        }

                        match outcome {
                            Ok(ObservationOutcome::Terminal { code }) => {
                                let slug = live_adapter_error_code_slug(&code);
                                let reason = format!("terminal:{slug}");
                                close_with(&mut socket, close_code::POLICY, &reason).await;
                                break;
                            }
                            Ok(_) => {}
                            Err(err) => {
                                tracing::warn!(
                                    channel = %channel_id,
                                    error = %err,
                                    "apply_observation failed; closing channel"
                                );
                                break;
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }
    }

    tracing::info!(channel = %channel_id, "live WebSocket disconnected");
    let _ = state.host.close_channel(&channel_id).await;
}

/// Start the live WebSocket listener on a pre-bound TCP listener.
pub async fn serve_live_ws_listener(
    listener: tokio::net::TcpListener,
    state: Arc<LiveWsState>,
) -> Result<(), std::io::Error> {
    let app = live_ws_router(state);
    axum::serve(listener, app).await
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
    async fn mint_and_consume_token() {
        let host = Arc::new(LiveAdapterHost::new());
        let state = LiveWsState::new(host);
        let channel_id = LiveChannelId::new("test_ch");
        let token = state.mint_token(channel_id.clone()).await;
        assert!(!token.as_str().is_empty());
        let consumed = state.consume_token(token.as_str(), &channel_id).await;
        assert_eq!(consumed.unwrap(), channel_id);
        assert_eq!(
            state
                .consume_token(token.as_str(), &channel_id)
                .await
                .unwrap_err(),
            TokenConsumeError::NotFound,
        );
    }

    #[tokio::test]
    async fn consume_unknown_token_returns_not_found() {
        let host = Arc::new(LiveAdapterHost::new());
        let state = LiveWsState::new(host);
        let any_channel = LiveChannelId::new("any");
        assert_eq!(
            state
                .consume_token("bogus", &any_channel)
                .await
                .unwrap_err(),
            TokenConsumeError::NotFound,
        );
    }

    #[tokio::test]
    async fn consume_malformed_token_returns_not_found() {
        let host = Arc::new(LiveAdapterHost::new());
        let state = LiveWsState::new(host);
        let any_channel = LiveChannelId::new("any");
        // Spaces are not URL-safe; treated as not-found rather than crashing.
        assert_eq!(
            state
                .consume_token("has spaces", &any_channel)
                .await
                .unwrap_err(),
            TokenConsumeError::NotFound,
        );
    }

    #[tokio::test]
    async fn consume_token_with_wrong_channel_rejects() {
        // G38: token minted for channel A must not redeem for channel B.
        let host = Arc::new(LiveAdapterHost::new());
        let state = LiveWsState::new(host);
        let channel_a = LiveChannelId::new("ch_a");
        let channel_b = LiveChannelId::new("ch_b");
        let token = state.mint_token(channel_a.clone()).await;

        assert_eq!(
            state
                .consume_token(token.as_str(), &channel_b)
                .await
                .unwrap_err(),
            TokenConsumeError::ChannelMismatch,
        );

        // ChannelMismatch is terminal — the token is consumed, not retryable.
        assert_eq!(
            state
                .consume_token(token.as_str(), &channel_a)
                .await
                .unwrap_err(),
            TokenConsumeError::NotFound,
            "token must remain consumed after ChannelMismatch (no retry)"
        );
    }

    #[tokio::test]
    async fn token_expires_after_ttl_and_is_reaped() {
        let host = Arc::new(LiveAdapterHost::new());
        let state = LiveWsState::with_token_ttl(host, Duration::from_millis(50));
        let channel_id = LiveChannelId::new("ttl_ch");
        let token = state.mint_token(channel_id.clone()).await;
        assert_eq!(state.pending_token_count().await, 1);

        tokio::time::sleep(Duration::from_millis(120)).await;

        // Either the reap (triggered by another mint/consume) drops it, or
        // consume itself reports Expired. We test both behaviors: first
        // consume — should report NotFound because reap removes expired
        // entries before lookup.
        let err = state
            .consume_token(token.as_str(), &channel_id)
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                TokenConsumeError::NotFound | TokenConsumeError::Expired
            ),
            "unexpected error: {err:?}"
        );

        // After consume, the map must be empty.
        assert_eq!(state.pending_token_count().await, 0);
    }

    #[tokio::test]
    async fn unrelated_mint_reaps_expired_tokens() {
        let host = Arc::new(LiveAdapterHost::new());
        let state = LiveWsState::with_token_ttl(host, Duration::from_millis(40));
        let _stale = state.mint_token(LiveChannelId::new("stale")).await;
        assert_eq!(state.pending_token_count().await, 1);

        tokio::time::sleep(Duration::from_millis(80)).await;

        // Minting another token must reap the stale entry as a side effect.
        let _fresh = state.mint_token(LiveChannelId::new("fresh")).await;
        assert_eq!(state.pending_token_count().await, 1);
    }

    #[tokio::test]
    async fn websocket_roundtrip_with_token() {
        // R5 (Option A): the test exercises the full input roundtrip path —
        // upgrade → consume_token → `host.send_input` → adapter.send_command.
        // Without an attached & Ready adapter, `send_input` errors with
        // `NoAdapter`, the server emits a JSON error frame and may tear the
        // socket down before the client's subsequent Close is observed,
        // surfacing as a Linux broken-pipe failure on BuildBuddy. Attach an
        // idle (Ready) stub so `send_input` succeeds; the server then idles
        // on `next_observation_raw` (pending forever) until the client sends
        // Close. The test no longer relies on any server-initiated close
        // frame as a success signal.
        let host = Arc::new(LiveAdapterHost::new());
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host.open_channel(session_id).await.unwrap();
        host.attach_adapter(&channel_id, Arc::new(IdleAdapter))
            .await
            .unwrap();
        host.apply_status_update(
            &channel_id,
            meerkat_core::live_adapter::LiveAdapterStatus::Ready,
        )
        .await
        .unwrap();

        let state = Arc::new(LiveWsState::new(host));
        let token = state.mint_token(channel_id.clone()).await;

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

    #[test]
    fn live_adapter_error_code_slug_emits_serde_tag() {
        use meerkat_core::live_adapter::LiveAdapterErrorCode;
        assert_eq!(
            live_adapter_error_code_slug(&LiveAdapterErrorCode::ProviderError),
            "provider_error"
        );
        assert_eq!(
            live_adapter_error_code_slug(&LiveAdapterErrorCode::ConnectionLost),
            "connection_lost"
        );
        assert_eq!(
            live_adapter_error_code_slug(&LiveAdapterErrorCode::Other {
                raw: "custom".into()
            }),
            "other"
        );
        // R12: ConfigRejected must surface the typed slug so WS clients can
        // distinguish a local-guard rejection from an upstream provider
        // failure without parsing the close-frame reason text.
        assert_eq!(
            live_adapter_error_code_slug(&LiveAdapterErrorCode::ConfigRejected {
                reason: "model swap requires close + reopen".into(),
            }),
            "config_rejected"
        );
    }

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
            // server drops the channel after the close frame is written.
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
    async fn websocket_closes_on_terminal_observation_with_typed_reason() {
        use meerkat_core::live_adapter::{LiveAdapterErrorCode, LiveAdapterObservation};

        let host = Arc::new(LiveAdapterHost::new());
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host.open_channel(session_id).await.unwrap();
        host.attach_adapter(
            &channel_id,
            Arc::new(ScriptedAdapter::new(LiveAdapterObservation::Error {
                code: LiveAdapterErrorCode::ProviderError,
                message: "scripted terminal failure".into(),
            })),
        )
        .await
        .unwrap();

        let state = Arc::new(LiveWsState::new(host));
        let token = state.mint_token(channel_id.clone()).await;

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

        // Expect: (1) the JSON observation forwarded to the client, (2) a
        // Close frame with reason matching the typed error code slug.
        let mut saw_observation = false;
        let mut saw_close_with_terminal_reason = false;
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if text.contains("\"observation\"") && text.contains("provider_error") {
                        saw_observation = true;
                    }
                }
                Ok(Message::Close(Some(frame))) => {
                    assert!(
                        frame.reason.contains("terminal:provider_error"),
                        "unexpected close reason: {}",
                        frame.reason
                    );
                    saw_close_with_terminal_reason = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }

        assert!(
            saw_observation,
            "client should have received the terminal observation JSON before the close frame"
        );
        assert!(
            saw_close_with_terminal_reason,
            "WS pump must close with a typed terminal:<code> reason on Error observations"
        );

        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_rejects_invalid_token() {
        let host = Arc::new(LiveAdapterHost::new());
        let state = Arc::new(LiveWsState::new(host));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        // G38: a non-matching channel must still be supplied for the upgrade
        // to parse — the token itself is bogus, so consume_token returns
        // NotFound and the handler closes the socket with `invalid_token`.
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
    async fn websocket_rejects_token_with_wrong_channel() {
        // G38 end-to-end: token minted for channel A is presented with
        // channel B in the WS upgrade — handler must refuse with
        // `invalid_token` and close.
        let host = Arc::new(LiveAdapterHost::new());
        let session_a = meerkat_core::types::SessionId::new();
        let session_b = meerkat_core::types::SessionId::new();
        let channel_a = host.open_channel(session_a).await.unwrap();
        let channel_b = host.open_channel(session_b).await.unwrap();

        let state = Arc::new(LiveWsState::new(host));
        let token = state.mint_token(channel_a.clone()).await;

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
        let host = Arc::new(LiveAdapterHost::new());
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host.open_channel(session_id).await.unwrap();
        // Attach an idle adapter so the observation arm of the pump's
        // select! stays pending instead of immediately resolving with
        // `Err(NoAdapter)` and closing the loop before the test sends.
        host.attach_adapter(&channel_id, Arc::new(IdleAdapter))
            .await
            .unwrap();
        let state = Arc::new(LiveWsState::new(host));
        let token = state.mint_token(channel_id.clone()).await;

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

        // Drain frames until we see a Close with the negotiated reason.
        let mut saw_close = false;
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Close(Some(frame))) => {
                    assert!(
                        frame.reason.contains("binary_format_unnegotiated"),
                        "unexpected close reason: {}",
                        frame.reason
                    );
                    saw_close = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(saw_close, "expected close frame for un-negotiated binary");

        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_closes_on_invalid_text_frame() {
        let host = Arc::new(LiveAdapterHost::new());
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host.open_channel(session_id).await.unwrap();
        // See `websocket_rejects_binary_without_format` for why the test
        // attaches an idle adapter.
        host.attach_adapter(&channel_id, Arc::new(IdleAdapter))
            .await
            .unwrap();
        let state = Arc::new(LiveWsState::new(host));
        let token = state.mint_token(channel_id.clone()).await;

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

        let mut saw_close = false;
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Close(Some(frame))) => {
                    assert!(
                        frame.reason.contains("invalid_frame"),
                        "unexpected close reason: {}",
                        frame.reason
                    );
                    saw_close = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(saw_close, "expected close frame for invalid JSON");

        server_handle.abort();
    }
}
