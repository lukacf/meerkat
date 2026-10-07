use super::protected::{
    ProtectedMetadataState, has_protected_metadata, protected_http_client, protected_sse_stream,
    read_bounded_body, serialize_bounded_message,
};
use futures::stream::BoxStream;
use http::header::WWW_AUTHENTICATE;
use http::{HeaderName, HeaderValue};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap};
use sse_stream::Sse;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use rmcp::model::{ClientJsonRpcMessage, JsonRpcMessage};
use rmcp::transport::common::http_header::{
    EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_MCP_PROTOCOL_VERSION, HEADER_SESSION_ID,
    JSON_MIME_TYPE,
};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, InsufficientScopeError, StreamableHttpClient, StreamableHttpError,
    StreamableHttpPostResponse,
};

/// Shared HTTP client for Streamable HTTP connections.
///
/// Reuses a single `reqwest::Client` for connection pooling and better performance.
/// A default client is created via `new()`, or pass an existing client via `with_client()`.
#[derive(Clone)]
pub(crate) struct ReqwestStreamableHttpClient {
    /// Follows only same-origin redirects. A build failure is kept, and every
    /// request fails with it; nothing falls back to a default client.
    client: Result<reqwest::Client, meerkat_auth_core::auth_oauth::CredentialHttpClientUnavailable>,
    headers: HeaderMap,
    auth_challenge: AuthChallengeRecorder,
    protected_metadata: ProtectedMetadataState,
    session_expiry: SessionExpiryRecorder,
}

/// Typed, sticky record that the server dropped this connection's session
/// (a `404` on a POST or GET that carried the session id). The transport
/// never re-initializes or re-sends after it: every later POST that would
/// attach the session is refused at its entry, and the connection refuses
/// later tool calls before queuing them. `get_stream` and `delete_session`
/// have no such entry guard. It says nothing about any one request; that is
/// [`RequestDispatch`].
#[derive(Clone, Debug)]
pub(crate) struct SessionExpiryRecorder {
    expired: Arc<tokio::sync::watch::Sender<bool>>,
    /// Test-only: the client classified the background GET as "no SSE
    /// stream" (`405`).
    #[cfg(test)]
    stream_unsupported: Arc<tokio::sync::Notify>,
}

impl Default for SessionExpiryRecorder {
    fn default() -> Self {
        Self {
            expired: Arc::new(tokio::sync::watch::Sender::new(false)),
            #[cfg(test)]
            stream_unsupported: Arc::new(tokio::sync::Notify::new()),
        }
    }
}

impl SessionExpiryRecorder {
    fn record(&self) {
        self.expired.send_replace(true);
    }

    pub(crate) fn expired(&self) -> bool {
        *self.expired.borrow()
    }

    /// Resolves once the expiry is recorded (test ordering only).
    #[cfg(test)]
    pub(crate) async fn recorded(&self) {
        let mut expired = self.expired.subscribe();
        let _ = expired.wait_for(|expired| *expired).await;
    }

    /// Resolves once the client has classified a background GET as `405`
    /// (test ordering only).
    #[cfg(test)]
    pub(crate) async fn stream_unsupported_classified(&self) {
        self.stream_unsupported.notified().await;
    }
}

/// What the transport did with ONE request, recorded on that request
/// itself (it travels in the request's extensions). A call's outcome is
/// decided from this, never from connection-wide state or error text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestDisposition {
    /// Refused at transport entry: the session was already known expired.
    /// Nothing was sent.
    RefusedExpired,
    /// Handed to the HTTP client; the server may have received it.
    Sent,
    /// Sent and answered with a redirect, followed (one routed call, more
    /// than one physical request) or stopped by the redirect policy. Whether
    /// any hop took effect is unknown, so a failed outcome is uncertain.
    SentRedirected,
    /// Sent, then answered `404` for its session: it may have taken effect.
    SentSessionExpired,
}

/// Request-local disposition witness. `RefusedExpired` is the affirmative
/// unsent case. No record means no final disposition was recorded: a
/// pre-send failure (for example the frame bound), or a transport that did
/// not finish (`Sent` is recorded only after `execute` returns, so a
/// cancelled or unfinished send also leaves none). It is not proof that
/// nothing was sent.
#[derive(Clone, Debug, Default)]
pub(crate) struct RequestDispatch {
    disposition: Arc<std::sync::OnceLock<RequestDisposition>>,
}

impl RequestDispatch {
    pub(crate) fn disposition(&self) -> Option<RequestDisposition> {
        self.disposition.get().copied()
    }

    /// Recorded once per request.
    fn record(&self, disposition: RequestDisposition) {
        let _ = self.disposition.set(disposition);
    }
}

/// The witness a `tools/call` carries, if any.
fn request_dispatch(message: &ClientJsonRpcMessage) -> Option<RequestDispatch> {
    use rmcp::model::ClientRequest;
    match message {
        ClientJsonRpcMessage::Request(request) => match &request.request {
            ClientRequest::CallToolRequest(call) => {
                call.extensions.get::<RequestDispatch>().cloned()
            }
            _ => None,
        },
        _ => None,
    }
}

impl std::fmt::Debug for ReqwestStreamableHttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReqwestStreamableHttpClient")
            .field("headers", &super::RedactedHeaders(&self.headers))
            .field("auth_challenge", &self.auth_challenge)
            .finish_non_exhaustive()
    }
}

/// Typed record of an auth-class connection failure captured at the transport
/// boundary. `status` is set on every `401`/`403` (regardless of whether a
/// `www-authenticate` challenge is present); `challenge` carries the parsed
/// challenge header when the server supplies one. The decision site reads these
/// typed fields rather than re-classifying the failure from message text.
#[derive(Clone, Debug, Default)]
pub(crate) struct AuthChallengeState {
    challenge: Option<String>,
    status: Option<reqwest::StatusCode>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AuthChallengeRecorder {
    inner: Arc<Mutex<AuthChallengeState>>,
}

impl AuthChallengeRecorder {
    pub(crate) fn record(&self, response: &reqwest::Response) {
        let status = response.status();
        if !matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ) {
            return;
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.status = Some(status);
        if let Some(header) = response.headers().get(WWW_AUTHENTICATE)
            && let Ok(header) = header.to_str()
        {
            inner.challenge = Some(header.to_string());
        }
    }

    /// Drain the captured auth-failure record (challenge text + typed status).
    pub(crate) fn take(&self) -> AuthChallengeState {
        std::mem::take(
            &mut *self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

impl AuthChallengeState {
    pub(crate) fn challenge(&self) -> Option<&str> {
        self.challenge.as_deref()
    }

    pub(crate) fn status(&self) -> Option<reqwest::StatusCode> {
        self.status
    }
}

/// Lazy-initialized shared client for Streamable HTTP transports. Every
/// request carries the configured headers (which can hold credentials) and
/// any bearer, so it follows only same-origin redirects.
static DEFAULT_HTTP_CLIENT: std::sync::LazyLock<
    Result<reqwest::Client, meerkat_auth_core::auth_oauth::CredentialHttpClientUnavailable>,
> = std::sync::LazyLock::new(meerkat_auth_core::auth_oauth::same_origin_credential_http_client);

/// A redirect answer the client's configured redirect policy did not follow
/// (for example another origin, the hop limit, a protected call, or a
/// missing or invalid `Location`), refused by its status before any header
/// or body is read.
fn refuse_redirect(status: reqwest::StatusCode) -> Result<(), StreamableHttpError<reqwest::Error>> {
    if status.is_redirection() {
        return Err(StreamableHttpError::UnexpectedServerResponse(
            format!(
                "MCP server answered with a redirect (status {}) that the configured redirect policy does not follow; refused",
                status.as_u16()
            )
            .into(),
        ));
    }
    Ok(())
}

impl ReqwestStreamableHttpClient {
    pub(crate) fn new_with_auth_challenge(
        headers: HeaderMap,
        auth_challenge: AuthChallengeRecorder,
    ) -> Self {
        Self {
            client: DEFAULT_HTTP_CLIENT.clone(),
            headers,
            auth_challenge,
            protected_metadata: Default::default(),
            session_expiry: Default::default(),
        }
    }

    /// Create an HTTP client with a custom reqwest::Client
    #[allow(dead_code)]
    pub(crate) fn with_client(client: reqwest::Client, headers: HeaderMap) -> Self {
        Self {
            client: Ok(client),
            headers,
            auth_challenge: AuthChallengeRecorder::default(),
            protected_metadata: Default::default(),
            session_expiry: Default::default(),
        }
    }

    pub(crate) fn with_protected_metadata(mut self, state: ProtectedMetadataState) -> Self {
        self.protected_metadata = state;
        self
    }

    fn client(&self) -> Result<&reqwest::Client, StreamableHttpError<reqwest::Error>> {
        self.client.as_ref().map_err(|_| {
            StreamableHttpError::UnexpectedServerResponse("MCP HTTP client unavailable".into())
        })
    }

    pub(crate) fn with_session_expiry(mut self, recorder: SessionExpiryRecorder) -> Self {
        self.session_expiry = recorder;
        self
    }

    fn apply_headers(&self, mut builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        builder
    }

    fn apply_custom_headers(
        mut builder: reqwest::RequestBuilder,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<reqwest::RequestBuilder, StreamableHttpError<reqwest::Error>> {
        for (name, value) in headers {
            validate_custom_header(&name).map_err(StreamableHttpError::ReservedHeaderConflict)?;
            builder = builder.header(name, value);
        }
        Ok(builder)
    }
}

fn validate_custom_header(name: &HeaderName) -> Result<(), String> {
    const RESERVED_HEADERS: &[&str] = &[
        "accept",
        HEADER_SESSION_ID,
        HEADER_MCP_PROTOCOL_VERSION,
        HEADER_LAST_EVENT_ID,
    ];
    if RESERVED_HEADERS
        .iter()
        .any(|reserved| name.as_str().eq_ignore_ascii_case(reserved))
    {
        if name
            .as_str()
            .eq_ignore_ascii_case(HEADER_MCP_PROTOCOL_VERSION)
        {
            return Ok(());
        }
        return Err(name.to_string());
    }
    Ok(())
}

fn extract_scope_from_header(header: &str) -> Option<String> {
    let header_lowercase = header.to_ascii_lowercase();
    let scope_key = "scope=";
    if let Some(pos) = header_lowercase.find(scope_key) {
        let start = pos + scope_key.len();
        let value_slice = &header[start..];
        if let Some(stripped) = value_slice.strip_prefix('"') {
            if let Some(end_quote) = stripped.find('"') {
                return Some(stripped[..end_quote].to_string());
            }
        } else {
            let end = value_slice
                .find(|c: char| c == ',' || c == ';' || c.is_whitespace())
                .unwrap_or(value_slice.len());
            if end > 0 {
                return Some(value_slice[..end].to_string());
            }
        }
    }
    None
}

impl StreamableHttpClient for ReqwestStreamableHttpClient {
    type Error = reqwest::Error;

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, sse_stream::Error>>, StreamableHttpError<Self::Error>>
    {
        let mut request_builder = self
            .client()?
            .get(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "))
            .header(HEADER_SESSION_ID, session_id.as_ref());
        request_builder = self.apply_headers(request_builder);
        request_builder = Self::apply_custom_headers(request_builder, custom_headers)?;
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
        refuse_redirect(response.status())?;
        self.auth_challenge.record(&response);
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            #[cfg(test)]
            self.session_expiry.stream_unsupported.notify_one();
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        // The stream request always carries the session: a 404 means the
        // server dropped it, exactly like a POST 404. Later requests are
        // refused at transport entry; no request's own disposition changes.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            self.session_expiry.record();
            return Err(StreamableHttpError::SessionExpired);
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
        let event_stream = protected_sse_stream(response, self.protected_metadata.clone(), false);
        Ok(event_stream)
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let mut request_builder = self.client()?.delete(uri.as_ref());
        request_builder = self.apply_headers(request_builder);
        request_builder = Self::apply_custom_headers(request_builder, custom_headers)?;
        if let Some(auth_header) = auth_token {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        let response = request_builder
            .header(HEADER_SESSION_ID, session.as_ref())
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;
        refuse_redirect(response.status())?;

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
        mut message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let dispatch = request_dispatch(&message);
        // A known-dead session is never entered again, not even by a request
        // that was queued before the expiry was recorded.
        if session_id.is_some() && self.session_expiry.expired() {
            if let Some(dispatch) = &dispatch {
                dispatch.record(RequestDisposition::RefusedExpired);
            }
            return Err(StreamableHttpError::SessionExpired);
        }
        let protected_call = has_protected_metadata(&message);
        let client = if protected_call {
            protected_http_client().map_err(|_| {
                StreamableHttpError::UnexpectedServerResponse(
                    "protected MCP HTTP client unavailable".into(),
                )
            })?
        } else {
            self.client()?
        };
        let mut request = client
            .post(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "));
        request = self.apply_headers(request);
        request = Self::apply_custom_headers(request, custom_headers)?;
        if let Some(auth_header) = auth_token {
            request = request.bearer_auth(auth_header);
        }
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        let mut request = request.build().map_err(StreamableHttpError::Client)?;
        let bytes = serialize_bounded_message(&mut message).map_err(|_| {
            StreamableHttpError::UnexpectedServerResponse(
                "invalid or oversized MCP JSON-RPC frame".into(),
            )
        })?;
        request
            .headers_mut()
            .entry(CONTENT_TYPE)
            .or_insert(HeaderValue::from_static(JSON_MIME_TYPE));
        *request.body_mut() = Some(bytes.into());
        let requested_url = request.url().clone();
        let result = client.execute(request).await;
        // From here the server may have received the request, possibly more
        // than once if a same-origin redirect was followed. Its own final
        // disposition is recorded here; a session 404 records its own below.
        if let Some(dispatch) = &dispatch {
            match &result {
                Ok(response)
                    if session_was_attached
                        && response.status() == reqwest::StatusCode::NOT_FOUND => {}
                Ok(response)
                    if response.url() != &requested_url || response.status().is_redirection() =>
                {
                    dispatch.record(RequestDisposition::SentRedirected);
                }
                _ => dispatch.record(RequestDisposition::Sent),
            }
        }
        let response = result.map_err(StreamableHttpError::Client)?;
        refuse_redirect(response.status())?;
        self.auth_challenge.record(&response);
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
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                header,
            )));
        }
        if response.status() == reqwest::StatusCode::FORBIDDEN
            && let Some(header) = response.headers().get(WWW_AUTHENTICATE)
        {
            let header_str = header
                .to_str()
                .map_err(|_| {
                    StreamableHttpError::UnexpectedServerResponse(Cow::Borrowed(
                        "invalid www-authenticate header value",
                    ))
                })?
                .to_string();
            let scope = extract_scope_from_header(&header_str);
            return Err(StreamableHttpError::InsufficientScope(
                InsufficientScopeError::new(header_str, scope),
            ));
        }
        let status = response.status();
        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            // The transport is built with `reinit_on_expired_session(false)`,
            // so this request is never re-initialized and re-sent: a server
            // or proxy that ran it before answering 404 must not run it
            // twice. The record types the outcome and refuses later calls;
            // only an explicit reconnect starts a new session.
            self.session_expiry.record();
            if let Some(dispatch) = &dispatch {
                dispatch.record(RequestDisposition::SentSessionExpired);
            }
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .map(|ct| String::from_utf8_lossy(ct.as_bytes()).to_string());
        let content_length = response.content_length();
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(std::string::ToString::to_string);
        if status.is_success()
            && content_length == Some(0)
            && matches!(
                message,
                ClientJsonRpcMessage::Notification(_)
                    | ClientJsonRpcMessage::Response(_)
                    | ClientJsonRpcMessage::Error(_)
            )
        {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if !status.is_success() {
            let body = read_bounded_body(response).await.map_err(|_| {
                StreamableHttpError::UnexpectedServerResponse(
                    "invalid or oversized MCP response".into(),
                )
            })?;
            if content_type
                .as_deref()
                .is_some_and(|ct| ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()))
                && let Ok(message @ JsonRpcMessage::Error(_)) = self.protected_metadata.parse(&body)
            {
                return Ok(StreamableHttpPostResponse::Json(message, session_id));
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}: MCP request refused"),
            )));
        }
        match content_type.as_deref() {
            Some(ct) if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                let event_stream =
                    protected_sse_stream(response, self.protected_metadata.clone(), false);
                Ok(StreamableHttpPostResponse::Sse(event_stream, session_id))
            }
            Some(ct) if ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()) => {
                let body = read_bounded_body(response).await.map_err(|_| {
                    StreamableHttpError::UnexpectedServerResponse(
                        "invalid or oversized MCP response".into(),
                    )
                })?;
                let message = self.protected_metadata.parse(&body).map_err(|_| {
                    StreamableHttpError::UnexpectedServerResponse(
                        "invalid MCP JSON-RPC response".into(),
                    )
                })?;
                Ok(StreamableHttpPostResponse::Json(message, session_id))
            }
            _ => Err(StreamableHttpError::UnexpectedContentType(content_type)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{ErrorData, NumberOrString};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve_once(
        status: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let headers = headers
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect::<Vec<_>>();
        let status = status.to_string();
        let body = body.to_string();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer).await;
                let mut response =
                    format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
                for (name, value) in headers {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                response.push_str("\r\n");
                response.push_str(&body);
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Ok(format!("http://{addr}/mcp"))
    }

    fn test_message() -> ClientJsonRpcMessage {
        ClientJsonRpcMessage::error(
            ErrorData::internal_error("client-side test", None),
            Some(NumberOrString::Number(1)),
        )
    }

    #[tokio::test]
    async fn post_message_maps_forbidden_authenticate_to_insufficient_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let uri = serve_once(
            "403 Forbidden",
            &[(
                "WWW-Authenticate",
                r#"Bearer error="insufficient_scope", scope="files:read files:write""#,
            )],
            "",
        )
        .await?;
        let client =
            ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), HeaderMap::default());

        let result = client
            .post_message(Arc::from(uri), test_message(), None, None, HashMap::new())
            .await;

        match result {
            Err(StreamableHttpError::InsufficientScope(scope)) => {
                assert_eq!(
                    scope.required_scope.as_deref(),
                    Some("files:read files:write")
                );
            }
            other => return Err(format!("expected insufficient scope, got {other:?}").into()),
        }
        Ok(())
    }

    #[tokio::test]
    async fn post_message_maps_attached_not_found_to_session_expired()
    -> Result<(), Box<dyn std::error::Error>> {
        let uri = serve_once("404 Not Found", &[], "gone").await?;
        let client =
            ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), HeaderMap::default());

        let result = client
            .post_message(
                Arc::from(uri),
                test_message(),
                Some(Arc::from("dead-session")),
                None,
                HashMap::new(),
            )
            .await;

        assert!(matches!(result, Err(StreamableHttpError::SessionExpired)));
        Ok(())
    }

    #[tokio::test]
    async fn post_message_preserves_non_success_json_rpc_error_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"tool failed"}}"#;
        let uri = serve_once(
            "500 Internal Server Error",
            &[("Content-Type", JSON_MIME_TYPE)],
            body,
        )
        .await?;
        let client =
            ReqwestStreamableHttpClient::with_client(reqwest::Client::new(), HeaderMap::default());

        let response = client
            .post_message(Arc::from(uri), test_message(), None, None, HashMap::new())
            .await?;

        match response {
            StreamableHttpPostResponse::Json(JsonRpcMessage::Error(error), _) => {
                assert_eq!(error.error.message, "tool failed");
            }
            other => return Err(format!("expected JSON-RPC error response, got {other:?}").into()),
        }
        Ok(())
    }
}
