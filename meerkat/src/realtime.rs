#[cfg(not(target_arch = "wasm32"))]
use crate::contracts::{
    RealtimeChannelOpenFrame, RealtimeChannelOpenedFrame, RealtimeClientFrame, RealtimeOpenInfo,
    RealtimeServerFrame,
};
use crate::contracts::{
    RealtimeChannelRole, RealtimeChannelTarget, RealtimeOpenRequest, RealtimeReconnectPolicy,
    RealtimeTurningMode,
};

#[cfg(not(target_arch = "wasm32"))]
use futures::stream::{SplitSink, SplitStream};
#[cfg(not(target_arch = "wasm32"))]
use futures::{SinkExt, StreamExt};
#[cfg(not(target_arch = "wasm32"))]
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

/// Public Rust SDK scaffold for building a realtime channel open request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeChannel {
    target: RealtimeChannelTarget,
    role: RealtimeChannelRole,
    turning_mode: RealtimeTurningMode,
    reconnect_policy: Option<RealtimeReconnectPolicy>,
}

impl RealtimeChannel {
    pub fn session(session_id: impl Into<String>) -> Self {
        Self {
            target: RealtimeChannelTarget::SessionTarget {
                session_id: session_id.into(),
            },
            role: RealtimeChannelRole::Primary,
            turning_mode: RealtimeTurningMode::ProviderManaged,
            reconnect_policy: None,
        }
    }

    pub fn role(mut self, role: RealtimeChannelRole) -> Self {
        self.role = role;
        self
    }

    pub fn turning_mode(mut self, turning_mode: RealtimeTurningMode) -> Self {
        self.turning_mode = turning_mode;
        self
    }

    pub fn reconnect_policy(mut self, reconnect_policy: RealtimeReconnectPolicy) -> Self {
        self.reconnect_policy = Some(reconnect_policy);
        self
    }

    pub fn target(&self) -> &RealtimeChannelTarget {
        &self.target
    }

    pub fn open_request(&self) -> RealtimeOpenRequest {
        RealtimeOpenRequest {
            target: self.target.clone(),
            role: self.role,
            turning_mode: self.turning_mode,
            reconnect_policy: self.reconnect_policy.clone(),
            channel_config: None,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn connect(
        &self,
        open_info: &RealtimeOpenInfo,
    ) -> Result<RealtimeConnection, RealtimeConnectionError> {
        if open_info.target != self.target {
            return Err(RealtimeConnectionError::TargetMismatch);
        }
        let open_frame = RealtimeChannelOpenFrame {
            protocol_version: open_info.default_protocol_version.clone(),
            open_token: open_info.open_token.clone(),
            role: self.role,
            turning_mode: self.turning_mode,
        };
        let (connection, _opened) = RealtimeConnection::open(open_info, open_frame).await?;
        Ok(connection)
    }
}

#[cfg(not(target_arch = "wasm32"))]
type RealtimeSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
#[cfg(not(target_arch = "wasm32"))]
type RealtimeSink = SplitSink<RealtimeSocket, Message>;
#[cfg(not(target_arch = "wasm32"))]
type RealtimeStream = SplitStream<RealtimeSocket>;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct RealtimeConnection {
    sink: RealtimeSink,
    stream: RealtimeStream,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct RealtimeConnectionSender {
    sink: RealtimeSink,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct RealtimeConnectionReceiver {
    stream: RealtimeStream,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, thiserror::Error)]
pub enum RealtimeConnectionError {
    #[error("realtime open target does not match the channel target")]
    TargetMismatch,
    #[error(
        "default realtime protocol version '{requested}' is not supported (supported: {supported:?})"
    )]
    UnsupportedProtocolVersion {
        requested: String,
        supported: Vec<String>,
    },
    #[error("realtime websocket rejected channel open with {code:?}: {message}")]
    OpenRejected {
        code: meerkat_contracts::RealtimeErrorCode,
        message: String,
    },
    #[error("realtime websocket returned unexpected open frame '{frame_type}'")]
    UnexpectedOpenFrame { frame_type: String },
    #[error("realtime websocket transport closed")]
    TransportClosed,
    #[error("realtime websocket frame was not valid UTF-8")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("realtime websocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("realtime websocket json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(not(target_arch = "wasm32"))]
impl RealtimeConnection {
    pub async fn open(
        open_info: &RealtimeOpenInfo,
        open_frame: RealtimeChannelOpenFrame,
    ) -> Result<(Self, RealtimeChannelOpenedFrame), RealtimeConnectionError> {
        if !open_info
            .supported_protocol_versions
            .iter()
            .any(|version| version == &open_frame.protocol_version)
        {
            return Err(RealtimeConnectionError::UnsupportedProtocolVersion {
                requested: open_frame.protocol_version,
                supported: open_info.supported_protocol_versions.clone(),
            });
        }

        let (mut socket, _response) = connect_async(&open_info.ws_url).await?;
        let open_message = serde_json::to_string(&RealtimeClientFrame::ChannelOpen(open_frame))?;
        socket.send(Message::Text(open_message.into())).await?;

        match Self::read_next_frame_from_socket(&mut socket).await? {
            Some(RealtimeServerFrame::ChannelOpened(frame)) => {
                let (sink, stream) = socket.split();
                Ok((Self { sink, stream }, frame))
            }
            Some(RealtimeServerFrame::ChannelError(error)) => {
                Err(RealtimeConnectionError::OpenRejected {
                    code: error.code,
                    message: error.message,
                })
            }
            Some(other) => Err(RealtimeConnectionError::UnexpectedOpenFrame {
                frame_type: server_frame_type(&other).to_string(),
            }),
            None => Err(RealtimeConnectionError::TransportClosed),
        }
    }

    pub async fn send_frame(
        &mut self,
        frame: RealtimeClientFrame,
    ) -> Result<(), RealtimeConnectionError> {
        let payload = serde_json::to_string(&frame)?;
        self.sink.send(Message::Text(payload.into())).await?;
        Ok(())
    }

    pub async fn send_input(
        &mut self,
        chunk: crate::contracts::RealtimeInputChunk,
    ) -> Result<(), RealtimeConnectionError> {
        self.send_frame(RealtimeClientFrame::ChannelInput(
            crate::contracts::RealtimeChannelInputFrame { chunk },
        ))
        .await
    }

    pub async fn commit_turn(&mut self) -> Result<(), RealtimeConnectionError> {
        self.send_frame(RealtimeClientFrame::ChannelCommitTurn)
            .await
    }

    pub async fn interrupt(&mut self) -> Result<(), RealtimeConnectionError> {
        self.send_frame(RealtimeClientFrame::ChannelInterrupt).await
    }

    pub async fn close(&mut self) -> Result<(), RealtimeConnectionError> {
        self.send_frame(RealtimeClientFrame::ChannelClose).await?;
        loop {
            match self.next_frame().await? {
                Some(RealtimeServerFrame::ChannelClosed(_)) | None => break,
                Some(_) => {}
            }
        }
        self.sink.close().await?;
        Ok(())
    }

    pub async fn next_frame(
        &mut self,
    ) -> Result<Option<RealtimeServerFrame>, RealtimeConnectionError> {
        Self::read_next_frame_from_stream(&mut self.stream).await
    }

    pub fn split(self) -> (RealtimeConnectionSender, RealtimeConnectionReceiver) {
        (
            RealtimeConnectionSender { sink: self.sink },
            RealtimeConnectionReceiver {
                stream: self.stream,
            },
        )
    }

    async fn read_next_frame_from_stream(
        stream: &mut RealtimeStream,
    ) -> Result<Option<RealtimeServerFrame>, RealtimeConnectionError> {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    return Ok(Some(serde_json::from_str(text.as_ref())?));
                }
                Some(Ok(Message::Binary(bytes))) => {
                    return Ok(Some(serde_json::from_str(std::str::from_utf8(&bytes)?)?));
                }
                Some(Ok(Message::Close(_))) | None => return Ok(None),
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
                Some(Err(error)) => return Err(error.into()),
            }
        }
    }

    async fn read_next_frame_from_socket(
        socket: &mut RealtimeSocket,
    ) -> Result<Option<RealtimeServerFrame>, RealtimeConnectionError> {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    return Ok(Some(serde_json::from_str(text.as_ref())?));
                }
                Some(Ok(Message::Binary(bytes))) => {
                    return Ok(Some(serde_json::from_str(std::str::from_utf8(&bytes)?)?));
                }
                Some(Ok(Message::Close(_))) | None => return Ok(None),
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
                Some(Err(error)) => return Err(error.into()),
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl RealtimeConnectionSender {
    pub async fn send_frame(
        &mut self,
        frame: RealtimeClientFrame,
    ) -> Result<(), RealtimeConnectionError> {
        let payload = serde_json::to_string(&frame)?;
        self.sink.send(Message::Text(payload.into())).await?;
        Ok(())
    }

    pub async fn send_input(
        &mut self,
        chunk: crate::contracts::RealtimeInputChunk,
    ) -> Result<(), RealtimeConnectionError> {
        self.send_frame(RealtimeClientFrame::ChannelInput(
            crate::contracts::RealtimeChannelInputFrame { chunk },
        ))
        .await
    }

    pub async fn commit_turn(&mut self) -> Result<(), RealtimeConnectionError> {
        self.send_frame(RealtimeClientFrame::ChannelCommitTurn)
            .await
    }

    pub async fn interrupt(&mut self) -> Result<(), RealtimeConnectionError> {
        self.send_frame(RealtimeClientFrame::ChannelInterrupt).await
    }

    pub async fn close(&mut self) -> Result<(), RealtimeConnectionError> {
        self.send_frame(RealtimeClientFrame::ChannelClose).await?;
        self.sink.close().await?;
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl RealtimeConnectionReceiver {
    pub async fn next_frame(
        &mut self,
    ) -> Result<Option<RealtimeServerFrame>, RealtimeConnectionError> {
        RealtimeConnection::read_next_frame_from_stream(&mut self.stream).await
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn server_frame_type(frame: &RealtimeServerFrame) -> &'static str {
    match frame {
        RealtimeServerFrame::ChannelOpened(_) => "channel.opened",
        RealtimeServerFrame::ChannelStatus(_) => "channel.status",
        RealtimeServerFrame::ChannelEvent(_) => "channel.event",
        RealtimeServerFrame::ChannelError(_) => "channel.error",
        RealtimeServerFrame::ChannelClosed(_) => "channel.closed",
    }
}
