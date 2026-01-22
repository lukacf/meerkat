use futures::StreamExt;
use futures::{Future, stream::BoxStream};
use http::Uri;
use reqwest::header::{ACCEPT, HeaderMap};
use sse_stream::{Error as SseError, Sse, SseStream};
use std::sync::Arc;

use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::service::RoleClient;
use rmcp::transport::Transport;
use rmcp::transport::common::http_header::{EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID};

#[derive(thiserror::Error, Debug)]
pub enum SseTransportError<E: std::error::Error + Send + Sync + 'static> {
    #[error("SSE error: {0}")]
    Sse(#[from] SseError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Client error: {0}")]
    Client(E),
    #[error("unexpected end of stream")]
    UnexpectedEndOfStream,
    #[error("Unexpected content type: {0:?}")]
    UnexpectedContentType(Option<String>),
    #[error("Invalid uri: {0}")]
    InvalidUri(#[from] http::uri::InvalidUri),
    #[error("Invalid uri parts: {0}")]
    InvalidUriParts(#[from] http::uri::InvalidUriParts),
}

pub trait SseClient: Clone + Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn post_message(
        &self,
        uri: Uri,
        message: ClientJsonRpcMessage,
        auth_token: Option<String>,
    ) -> impl Future<Output = Result<(), SseTransportError<Self::Error>>> + Send + '_;

    fn get_stream(
        &self,
        uri: Uri,
        last_event_id: Option<String>,
        auth_token: Option<String>,
    ) -> impl Future<
        Output = Result<BoxStream<'static, Result<Sse, SseError>>, SseTransportError<Self::Error>>,
    > + Send
    + '_;
}

#[derive(Debug, Clone)]
pub struct SseClientConfig {
    pub sse_endpoint: Arc<str>,
    pub use_message_endpoint: Option<String>,
}

impl Default for SseClientConfig {
    fn default() -> Self {
        Self {
            sse_endpoint: "".into(),
            use_message_endpoint: None,
        }
    }
}

pub struct SseClientTransport<C: SseClient> {
    client: C,
    message_endpoint: Uri,
    stream: Option<BoxStream<'static, Result<Sse, SseError>>>,
}

impl<C: SseClient> Transport<RoleClient> for SseClientTransport<C> {
    type Error = SseTransportError<C::Error>;

    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        let stream = self.stream.as_mut()?;
        loop {
            let next = stream.next().await;
            match next {
                Some(Ok(event)) => {
                    if let Some(data) = event.data {
                        match serde_json::from_str::<ServerJsonRpcMessage>(&data) {
                            Ok(message) => return Some(message),
                            Err(_) => continue,
                        }
                    }
                }
                Some(Err(_)) => return None,
                None => return None,
            }
        }
    }

    fn send(
        &mut self,
        item: rmcp::service::TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let client = self.client.clone();
        let uri = self.message_endpoint.clone();
        async move { client.post_message(uri, item, None).await }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.stream.take();
        Ok(())
    }
}

impl<C: SseClient> SseClientTransport<C> {
    pub async fn start_with_client(
        client: C,
        config: SseClientConfig,
    ) -> Result<Self, SseTransportError<C::Error>> {
        let sse_endpoint = config.sse_endpoint.as_ref().parse::<http::Uri>()?;

        let mut sse_stream = client.get_stream(sse_endpoint.clone(), None, None).await?;
        let message_endpoint = if let Some(endpoint) = config.use_message_endpoint.clone() {
            let ep = endpoint.parse::<http::Uri>()?;
            let mut sse_endpoint_parts = sse_endpoint.clone().into_parts();
            sse_endpoint_parts.path_and_query = ep.into_parts().path_and_query;
            Uri::from_parts(sse_endpoint_parts)?
        } else {
            loop {
                let sse = sse_stream
                    .next()
                    .await
                    .ok_or(SseTransportError::UnexpectedEndOfStream)??;
                let Some("endpoint") = sse.event.as_deref() else {
                    continue;
                };
                let ep = sse.data.unwrap_or_default();
                break message_endpoint(sse_endpoint.clone(), ep)?;
            }
        };

        Ok(Self {
            client,
            message_endpoint,
            stream: Some(sse_stream),
        })
    }
}

fn message_endpoint(base: http::Uri, endpoint: String) -> Result<http::Uri, http::uri::InvalidUri> {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return endpoint.parse::<http::Uri>();
    }

    let mut base_parts = base.into_parts();
    let endpoint_clone = endpoint.clone();

    if endpoint.starts_with("?") {
        if let Some(base_path_and_query) = &base_parts.path_and_query {
            let base_path = base_path_and_query.path();
            base_parts.path_and_query = Some(format!("{}{}", base_path, endpoint).parse()?);
        } else {
            base_parts.path_and_query = Some(format!("/{}", endpoint).parse()?);
        }
    } else {
        let path_to_use = if endpoint.starts_with("/") {
            endpoint
        } else {
            format!("/{}", endpoint)
        };
        base_parts.path_and_query = Some(path_to_use.parse()?);
    }

    http::Uri::from_parts(base_parts).map_err(|_| endpoint_clone.parse::<http::Uri>().unwrap_err())
}

#[derive(Clone, Debug)]
pub(crate) struct ReqwestSseClient {
    client: reqwest::Client,
    headers: HeaderMap,
}

impl ReqwestSseClient {
    pub(crate) fn new(headers: HeaderMap) -> Self {
        Self {
            client: reqwest::Client::default(),
            headers,
        }
    }

    fn apply_headers(&self, mut builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (name, value) in self.headers.iter() {
            builder = builder.header(name, value);
        }
        builder
    }
}

impl From<reqwest::Error> for SseTransportError<reqwest::Error> {
    fn from(e: reqwest::Error) -> Self {
        SseTransportError::Client(e)
    }
}

impl SseClient for ReqwestSseClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Uri,
        message: ClientJsonRpcMessage,
        auth_token: Option<String>,
    ) -> Result<(), SseTransportError<Self::Error>> {
        let mut request_builder = self.client.post(uri.to_string()).json(&message);
        request_builder = self.apply_headers(request_builder);
        if let Some(auth_header) = auth_token {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        request_builder
            .send()
            .await
            .and_then(|resp| resp.error_for_status())
            .map_err(SseTransportError::from)
            .map(drop)
    }

    async fn get_stream(
        &self,
        uri: Uri,
        last_event_id: Option<String>,
        auth_token: Option<String>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, SseTransportError<Self::Error>> {
        let mut request_builder = self
            .client
            .get(uri.to_string())
            .header(ACCEPT, EVENT_STREAM_MIME_TYPE);
        request_builder = self.apply_headers(request_builder);
        if let Some(auth_header) = auth_token {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        if let Some(last_event_id) = last_event_id {
            request_builder = request_builder.header(HEADER_LAST_EVENT_ID, last_event_id);
        }
        let response = request_builder.send().await?;
        let response = response.error_for_status()?;
        match response.headers().get(reqwest::header::CONTENT_TYPE) {
            Some(ct) => {
                if !ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) {
                    return Err(SseTransportError::UnexpectedContentType(Some(
                        String::from_utf8_lossy(ct.as_bytes()).to_string(),
                    )));
                }
            }
            None => {
                return Err(SseTransportError::UnexpectedContentType(None));
            }
        }
        let event_stream = SseStream::from_byte_stream(response.bytes_stream()).boxed();
        Ok(event_stream)
    }
}
