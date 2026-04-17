use crate::transport::ws::WsStream;
use futures::stream::ReuniteError;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_tungstenite::tungstenite::protocol::Message;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApiErrorType {
    InvalidRequestError,
    RateLimitError,
    AuthenticationError,
    ServerError,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ServerError {
    #[serde(rename = "type")]
    pub error_type: ApiErrorType,
    pub code: Option<String>,
    pub message: String,
    /// Error parameter is a string field name in GA responses.
    pub param: Option<String>,
    pub event_id: Option<String>,
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("HTTP protocol error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Failed to parse or serialize JSON: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Invalid URL: {0}")]
    Url(#[from] url::ParseError),

    #[error("Header error: {0}")]
    Header(#[from] reqwest::header::InvalidHeaderValue),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("OpenAI API error: {0:?}")]
    Api(ServerError),

    #[error("The connection was closed unexpectedly")]
    ConnectionClosed,

    #[error("Failed to reunite split client: {0}")]
    Reunite(#[from] ReuniteError<WsStream, Message>),

    #[error("MIME type error: {0}")]
    Mime(String),

    #[error("Invalid client event: {0}")]
    InvalidClientEvent(String),

    #[error("Not implemented: {0}")]
    NotImplemented(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;
