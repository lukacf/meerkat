//! Shared real-rmcp server for connection-local form-elicitation tests.

mod fixture;
pub use fixture::{FIXTURE_ENV, FixtureError, fixture_binary, resolve_fixture_binary};

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientInfo, Content, CreateElicitationRequest,
    CreateElicitationRequestParams, CreateMessageRequest, CreateMessageRequestParams,
    ListToolsResult, PaginatedRequestParams, SamplingMessage, ServerCapabilities, ServerInfo,
    ServerRequest, Tool,
};
use rmcp::service::{Peer, PeerRequestOptions, RequestContext, ServiceError};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The same handler serves stdio and both HTTP test transports. HTTP fixtures
/// frame messages only; this is the sole MCP protocol dispatcher.
#[derive(Clone, Default)]
pub struct FormTestServer {
    initialize_meta: Arc<Mutex<Option<Value>>>,
}

pub fn form_request(message: &str) -> Result<CreateElicitationRequestParams, serde_json::Error> {
    serde_json::from_value(json!({
        "mode": "form", "message": message,
        "requestedSchema": {"type": "object", "properties": {"confirm": {"type": "boolean"}}, "required": ["confirm"]}
    }))
}

async fn callback(peer: &Peer<RoleServer>, request: ServerRequest) -> Value {
    let response = match peer
        .send_request_with_option(
            request,
            PeerRequestOptions::with_timeout(Duration::from_secs(5)),
        )
        .await
    {
        Ok(handle) => handle.await_response().await,
        Err(error) => Err(error),
    };
    match response {
        Ok(value) => json!({"kind": "result", "value": value}),
        Err(ServiceError::McpError(error)) => json!({"kind": "protocol_error", "error": error}),
        Err(error) => json!({"kind": "transport_or_lifetime_error", "detail": error.to_string()}),
    }
}

impl ServerHandler for FormTestServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn initialize(
        &self,
        request: ClientInfo,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerInfo, ErrorData> {
        *self
            .initialize_meta
            .lock()
            .map_err(|_| ErrorData::internal_error("fixture trace poisoned", None))? = Some(
            serde_json::to_value(&context.meta)
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?,
        );
        context.peer.set_peer_info(request);
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut result = ListToolsResult::default();
        result.tools.push(Tool::new(
            "mcp_form",
            "Real form callback fixture",
            json!({"type": "object", "properties": {"message": {"type": "string"}}})
                .as_object()
                .cloned()
                .unwrap_or_default(),
        ));
        Ok(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = match request.name.as_ref() {
            "mcp_form" => {
                let message = request
                    .arguments
                    .as_ref()
                    .and_then(|a| a.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("accept");
                let form = form_request(message)
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                let response = callback(
                    &context.peer,
                    ServerRequest::CreateElicitationRequest(CreateElicitationRequest::new(form)),
                )
                .await;
                json!({"client_info": context.peer.peer_info(), "form": response,
                    "initialize_meta": self.initialize_meta.lock().map_err(|_| ErrorData::internal_error("fixture trace poisoned", None))?.clone()})
            }
            "mcp_defaults" => {
                // Deliberately probe unadvertised callbacks to verify safe
                // defaults. Sampling is pinned compatibility evidence only.
                let url: CreateElicitationRequestParams = serde_json::from_value(json!({
                    "mode": "url", "message": "fixture URL", "url": "https://invalid.example/fixture", "elicitationId": "fixture"
                })).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                let url = callback(
                    &context.peer,
                    ServerRequest::CreateElicitationRequest(CreateElicitationRequest::new(url)),
                )
                .await;
                let sampling = callback(
                    &context.peer,
                    ServerRequest::CreateMessageRequest(CreateMessageRequest::new(
                        CreateMessageRequestParams::new(
                            vec![SamplingMessage::user_text("fixture only, no model")],
                            8,
                        ),
                    )),
                )
                .await;
                json!({"url": url, "sampling": sampling})
            }
            _ => return Err(ErrorData::invalid_params("unknown form fixture tool", None)),
        };
        Ok(CallToolResult::success(vec![Content::text(
            value.to_string(),
        )]))
    }
}

pub async fn serve_forms_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let service = FormTestServer::default()
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
