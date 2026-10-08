//! Keep host metadata out of rmcp's Debug-based request and worker logs.
//! Values travel in opaque Extensions until an owned final wire serializer.

use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, RwLock};

use futures::{SinkExt, StreamExt, stream::BoxStream};
use rmcp::model::{ClientJsonRpcMessage, ClientRequest, ServerJsonRpcMessage};
use rmcp::service::RoleClient;
use rmcp::transport::Transport;
use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};

/// A frame can contain images or large tool results. This is a wire bound,
/// independent of the much smaller trusted metadata preparation bound.
pub(crate) const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 64 * 1024;
const MAX_METADATA_KEYS: usize = 128;

/// rmcp's Extensions Debug and Serialize do not expose typed entry values.
#[derive(Clone)]
pub(crate) struct ProtectedMetadata(pub(crate) Map<String, Value>);

/// Retain key names for the connection lifetime so late server responses are
/// also scrubbed. This is bounded and contains no values or caller identity.
#[derive(Clone)]
pub(crate) struct ProtectedMetadataState(Arc<RwLock<BTreeSet<String>>>);

impl Default for ProtectedMetadataState {
    fn default() -> Self {
        Self(Arc::new(RwLock::new(BTreeSet::new())))
    }
}

impl ProtectedMetadataState {
    pub(crate) fn register(
        &self,
        metadata: &Map<String, Value>,
    ) -> Result<(), crate::McpCallContextError> {
        if serde_json::to_vec(metadata)
            .map_err(|_| crate::McpCallContextError::Unavailable)?
            .len()
            > MAX_METADATA_BYTES
        {
            return Err(crate::McpCallContextError::Denied);
        }
        let mut keys = self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let new_keys = metadata.keys().filter(|key| !keys.contains(*key)).count();
        if keys.len().saturating_add(new_keys) > MAX_METADATA_KEYS {
            return Err(crate::McpCallContextError::Denied);
        }
        keys.extend(metadata.keys().cloned());
        Ok(())
    }

    pub(crate) fn has_protected_calls(&self) -> bool {
        !self
            .0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    fn strip(&self, value: &mut Value) {
        let keys = self
            .0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // These are protocol metadata slots. Tool arguments, structuredContent,
        // resources, and JSON-RPC error data are application-owned values even
        // when they contain an object named _meta.
        for pointer in ["/params/_meta", "/result/_meta"] {
            if let Some(Value::Object(metadata)) = value.pointer_mut(pointer) {
                metadata.retain(|key, _| !keys.contains(key));
            }
        }
    }

    pub(crate) fn parse(&self, bytes: &[u8]) -> io::Result<ServerJsonRpcMessage> {
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(invalid_frame());
        }
        let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
        let mut value: Value = serde_json::from_slice(bytes).map_err(|_| invalid_frame())?;
        self.strip(&mut value);
        let mut message = serde_json::from_value(value).map_err(|_| invalid_frame())?;
        self.strip_typed(&mut message);
        Ok(message)
    }

    /// Traverse only rmcp-owned typed metadata. Arbitrary arguments, schemas,
    /// structured results, resource text, and error data are never traversed.
    fn strip_typed(&self, message: &mut ServerJsonRpcMessage) {
        use rmcp::model::{
            PromptMessageContent, SamplingContent, SamplingMessageContent, ServerRequest,
            ServerResult,
        };
        let keys = self
            .0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match message {
            ServerJsonRpcMessage::Response(response) => match &mut response.result {
                ServerResult::CallToolResult(result) => {
                    for content in &mut result.content {
                        strip_content(&keys, &mut content.raw);
                    }
                }
                ServerResult::ListToolsResult(result) => {
                    for tool in &mut result.tools {
                        strip_meta(&keys, &mut tool.meta);
                    }
                }
                ServerResult::ListResourcesResult(result) => {
                    for resource in &mut result.resources {
                        strip_meta(&keys, &mut resource.raw.meta);
                    }
                }
                ServerResult::ReadResourceResult(result) => {
                    for resource in &mut result.contents {
                        strip_resource(&keys, resource);
                    }
                }
                ServerResult::ListPromptsResult(result) => {
                    for prompt in &mut result.prompts {
                        strip_meta(&keys, &mut prompt.meta);
                    }
                }
                ServerResult::GetPromptResult(result) => {
                    for message in &mut result.messages {
                        match &mut message.content {
                            PromptMessageContent::Image { image } => {
                                strip_meta(&keys, &mut image.raw.meta);
                            }
                            PromptMessageContent::Resource { resource } => {
                                strip_meta(&keys, &mut resource.raw.meta);
                                strip_resource(&keys, &mut resource.raw.resource);
                            }
                            PromptMessageContent::ResourceLink { link } => {
                                strip_meta(&keys, &mut link.raw.meta);
                            }
                            PromptMessageContent::Text { .. } => {}
                        }
                    }
                }
                _ => {}
            },
            ServerJsonRpcMessage::Request(request) => {
                if let ServerRequest::CreateMessageRequest(request) = &mut request.request {
                    if let Some(tools) = &mut request.params.tools {
                        for tool in tools {
                            strip_meta(&keys, &mut tool.meta);
                        }
                    }
                    for message in &mut request.params.messages {
                        strip_meta(&keys, &mut message.meta);
                        let contents = match &mut message.content {
                            SamplingContent::Single(content) => std::slice::from_mut(content),
                            SamplingContent::Multiple(contents) => contents.as_mut_slice(),
                        };
                        for content in contents {
                            match content {
                                SamplingMessageContent::Text(content) => {
                                    strip_meta(&keys, &mut content.meta);
                                }
                                SamplingMessageContent::Image(content) => {
                                    strip_meta(&keys, &mut content.meta);
                                }
                                SamplingMessageContent::ToolUse(content) => {
                                    strip_meta(&keys, &mut content.meta);
                                }
                                SamplingMessageContent::ToolResult(content) => {
                                    strip_meta(&keys, &mut content.meta);
                                    for nested in &mut content.content {
                                        strip_content(&keys, &mut nested.raw);
                                    }
                                }
                                SamplingMessageContent::Audio(_) => {}
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub(crate) fn sanitize_sse(
        &self,
        mut event: sse_stream::Sse,
    ) -> Result<sse_stream::Sse, sse_stream::Error> {
        if let Some(data) = event.data.take() {
            let message = self
                .parse(data.as_bytes())
                .map_err(|_| sse_stream::Error::InvalidLine)?;
            event.data =
                Some(serde_json::to_string(&message).map_err(|_| sse_stream::Error::InvalidLine)?);
        }
        Ok(event)
    }
}

fn strip_meta(keys: &BTreeSet<String>, meta: &mut Option<rmcp::model::Meta>) {
    if let Some(meta) = meta {
        meta.0.retain(|key, _| !keys.contains(key));
    }
}

fn strip_resource(keys: &BTreeSet<String>, resource: &mut rmcp::model::ResourceContents) {
    use rmcp::model::ResourceContents;
    match resource {
        ResourceContents::TextResourceContents { meta, .. }
        | ResourceContents::BlobResourceContents { meta, .. } => strip_meta(keys, meta),
    }
}

fn strip_content(keys: &BTreeSet<String>, content: &mut rmcp::model::RawContent) {
    use rmcp::model::RawContent;
    match content {
        RawContent::Text(content) => strip_meta(keys, &mut content.meta),
        RawContent::Image(content) => strip_meta(keys, &mut content.meta),
        RawContent::Resource(content) => {
            strip_meta(keys, &mut content.meta);
            strip_resource(keys, &mut content.resource);
        }
        RawContent::ResourceLink(content) => strip_meta(keys, &mut content.meta),
        RawContent::Audio(_) => {}
    }
}

fn invalid_frame() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid or oversized MCP JSON-RPC frame",
    )
}

/// Identify the opaque extension before the final serializer restores it.
pub(crate) fn has_protected_metadata(message: &ClientJsonRpcMessage) -> bool {
    matches!(message, ClientJsonRpcMessage::Request(request)
        if matches!(&request.request, ClientRequest::CallToolRequest(call)
            if call.extensions.get::<ProtectedMetadata>().is_some()))
}

/// A protected body may only reach the exact configured destination: no
/// redirect at all, and no environment proxy. Ordinary calls follow only
/// same-origin redirects and keep the environment-proxy behavior.
pub(crate) fn protected_http_client() -> io::Result<&'static reqwest::Client> {
    static CLIENT: std::sync::LazyLock<Result<reqwest::Client, reqwest::Error>> =
        std::sync::LazyLock::new(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .retry(reqwest::retry::never())
                .build()
        });
    CLIENT
        .as_ref()
        .map_err(|_| io::Error::other("protected MCP HTTP client unavailable"))
}

/// Call only at the final serializer, after all rmcp queues and diagnostics.
pub(crate) fn restore_metadata(message: &mut ClientJsonRpcMessage) {
    if let ClientJsonRpcMessage::Request(message) = message
        && let ClientRequest::CallToolRequest(request) = &mut message.request
        && let Some(metadata) = request.extensions.get::<ProtectedMetadata>()
    {
        request
            .params
            .meta
            .get_or_insert_default()
            .0
            .extend(metadata.0.clone());
    }
}

/// Serialize once at the final transport boundary and enforce the encoded
/// frame bound before any bytes are written or sent.
pub(crate) fn serialize_bounded_message(message: &mut ClientJsonRpcMessage) -> io::Result<Vec<u8>> {
    restore_metadata(message);
    let bytes = serde_json::to_vec(message).map_err(|_| invalid_frame())?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(invalid_frame());
    }
    Ok(bytes)
}

/// Match rmcp 1.8's optional-notification compatibility without its raw-frame
/// diagnostics. rmcp remains the owner of typed message parsing.
fn ignored_notification(value: &Value) -> bool {
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return false;
    };
    let standard_notification = matches!(
        method,
        "notifications/cancelled"
            | "notifications/initialized"
            | "notifications/message"
            | "notifications/progress"
            | "notifications/prompts/list_changed"
            | "notifications/resources/list_changed"
            | "notifications/resources/updated"
            | "notifications/roots/list_changed"
            | "notifications/tools/list_changed"
    );
    let standard_method = standard_notification
        || matches!(
            method,
            "initialize"
                | "ping"
                | "prompts/get"
                | "prompts/list"
                | "resources/list"
                | "resources/read"
                | "resources/subscribe"
                | "resources/unsubscribe"
                | "resources/templates/list"
                | "tools/call"
                | "tools/list"
                | "completion/complete"
                | "logging/setLevel"
                | "roots/list"
                | "sampling/createMessage"
        );
    (value.get("id").is_none() && !standard_method)
        || (method.starts_with("notifications/") && !standard_notification)
}

enum InputFrame {
    Message(Box<ServerJsonRpcMessage>),
    ParseError,
}

struct ProtectedInputCodec {
    next_index: usize,
    state: ProtectedMetadataState,
}

impl ProtectedInputCodec {
    fn parse(&self, bytes: &[u8]) -> Option<InputFrame> {
        let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
        let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
        if bytes.is_empty() {
            return None;
        }
        let Ok(mut value) = serde_json::from_slice::<Value>(bytes) else {
            return Some(InputFrame::ParseError);
        };
        self.state.strip(&mut value);
        let ignored = ignored_notification(&value);
        match serde_json::from_value(value) {
            Ok(mut message) => {
                self.state.strip_typed(&mut message);
                Some(InputFrame::Message(Box::new(message)))
            }
            Err(_) if ignored => None,
            Err(_) => Some(InputFrame::ParseError),
        }
    }
}

impl Decoder for ProtectedInputCodec {
    type Item = InputFrame;
    type Error = io::Error;

    fn decode(&mut self, buffer: &mut BytesMut) -> io::Result<Option<Self::Item>> {
        loop {
            let start = self.next_index;
            let Some(end) = buffer[start..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|offset| start + offset)
            else {
                self.next_index = buffer.len();
                return if buffer.len() > MAX_FRAME_BYTES {
                    Err(invalid_frame())
                } else {
                    Ok(None)
                };
            };
            if end > MAX_FRAME_BYTES {
                return Err(invalid_frame());
            }
            let line = buffer.split_to(end + 1);
            self.next_index = 0;
            if let Some(frame) = self.parse(&line[..end]) {
                return Ok(Some(frame));
            }
        }
    }

    fn decode_eof(&mut self, buffer: &mut BytesMut) -> io::Result<Option<Self::Item>> {
        if let Some(frame) = self.decode(buffer)? {
            return Ok(Some(frame));
        }
        let remaining = buffer.split();
        self.next_index = 0;
        Ok(self.parse(&remaining))
    }
}

struct ProtectedOutputCodec;

impl Encoder<ClientJsonRpcMessage> for ProtectedOutputCodec {
    type Error = io::Error;

    fn encode(&mut self, mut item: ClientJsonRpcMessage, buffer: &mut BytesMut) -> io::Result<()> {
        let bytes = serialize_bounded_message(&mut item)?;
        buffer.extend_from_slice(&bytes);
        buffer.extend_from_slice(b"\n");
        Ok(())
    }
}

pub(crate) struct ProtectedStdioTransport<R: AsyncRead, W: AsyncWrite> {
    read: FramedRead<R, ProtectedInputCodec>,
    write: Arc<Mutex<Option<FramedWrite<W, ProtectedOutputCodec>>>>,
}

impl<R: AsyncRead, W: AsyncWrite> ProtectedStdioTransport<R, W> {
    pub(crate) fn new(read: R, write: W, state: ProtectedMetadataState) -> Self {
        Self {
            read: FramedRead::new(
                read,
                ProtectedInputCodec {
                    next_index: 0,
                    state,
                },
            ),
            write: Arc::new(Mutex::new(Some(FramedWrite::new(
                write,
                ProtectedOutputCodec,
            )))),
        }
    }
}

impl<R, W> Transport<RoleClient> for ProtectedStdioTransport<R, W>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    type Error = io::Error;

    fn send(
        &mut self,
        item: ClientJsonRpcMessage,
    ) -> impl Future<Output = io::Result<()>> + Send + 'static {
        let write = Arc::clone(&self.write);
        async move {
            match write.lock().await.as_mut() {
                Some(writer) => writer.send(item).await,
                None => Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "MCP transport closed",
                )),
            }
        }
    }

    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        loop {
            match self.read.next().await {
                Some(Ok(InputFrame::Message(message))) => return Some(*message),
                Some(Ok(InputFrame::ParseError)) => {
                    tracing::debug!("MCP input refused: invalid JSON-RPC frame");
                    let error = ClientJsonRpcMessage::error(
                        rmcp::ErrorData::parse_error("Parse error", None),
                        None,
                    );
                    if self.write.lock().await.as_mut()?.send(error).await.is_err() {
                        return None;
                    }
                }
                Some(Err(_)) => {
                    tracing::warn!("MCP input refused: unreadable or oversized JSON-RPC frame");
                    return None;
                }
                None => return None,
            }
        }
    }

    async fn close(&mut self) -> io::Result<()> {
        drop(self.write.lock().await.take());
        Ok(())
    }
}

/// Bound each SSE frame before the SSE parser buffers it. Once protected calls
/// register metadata, sanitize JSON before rmcp can log or parse those events.
pub(crate) fn protected_sse_stream(
    response: reqwest::Response,
    state: ProtectedMetadataState,
    preserve_endpoint: bool,
) -> BoxStream<'static, Result<sse_stream::Sse, sse_stream::Error>> {
    let bytes =
        response
            .bytes_stream()
            .scan((0usize, 0u8, false), |(count, previous, stopped), chunk| {
                let result = if *stopped {
                    None
                } else {
                    Some(match chunk {
                        Err(_) => {
                            *stopped = true;
                            Err(invalid_frame())
                        }
                        Ok(chunk) => {
                            let mut invalid = false;
                            for byte in &chunk {
                                *count = count.saturating_add(1);
                                if *count > MAX_FRAME_BYTES {
                                    invalid = true;
                                    break;
                                }
                                if *byte == b'\n' && *previous == b'\n' {
                                    *count = 0;
                                }
                                if *byte != b'\r' {
                                    *previous = *byte;
                                }
                            }
                            if invalid {
                                *stopped = true;
                                Err(invalid_frame())
                            } else {
                                Ok(chunk)
                            }
                        }
                    })
                };
                futures::future::ready(result)
            });
    sse_stream::SseStream::from_byte_stream(bytes)
        .filter_map(move |event| {
            let result = match event {
                Err(error) => Some(Err(error)),
                // Check each event: a stream can start before the first
                // protected call. Ordinary data stays byte-identical and rmcp
                // owns its parsing; the byte-level frame bound above always runs.
                Ok(event) if !state.has_protected_calls() => Some(Ok(event)),
                Ok(event) if preserve_endpoint && event.event.as_deref() == Some("endpoint") => {
                    Some(Ok(event))
                }
                // rmcp ignores heartbeat/extension event classes and empty
                // data. Do not turn those legal stream frames into failures.
                Ok(event)
                    if event
                        .event
                        .as_deref()
                        .is_some_and(|kind| !kind.is_empty() && kind != "message")
                        || event
                            .data
                            .as_deref()
                            .is_none_or(|data| data.trim().is_empty()) =>
                {
                    None
                }
                Ok(event) => Some(state.sanitize_sse(event)),
            };
            futures::future::ready(result)
        })
        .boxed()
}

pub(crate) async fn read_bounded_body(response: reqwest::Response) -> io::Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| invalid_frame())?;
        if body.len().saturating_add(chunk.len()) > MAX_FRAME_BYTES {
            return Err(invalid_frame());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
#[path = "protected_tests.rs"]
mod tests;
