//! Live WebSocket transport — bridges browser/test clients to `LiveAdapterHost`.
//!
//! Simple axum WebSocket server. Client sends `LiveInputChunk` JSON frames,
//! receives `LiveAdapterObservation` JSON frames. Token-based channel auth.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use meerkat_core::live_adapter::{LiveAdapterObservation, LiveAdapterStatus, LiveInputChunk};
use meerkat_runtime::live_adapter_host::{LiveAdapterHost, LiveChannelId};
use tokio::sync::Mutex;
use uuid::Uuid;

pub const LIVE_WS_PATH: &str = "/live/ws";

/// Shared state for the live WebSocket transport.
///
/// Composable — any surface creates one of these with a shared
/// `LiveAdapterHost` and either mounts the router or starts a listener.
pub struct LiveWsState {
    host: Arc<LiveAdapterHost>,
    pending_tokens: Mutex<HashMap<String, LiveChannelId>>,
}

impl LiveWsState {
    pub fn new(host: Arc<LiveAdapterHost>) -> Self {
        Self {
            host,
            pending_tokens: Mutex::new(HashMap::new()),
        }
    }

    pub fn host(&self) -> &Arc<LiveAdapterHost> {
        &self.host
    }

    /// Mint a single-use token for a channel. Returns the token string.
    pub async fn mint_token(&self, channel_id: LiveChannelId) -> String {
        let token = Uuid::new_v4().to_string();
        self.pending_tokens
            .lock()
            .await
            .insert(token.clone(), channel_id);
        token
    }

    /// Consume a token, returning the channel ID if valid.
    pub async fn consume_token(&self, token: &str) -> Option<LiveChannelId> {
        self.pending_tokens.lock().await.remove(token)
    }
}

#[derive(serde::Deserialize)]
pub struct WsConnectParams {
    pub token: String,
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
    let token = params.token;
    ws.on_upgrade(move |socket| handle_live_socket(socket, token, state))
}

async fn handle_live_socket(mut socket: WebSocket, token: String, state: Arc<LiveWsState>) {
    let channel_id = match state.consume_token(&token).await {
        Some(id) => id,
        None => {
            let err_json = serde_json::json!({"error": "invalid_token"}).to_string();
            let _ = socket.send(WsMessage::Text(err_json.into())).await;
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
                        if let Ok(chunk) = serde_json::from_str::<LiveInputChunk>(text.as_str()) {
                            if let Err(err) = state.host.send_input(&channel_id, chunk).await {
                                tracing::warn!(channel = %channel_id, error = %err, "send_input failed");
                                let err_json = serde_json::json!({"error": err.to_string()}).to_string();
                                let _ = socket.send(WsMessage::Text(err_json.into())).await;
                            }
                        }
                    }
                    Some(Ok(WsMessage::Binary(data))) => {
                        let chunk = LiveInputChunk::Audio {
                            data: data.to_vec(),
                            sample_rate_hz: 24000,
                            channels: 1,
                        };
                        if let Err(err) = state.host.send_input(&channel_id, chunk).await {
                            tracing::warn!(channel = %channel_id, error = %err, "binary send_input failed");
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    _ => {}
                }
            }

            observation = state.host.next_observation(&channel_id) => {
                match observation {
                    Ok(Some(obs)) => {
                        let is_closed = matches!(obs, LiveAdapterObservation::StatusChanged {
                            status: LiveAdapterStatus::Closed
                        });
                        if let Ok(json) = serde_json::to_string(&obs) {
                            if socket.send(WsMessage::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                        if is_closed {
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mint_and_consume_token() {
        let host = Arc::new(LiveAdapterHost::new());
        let state = LiveWsState::new(host);
        let channel_id = LiveChannelId::new("test_ch");
        let token = state.mint_token(channel_id.clone()).await;
        assert!(!token.is_empty());
        let consumed = state.consume_token(&token).await;
        assert_eq!(consumed, Some(channel_id));
        assert!(state.consume_token(&token).await.is_none());
    }

    #[tokio::test]
    async fn consume_unknown_token_returns_none() {
        let host = Arc::new(LiveAdapterHost::new());
        let state = LiveWsState::new(host);
        assert!(state.consume_token("bogus").await.is_none());
    }

    #[tokio::test]
    async fn websocket_roundtrip_with_token() {
        let host = Arc::new(LiveAdapterHost::new());
        let session_id = meerkat_core::types::SessionId::new();
        let channel_id = host.open_channel(session_id).await.unwrap();

        let state = Arc::new(LiveWsState::new(host));
        let token = state.mint_token(channel_id).await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token={token}");
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

    #[tokio::test]
    async fn websocket_rejects_invalid_token() {
        let host = Arc::new(LiveAdapterHost::new());
        let state = Arc::new(LiveWsState::new(host));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_state = Arc::clone(&state);
        let server_handle =
            tokio::spawn(async move { serve_live_ws_listener(listener, ws_state).await });

        let url = format!("ws://{addr}{LIVE_WS_PATH}?token=bogus");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (_write, mut read) = futures::StreamExt::split(ws_stream);

        use futures::StreamExt;
        if let Some(Ok(msg)) = read.next().await {
            let text = msg.into_text().unwrap();
            assert!(text.contains("invalid_token"));
        }

        server_handle.abort();
    }
}
