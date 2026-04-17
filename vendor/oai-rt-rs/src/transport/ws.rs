use crate::error::Result;
use crate::protocol::models::DEFAULT_MODEL;
use reqwest::header::HeaderValue;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use url::Url;

#[derive(Debug)]
pub struct WsStream(WebSocketStream<MaybeTlsStream<TcpStream>>);

impl WsStream {
    pub(crate) const fn new(stream: WebSocketStream<MaybeTlsStream<TcpStream>>) -> Self {
        Self(stream)
    }
}

impl futures::Stream for WsStream {
    type Item = std::result::Result<
        tokio_tungstenite::tungstenite::Message,
        tokio_tungstenite::tungstenite::Error,
    >;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.0).poll_next(cx)
    }
}

impl futures::Sink<tokio_tungstenite::tungstenite::Message> for WsStream {
    type Error = tokio_tungstenite::tungstenite::Error;

    fn poll_ready(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
        std::pin::Pin::new(&mut self.0).poll_ready(cx)
    }

    fn start_send(
        mut self: std::pin::Pin<&mut Self>,
        item: tokio_tungstenite::tungstenite::Message,
    ) -> std::result::Result<(), Self::Error> {
        std::pin::Pin::new(&mut self.0).start_send(item)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
        std::pin::Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
        std::pin::Pin::new(&mut self.0).poll_close(cx)
    }
}

const WS_BASE_URL: &str = "wss://api.openai.com/v1/realtime";

/// Establish a WebSocket connection to the Realtime API.
///
/// # Errors
/// Returns an error if the handshake fails.
pub async fn connect(
    api_key: &str,
    model: Option<&str>,
    call_id: Option<&str>,
) -> Result<WsStream> {
    let mut url = Url::parse(WS_BASE_URL)?;

    {
        let mut query = url.query_pairs_mut();
        if let Some(cid) = call_id {
            query.append_pair("call_id", cid);
        } else {
            query.append_pair("model", model.unwrap_or(DEFAULT_MODEL));
        }
    }

    let auth_header = HeaderValue::from_str(&format!("Bearer {api_key}"))?;

    let mut req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        url.as_str(),
    )?;
    let h = req.headers_mut();
    h.insert(reqwest::header::AUTHORIZATION, auth_header);
    let (ws_stream, _) = connect_async(req).await?;

    tracing::info!("Connected to OpenAI Realtime");

    Ok(WsStream::new(ws_stream))
}
