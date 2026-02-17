use futures::StreamExt;
use futures::stream::BoxStream;
use http::header::WWW_AUTHENTICATE;
use reqwest::header::{ACCEPT, HeaderMap};
use sse_stream::{Sse, SseStream};
use std::sync::Arc;

use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::common::http_header::{
    EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_SESSION_ID, JSON_MIME_TYPE,
};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};

/// Shared HTTP client for Streamable HTTP connections.
///
/// Reuses a single `reqwest::Client` for connection pooling and better performance.
/// A default client is created via `new()`, or pass an existing client via `with_client()`.
#[derive(Clone, Debug)]
pub(crate) struct ReqwestStreamableHttpClient {
    client: reqwest::Client,
    headers: HeaderMap,
}

/// Lazy-initialized shared reqwest client for Streamable HTTP transports
static DEFAULT_HTTP_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(reqwest::Client::new);

impl ReqwestStreamableHttpClient {
    /// Create a new HTTP client using the shared default reqwest::Client
    pub(crate) fn new(headers: HeaderMap) -> Self {
        Self {
            client: DEFAULT_HTTP_CLIENT.clone(),
            headers,
        }
    }

    /// Create an HTTP client with a custom reqwest::Client
    #[allow(dead_code)]
    pub(crate) fn with_client(client: reqwest::Client, headers: HeaderMap) -> Self {
        Self { client, headers }
    }

    fn apply_headers(&self, mut builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (name, value) in self.headers.iter() {
            builder = builder.header(name, value);
        }
        builder
    }
}

impl StreamableHttpClient for ReqwestStreamableHttpClient {
    type Error = reqwest::Error;

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
    ) -> Result<BoxStream<'static, Result<Sse, sse_stream::Error>>, StreamableHttpError<Self::Error>>
    {
        let mut request_builder = self
            .client
            .get(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "))
            .header(HEADER_SESSION_ID, session_id.as_ref());
        request_builder = self.apply_headers(request_builder);
        if let Some(last_event_id) = last_event_id {
            request_builder = request_builder.header(HEADER_LAST_EVENT_ID, last_event_id);
        }
        if let Some(auth_header) = auth_token {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        let response = request_builder
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        let response = response
            .error_for_status()
            .map_err(StreamableHttpError::Client)?;
        match response.headers().get(reqwest::header::CONTENT_TYPE) {
            Some(ct) => {
                if !ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes())
                    && !ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes())
                {
                    return Err(StreamableHttpError::UnexpectedContentType(Some(
                        String::from_utf8_lossy(ct.as_bytes()).to_string(),
                    )));
                }
            }
            None => {
                return Err(StreamableHttpError::UnexpectedContentType(None));
            }
        }
        let event_stream = SseStream::from_byte_stream(response.bytes_stream()).boxed();
        Ok(event_stream)
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        auth_token: Option<String>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let mut request_builder = self.client.delete(uri.as_ref());
        request_builder = self.apply_headers(request_builder);
        if let Some(auth_header) = auth_token {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        let response = request_builder
            .header(HEADER_SESSION_ID, session.as_ref())
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;

        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        let _response = response
            .error_for_status()
            .map_err(StreamableHttpError::Client)?;
        Ok(())
    }

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let mut request = self
            .client
            .post(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "));
        request = self.apply_headers(request);
        if let Some(auth_header) = auth_token {
            request = request.bearer_auth(auth_header);
        }
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        let response = request
            .json(&message)
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && let Some(header) = response.headers().get(WWW_AUTHENTICATE)
        {
            let header = header
                .to_str()
                .map_err(|_| {
                    StreamableHttpError::UnexpectedServerResponse(
                        "invalid www-authenticate header value".into(),
                    )
                })?
                .to_string();
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError {
                www_authenticate_header: header,
            }));
        }
        let status = response.status();
        let response = response
            .error_for_status()
            .map_err(StreamableHttpError::Client)?;
        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        let content_type = response.headers().get(reqwest::header::CONTENT_TYPE);
        let session_id = response.headers().get(HEADER_SESSION_ID);
        let session_id = session_id
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        match content_type {
            Some(ct) if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                let event_stream = SseStream::from_byte_stream(response.bytes_stream()).boxed();
                Ok(StreamableHttpPostResponse::Sse(event_stream, session_id))
            }
            Some(ct) if ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()) => {
                let message: ServerJsonRpcMessage =
                    response.json().await.map_err(StreamableHttpError::Client)?;
                Ok(StreamableHttpPostResponse::Json(message, session_id))
            }
            _ => Err(StreamableHttpError::UnexpectedContentType(
                content_type.map(|ct| String::from_utf8_lossy(ct.as_bytes()).to_string()),
            )),
        }
    }
}
