use async_trait::async_trait;
use futures::{Stream, StreamExt, TryStreamExt};
use reqwest::Method;
use std::pin::Pin;

#[cfg(not(target_arch = "wasm32"))]
use std::error::Error as StdError;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type TransportByteStream =
    Pin<Box<dyn Stream<Item = Result<Vec<u8>, TransportError>> + Send + 'static>>;

#[cfg(target_arch = "wasm32")]
pub(crate) type TransportByteStream =
    Pin<Box<dyn Stream<Item = Result<Vec<u8>, TransportError>> + 'static>>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TransportHeaders(Vec<(String, String)>);

impl TransportHeaders {
    pub(crate) fn insert(
        &mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> &mut Self {
        let name = name.into().to_ascii_lowercase();
        let value = value.into();
        if let Some((_, existing)) = self.0.iter_mut().find(|(key, _)| *key == name) {
            *existing = value;
        } else {
            self.0.push((name, value));
        }
        self
    }

    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.0
            .iter()
            .find(|(key, _)| key == &name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }
}

impl From<&reqwest::header::HeaderMap> for TransportHeaders {
    fn from(headers: &reqwest::header::HeaderMap) -> Self {
        let mut result = Self::default();
        for (name, value) in headers {
            if let Ok(value) = value.to_str() {
                result.insert(name.as_str(), value);
            }
        }
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransportRequest {
    pub(crate) method: Method,
    pub(crate) url: String,
    pub(crate) headers: TransportHeaders,
    pub(crate) body: Vec<u8>,
}

impl TransportRequest {
    pub(crate) fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: TransportHeaders::default(),
            body: Vec::new(),
        }
    }

    pub(crate) fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name, value);
        self
    }

    pub(crate) fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }
}

pub(crate) struct TransportResponse {
    pub(crate) status: u16,
    pub(crate) headers: TransportHeaders,
    body: TransportByteStream,
}

impl TransportResponse {
    pub(crate) fn new(status: u16, headers: TransportHeaders, body: TransportByteStream) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    pub(crate) fn into_body(self) -> TransportByteStream {
        self.body
    }

    pub(crate) async fn into_text(self) -> Result<String, TransportError> {
        let bytes = self
            .body
            .try_fold(Vec::new(), |mut acc, chunk| async move {
                acc.extend_from_slice(&chunk);
                Ok(acc)
            })
            .await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TransportError {
    #[error("network timeout after {duration_ms}ms")]
    Timeout { duration_ms: u64 },

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    #[error("connection reset")]
    ConnectionReset,

    #[error("transport error: {message}")]
    Other { message: String },
}

impl TransportError {
    #[cfg(not(target_arch = "wasm32"))]
    fn source_chain_contains_connection_reset(err: &(dyn StdError + 'static)) -> bool {
        let mut current = Some(err);
        while let Some(error) = current {
            if let Some(io) = error.downcast_ref::<std::io::Error>()
                && matches!(
                    io.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::UnexpectedEof
                )
            {
                return true;
            }
            current = error.source();
        }
        false
    }

    fn from_reqwest_error(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            Self::Timeout { duration_ms: 30000 }
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            if Self::source_chain_contains_connection_reset(&err) {
                return Self::ConnectionReset;
            }
            Self::Other {
                message: err.to_string(),
            }
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub(crate) trait TransportClient: Send + Sync {
    async fn execute(&self, request: TransportRequest)
    -> Result<TransportResponse, TransportError>;
}

pub(crate) struct ReqwestTransportClient {
    http: reqwest::Client,
}

impl ReqwestTransportClient {
    pub(crate) fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl TransportClient for ReqwestTransportClient {
    async fn execute(
        &self,
        request: TransportRequest,
    ) -> Result<TransportResponse, TransportError> {
        let mut builder = self.http.request(request.method, &request.url);
        for (name, value) in request.headers.iter() {
            builder = builder.header(name, value);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }

        let response = builder
            .send()
            .await
            .map_err(TransportError::from_reqwest_error)?;
        let status = response.status().as_u16();
        let headers = TransportHeaders::from(response.headers());
        let body = Box::pin(response.bytes_stream().map(|chunk| {
            chunk
                .map(|bytes| bytes.to_vec())
                .map_err(TransportError::from_reqwest_error)
        }));

        Ok(TransportResponse::new(status, headers, body))
    }
}
