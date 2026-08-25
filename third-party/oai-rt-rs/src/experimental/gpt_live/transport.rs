use super::codec::{
    CodecError, decode_server_event, encode_client_event, encode_create_call_request,
};
use super::protocol::{
    ClientEvent, CreateCallRequest, CreateCallResponse, ProviderCallId, ServerEvent,
};
use super::redaction::{Direction, TerminalClass, WireSummary};
use futures::{SinkExt, StreamExt};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, LOCATION};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Error as TungsteniteError;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use url::Url;

const OPENAI_ALPHA: &str = "quicksilver=v2";
const DEFAULT_CALL_URL: &str =
    "https://chatgpt.com/backend-api/codex/realtime/calls?intent=quicksilver&architecture=avas";
const DEFAULT_SIDEBAND_BASE_URL: &str = "wss://api.openai.com/v1/live/";
const MAX_ANSWER_SDP_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpFailureClass {
    Builder,
    Request,
    Connect,
    Timeout,
    Redirect,
    Status,
    Body,
    Decode,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSocketFailureClass {
    Closed,
    Io,
    Tls,
    Capacity,
    Protocol,
    Backpressure,
    Utf8,
    AttackAttempt,
    Url,
    Handshake,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("private realtime codec failure")]
    Codec(#[from] CodecError),
    #[error("private realtime HTTP transport failure")]
    Http(HttpFailureClass),
    #[error("private realtime WebSocket transport failure")]
    WebSocket(WebSocketFailureClass),
    #[error("private realtime endpoint is invalid")]
    InvalidEndpoint,
    #[error("private realtime header {0} is invalid")]
    InvalidHeader(&'static str),
    #[error("private realtime call returned HTTP status {0}")]
    UnexpectedStatus(reqwest::StatusCode),
    #[error("private realtime call returned an unexpected content type")]
    UnexpectedContentType,
    #[error("private realtime call did not return a provider call location")]
    MissingCallLocation,
    #[error("private realtime call returned an invalid provider call location")]
    InvalidCallLocation,
    #[error("private realtime call returned an oversized SDP answer")]
    OversizedAnswer,
    #[error("private realtime call returned a non-UTF-8 SDP answer")]
    InvalidAnswerEncoding,
    #[error("private realtime sideband closed")]
    Closed,
}

pub struct GptLiveCredentials {
    bearer_token: String,
    sideband: SidebandHeaders,
}

impl GptLiveCredentials {
    #[must_use]
    pub fn new(bearer_token: impl Into<String>, sideband: SidebandHeaders) -> Self {
        Self {
            bearer_token: bearer_token.into(),
            sideband,
        }
    }
}

impl fmt::Debug for GptLiveCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GptLiveCredentials")
            .field("bearer_token", &"<redacted>")
            .field("sideband", &self.sideband)
            .finish()
    }
}

#[derive(Default)]
pub struct SidebandHeaders {
    pub account_id: Option<String>,
    pub attestation: Option<String>,
    pub originator: Option<String>,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub version: Option<String>,
    pub x_session_id: Option<String>,
    pub user_agent: Option<String>,
}

impl fmt::Debug for SidebandHeaders {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SidebandHeaders")
            .field(
                "account_id",
                &self.account_id.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "attestation",
                &self.attestation.as_ref().map(|_| "<redacted>"),
            )
            .field("originator", &self.originator.as_ref().map(|_| "<present>"))
            .field(
                "session_id",
                &self.session_id.as_ref().map(|_| "<redacted>"),
            )
            .field("thread_id", &self.thread_id.as_ref().map(|_| "<redacted>"))
            .field("version", &self.version.as_ref().map(|_| "<present>"))
            .field(
                "x_session_id",
                &self.x_session_id.as_ref().map(|_| "<redacted>"),
            )
            .field("user_agent", &self.user_agent.as_ref().map(|_| "<present>"))
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct GptLiveEndpoints {
    call_url: Url,
    sideband_base_url: Url,
}

impl GptLiveEndpoints {
    /// Build endpoint overrides, primarily for deterministic local testing.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::InvalidEndpoint`] when either URL is invalid.
    pub fn new(call_url: &str, sideband_base_url: &str) -> Result<Self, TransportError> {
        Ok(Self {
            call_url: Url::parse(call_url).map_err(|_| TransportError::InvalidEndpoint)?,
            sideband_base_url: Url::parse(sideband_base_url)
                .map_err(|_| TransportError::InvalidEndpoint)?,
        })
    }
}

impl Default for GptLiveEndpoints {
    fn default() -> Self {
        Self::new(DEFAULT_CALL_URL, DEFAULT_SIDEBAND_BASE_URL)
            .expect("static GPT Live endpoints must be valid")
    }
}

#[derive(Debug, Clone)]
pub struct GptLiveTransport {
    client: reqwest::Client,
    endpoints: GptLiveEndpoints,
}

impl GptLiveTransport {
    /// Build a transport for the captured private endpoints.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the HTTP client cannot be constructed.
    pub fn try_new() -> Result<Self, TransportError> {
        Self::with_endpoints(GptLiveEndpoints::default())
    }

    /// Build a transport using explicit endpoints.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the HTTP client cannot be constructed.
    pub fn with_endpoints(endpoints: GptLiveEndpoints) -> Result<Self, TransportError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| http_failure(&error))?;
        Ok(Self { client, endpoints })
    }

    /// Create the private call and return its SDP answer plus opaque call ID.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] for request, response, header, or contract
    /// failures. Response bodies are never included in the error.
    pub async fn create_call(
        &self,
        request: &CreateCallRequest,
        credentials: &GptLiveCredentials,
    ) -> Result<CreateCallResponse, TransportError> {
        let body = encode_create_call_request(request)?;
        WireSummary::event(Direction::ToOpenAi, "call.create", body.len()).emit();

        let authorization = bearer_header(&credentials.bearer_token, "authorization")?;
        let response = self
            .client
            .post(self.endpoints.call_url.clone())
            .header(AUTHORIZATION, authorization)
            .header("OpenAI-Alpha", OPENAI_ALPHA)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|source| {
                WireSummary::terminal(Direction::FromOpenAi, "call.create", 0, TerminalClass::Http)
                    .emit();
                http_failure(&source)
            })?;

        if response.status() != reqwest::StatusCode::CREATED {
            let status = response.status();
            WireSummary::terminal(Direction::FromOpenAi, "call.create", 0, TerminalClass::Http)
                .emit();
            return Err(TransportError::UnexpectedStatus(status));
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !content_type
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/plain"))
        {
            return Err(TransportError::UnexpectedContentType);
        }
        let call_id = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(TransportError::MissingCallLocation)
            .and_then(extract_call_id)?;
        let answer = response
            .bytes()
            .await
            .map_err(|error| http_failure(&error))?;
        if answer.len() > MAX_ANSWER_SDP_BYTES {
            return Err(TransportError::OversizedAnswer);
        }
        let answer_sdp = String::from_utf8(answer.to_vec())
            .map_err(|_| TransportError::InvalidAnswerEncoding)?;
        WireSummary::event(Direction::FromOpenAi, "call.created", answer_sdp.len()).emit();
        Ok(CreateCallResponse {
            answer_sdp,
            call_id,
        })
    }

    /// Attach the private sideband using caller-supplied resolved credentials.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] for invalid headers, endpoints, or handshake
    /// failure.
    pub async fn connect_sideband(
        &self,
        call_id: &ProviderCallId,
        credentials: &GptLiveCredentials,
    ) -> Result<Sideband, TransportError> {
        let url = self
            .endpoints
            .sideband_base_url
            .join(call_id.as_str())
            .map_err(|_| TransportError::InvalidEndpoint)?;
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|error| websocket_failure(&error))?;
        let headers = request.headers_mut();
        headers.insert(
            AUTHORIZATION,
            bearer_header(&credentials.bearer_token, "authorization")?,
        );
        insert_header(headers, "openai-alpha", Some(OPENAI_ALPHA))?;
        let sideband = &credentials.sideband;
        insert_header(
            headers,
            "chatgpt-account-id",
            sideband.account_id.as_deref(),
        )?;
        insert_header(
            headers,
            "x-oai-attestation",
            sideband.attestation.as_deref(),
        )?;
        insert_header(headers, "originator", sideband.originator.as_deref())?;
        insert_header(headers, "session-id", sideband.session_id.as_deref())?;
        insert_header(headers, "thread-id", sideband.thread_id.as_deref())?;
        insert_header(headers, "version", sideband.version.as_deref())?;
        insert_header(headers, "x-session-id", sideband.x_session_id.as_deref())?;
        insert_header(headers, "user-agent", sideband.user_agent.as_deref())?;

        let (stream, _) = connect_async(request).await.map_err(|source| {
            WireSummary::terminal(
                Direction::FromOpenAi,
                "sideband.connect",
                0,
                TerminalClass::WebSocket,
            )
            .emit();
            websocket_failure(&source)
        })?;
        WireSummary::event(Direction::FromOpenAi, "sideband.connected", 0).emit();
        Ok(Sideband { stream })
    }
}

pub struct Sideband {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

type SidebandSink = futures::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
type SidebandStream = futures::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

/// Concurrent send half of a private sideband connection.
#[derive(Clone)]
pub struct SidebandSender {
    sink: Arc<Mutex<SidebandSink>>,
}

impl fmt::Debug for SidebandSender {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SidebandSender(<private realtime connection>)")
    }
}

/// Concurrent receive half of a private sideband connection.
pub struct SidebandReceiver {
    stream: SidebandStream,
    sink: Arc<Mutex<SidebandSink>>,
}

impl fmt::Debug for SidebandReceiver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SidebandReceiver(<private realtime connection>)")
    }
}

impl fmt::Debug for Sideband {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Sideband(<private realtime connection>)")
    }
}

impl Sideband {
    /// Split this connection into independently awaitable send and receive
    /// halves. The receive half shares only the sink needed for mechanical
    /// ping replies; it never holds that lock while awaiting the next frame.
    #[must_use]
    pub fn split(self) -> (SidebandSender, SidebandReceiver) {
        let (sink, stream) = self.stream.split();
        let sink = Arc::new(Mutex::new(sink));
        (
            SidebandSender {
                sink: Arc::clone(&sink),
            },
            SidebandReceiver { stream, sink },
        )
    }

    /// Send one verified private client event.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when encoding or WebSocket transmission fails.
    pub async fn send(&mut self, event: &ClientEvent) -> Result<(), TransportError> {
        let text = encode_client_event(event)?;
        let kind = match event {
            ClientEvent::SessionContextAppend(_) => "session.context.append",
            ClientEvent::DelegationContextAppend(_) => "delegation.context.append",
        };
        WireSummary::event(Direction::ToOpenAi, kind, text.len()).emit();
        self.stream
            .send(Message::Text(text.into()))
            .await
            .map_err(|error| websocket_failure(&error))
    }

    /// Receive and decode the next text event, handling ping frames mechanically.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the WebSocket or private codec fails.
    pub async fn next_event(&mut self) -> Result<Option<ServerEvent>, TransportError> {
        while let Some(message) = self.stream.next().await {
            let message = match message {
                Ok(message) => message,
                Err(tokio_tungstenite::tungstenite::Error::ConnectionClosed) => return Ok(None),
                Err(source) => return Err(websocket_failure(&source)),
            };
            match message {
                Message::Text(text) => {
                    let decoded = decode_server_event(&text).map_err(|source| {
                        WireSummary::terminal(
                            Direction::FromOpenAi,
                            "malformed",
                            text.len(),
                            TerminalClass::Codec,
                        )
                        .emit();
                        TransportError::Codec(source)
                    })?;
                    WireSummary::event(
                        Direction::FromOpenAi,
                        loggable_server_event_kind(&decoded),
                        text.len(),
                    )
                    .emit();
                    return Ok(Some(decoded));
                }
                Message::Close(_) => {
                    WireSummary::terminal(
                        Direction::FromOpenAi,
                        "sideband.closed",
                        0,
                        TerminalClass::Closed,
                    )
                    .emit();
                    return Ok(None);
                }
                Message::Ping(payload) => self
                    .stream
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|error| websocket_failure(&error))?,
                Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
        Ok(None)
    }

    /// Close the private sideband mechanically.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the WebSocket close frame cannot be sent.
    pub async fn close(&mut self) -> Result<(), TransportError> {
        self.stream
            .close(None)
            .await
            .map_err(|error| websocket_failure(&error))?;
        WireSummary::terminal(
            Direction::ToOpenAi,
            "sideband.closed",
            0,
            TerminalClass::Closed,
        )
        .emit();
        Ok(())
    }
}

impl SidebandSender {
    /// Send one verified private client event without blocking an outstanding
    /// receive operation.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when encoding fails or the WebSocket message
    /// cannot be sent.
    pub async fn send(&self, event: &ClientEvent) -> Result<(), TransportError> {
        let text = encode_client_event(event)?;
        let kind = match event {
            ClientEvent::SessionContextAppend(_) => "session.context.append",
            ClientEvent::DelegationContextAppend(_) => "delegation.context.append",
        };
        WireSummary::event(Direction::ToOpenAi, kind, text.len()).emit();
        self.sink
            .lock()
            .await
            .send(Message::Text(text.into()))
            .await
            .map_err(|error| websocket_failure(&error))
    }

    /// Close the private sideband mechanically.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the WebSocket close frame cannot be sent.
    pub async fn close(&self) -> Result<(), TransportError> {
        self.sink
            .lock()
            .await
            .close()
            .await
            .map_err(|error| websocket_failure(&error))?;
        WireSummary::terminal(
            Direction::ToOpenAi,
            "sideband.closed",
            0,
            TerminalClass::Closed,
        )
        .emit();
        Ok(())
    }
}

impl SidebandReceiver {
    /// Receive and decode the next text event while the send half remains
    /// independently usable.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when receiving a WebSocket frame fails or a
    /// known private event is malformed.
    pub async fn next_event(&mut self) -> Result<Option<ServerEvent>, TransportError> {
        while let Some(message) = self.stream.next().await {
            let message = match message {
                Ok(message) => message,
                Err(tokio_tungstenite::tungstenite::Error::ConnectionClosed) => return Ok(None),
                Err(source) => return Err(websocket_failure(&source)),
            };
            match message {
                Message::Text(text) => {
                    let decoded = decode_server_event(&text).map_err(|source| {
                        WireSummary::terminal(
                            Direction::FromOpenAi,
                            "malformed",
                            text.len(),
                            TerminalClass::Codec,
                        )
                        .emit();
                        TransportError::Codec(source)
                    })?;
                    WireSummary::event(
                        Direction::FromOpenAi,
                        loggable_server_event_kind(&decoded),
                        text.len(),
                    )
                    .emit();
                    return Ok(Some(decoded));
                }
                Message::Close(_) => {
                    WireSummary::terminal(
                        Direction::FromOpenAi,
                        "sideband.closed",
                        0,
                        TerminalClass::Closed,
                    )
                    .emit();
                    return Ok(None);
                }
                Message::Ping(payload) => self
                    .sink
                    .lock()
                    .await
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|error| websocket_failure(&error))?,
                Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
        Ok(None)
    }
}

fn bearer_header(token: &str, name: &'static str) -> Result<HeaderValue, TransportError> {
    HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| TransportError::InvalidHeader(name))
}

fn http_failure(error: &reqwest::Error) -> TransportError {
    let class = if error.is_builder() {
        HttpFailureClass::Builder
    } else if error.is_timeout() {
        HttpFailureClass::Timeout
    } else if error.is_connect() {
        HttpFailureClass::Connect
    } else if error.is_redirect() {
        HttpFailureClass::Redirect
    } else if error.is_status() {
        HttpFailureClass::Status
    } else if error.is_body() {
        HttpFailureClass::Body
    } else if error.is_decode() {
        HttpFailureClass::Decode
    } else if error.is_request() {
        HttpFailureClass::Request
    } else {
        HttpFailureClass::Other
    };
    TransportError::Http(class)
}

const fn websocket_failure(error: &TungsteniteError) -> TransportError {
    let class = match error {
        TungsteniteError::ConnectionClosed | TungsteniteError::AlreadyClosed => {
            WebSocketFailureClass::Closed
        }
        TungsteniteError::Io(_) => WebSocketFailureClass::Io,
        TungsteniteError::Tls(_) => WebSocketFailureClass::Tls,
        TungsteniteError::Capacity(_) => WebSocketFailureClass::Capacity,
        TungsteniteError::Protocol(_) => WebSocketFailureClass::Protocol,
        TungsteniteError::WriteBufferFull(_) => WebSocketFailureClass::Backpressure,
        TungsteniteError::Utf8 => WebSocketFailureClass::Utf8,
        TungsteniteError::AttackAttempt => WebSocketFailureClass::AttackAttempt,
        TungsteniteError::Url(_) => WebSocketFailureClass::Url,
        TungsteniteError::Http(_) | TungsteniteError::HttpFormat(_) => {
            WebSocketFailureClass::Handshake
        }
    };
    TransportError::WebSocket(class)
}

fn loggable_server_event_kind(event: &ServerEvent) -> &str {
    if matches!(event, ServerEvent::Unknown(_)) {
        "unknown"
    } else {
        event.kind()
    }
}

fn insert_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: Option<&str>,
) -> Result<(), TransportError> {
    if let Some(value) = value {
        let value =
            HeaderValue::from_str(value).map_err(|_| TransportError::InvalidHeader(name))?;
        headers.insert(HeaderName::from_static(name), value);
    }
    Ok(())
}

fn extract_call_id(location: &str) -> Result<ProviderCallId, TransportError> {
    let id = location
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| value.starts_with("rtc_") && value.len() > 4)
        .ok_or(TransportError::InvalidCallLocation)?;
    Ok(ProviderCallId::new(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_server_discriminant_is_not_loggable() {
        let event = decode_server_event(
            r#"{"type":"FIXTURE_PRIVATE_UNKNOWN_KIND","secret":"FIXTURE_PRIVATE_SECRET"}"#,
        )
        .expect("unknown event");
        assert_eq!(loggable_server_event_kind(&event), "unknown");
    }
}
