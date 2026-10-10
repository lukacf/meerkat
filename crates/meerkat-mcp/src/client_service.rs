//! Connection-local host service selection using rmcp's existing protocol.

use std::ops::Deref;

use meerkat_core::McpServerConfig;
use rmcp::model::{
    ClientCapabilities, ClientInfo, ClientResult, CreateElicitationRequest,
    CreateElicitationRequestParams, CreateElicitationResult, ElicitationCapability, ServerRequest,
};
use rmcp::service::{
    ClientInitializeError, DynService, Peer, QuitReason, RequestContext, RoleClient,
    RunningService, ServiceExt,
};
use rmcp::transport::IntoTransport;
use rmcp::{ClientHandler, ErrorData};

use crate::McpError;

/// Creates a fresh host service for one physical MCP connection attempt.
///
/// The complete selected config identifies the exact destination; it may also
/// contain static credentials and must not be logged or sent to a model. The
/// auth resolver continues to own token resolution and login. Retries invoke
/// this factory again rather than sharing a previous connection's interaction.
///
/// Only declared form elicitation is forwarded in this first profile. Other
/// callbacks retain rmcp's limited unit-handler behavior. Requests carry actual
/// connection context, not the original user's identity. Host code must apply
/// its connection-level policy and own any UI work it starts. Elicitation does
/// not grant tools, secrets, installation permission or autonomous work.
///
/// The forwarded context retains rmcp's exact request cancellation token and
/// peer. Cancellation retires the pending host future; a closed peer rejects a
/// late result. rmcp does not expose a remote-EOF notification or a join of work
/// spawned independently by a handler, so hosts must own that work themselves.
pub trait McpClientServiceFactory: Send + Sync {
    fn create(&self, config: &McpServerConfig)
    -> Result<Box<dyn DynService<RoleClient>>, McpError>;
}

pub(crate) enum ClientServiceSelection {
    Default,
    Host(FormClient),
}

impl ClientServiceSelection {
    pub(crate) fn select(
        config: &McpServerConfig,
        factory: Option<&dyn McpClientServiceFactory>,
    ) -> Result<Self, McpError> {
        match factory {
            Some(factory) => Ok(Self::Host(FormClient::new(factory.create(config)?))),
            None => Ok(Self::Default),
        }
    }

    pub(crate) async fn serve<T, E, A>(
        self,
        transport: T,
    ) -> Result<ConnectedClient, ClientInitializeError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        match self {
            Self::Default => ().serve(transport).await.map(ConnectedClient::Default),
            Self::Host(host) => host
                .into_dyn()
                .serve(transport)
                .await
                .map(ConnectedClient::Host),
        }
    }
}

/// Keeps the existing already-running unit constructor source-compatible.
/// rmcp can erase a service before serve, but cannot publicly erase a running
/// service. This owner does not wrap or recreate protocol dispatch.
pub(crate) enum ConnectedClient {
    Default(RunningService<RoleClient, ()>),
    Host(RunningService<RoleClient, Box<dyn DynService<RoleClient>>>),
}

impl From<RunningService<RoleClient, ()>> for ConnectedClient {
    fn from(service: RunningService<RoleClient, ()>) -> Self {
        Self::Default(service)
    }
}

impl Deref for ConnectedClient {
    type Target = Peer<RoleClient>;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Default(service) => service.peer(),
            Self::Host(service) => service.peer(),
        }
    }
}

impl ConnectedClient {
    pub(crate) fn supports_mcp_apps(&self) -> bool {
        let Self::Host(service) = self else {
            return false;
        };
        service
            .service()
            .get_info()
            .capabilities
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get(crate::apps::MCP_APPS_EXTENSION))
            .and_then(|extension| extension.get("mimeTypes"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|types| {
                types
                    .iter()
                    .any(|mime| mime.as_str() == Some(crate::apps::MCP_APP_RESOURCE_MIME_TYPE))
            })
    }

    pub(crate) async fn cancel(self) -> Result<QuitReason, tokio::task::JoinError> {
        match self {
            Self::Default(service) => service.cancel().await,
            Self::Host(service) => service.cancel().await,
        }
    }
}

pub(crate) struct FormClient {
    host: Box<dyn DynService<RoleClient>>,
    info: ClientInfo,
    forms_enabled: bool,
}

impl FormClient {
    fn new(host: Box<dyn DynService<RoleClient>>) -> Self {
        // Keep protocol version, implementation identity and client metadata.
        // Restrict only capabilities, and snapshot them for this connection.
        let mut info = host.get_info();
        let form = info
            .capabilities
            .elicitation
            .as_ref()
            .and_then(|e| e.form.clone());
        let forms_enabled = form.is_some();
        let ui = info
            .capabilities
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get(crate::apps::MCP_APPS_EXTENSION))
            .cloned();
        info.capabilities = ClientCapabilities::default();
        if let Some(ui) = ui {
            info.capabilities.extensions =
                Some([(crate::apps::MCP_APPS_EXTENSION.into(), ui)].into());
        }
        if forms_enabled {
            info.capabilities.elicitation = Some(ElicitationCapability { form, url: None });
        }
        Self {
            host,
            info,
            forms_enabled,
        }
    }

    fn retired() -> ErrorData {
        // The pinned rmcp has no cancellation ErrorCode constant. This is a
        // protocol failure, never a synthesized user Decline or Cancel action.
        ErrorData::invalid_request("form elicitation request is no longer active", None)
    }
}

impl ClientHandler for FormClient {
    fn get_info(&self) -> ClientInfo {
        self.info.clone()
    }

    async fn create_elicitation(
        &self,
        request: CreateElicitationRequestParams,
        context: RequestContext<RoleClient>,
    ) -> Result<CreateElicitationResult, ErrorData> {
        if !self.forms_enabled
            || !matches!(
                &request,
                CreateElicitationRequestParams::FormElicitationParams { .. }
            )
        {
            return ().create_elicitation(request, context).await;
        }
        let cancellation = context.ct.clone();
        let peer = context.peer.clone();
        if cancellation.is_cancelled() || peer.is_transport_closed() {
            return Err(Self::retired());
        }
        let response = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(Self::retired()),
            response = self.host.handle_request(
                ServerRequest::CreateElicitationRequest(CreateElicitationRequest::new(request)),
                context,
            ) => response,
        };
        if cancellation.is_cancelled() || peer.is_transport_closed() {
            return Err(Self::retired());
        }
        match response? {
            ClientResult::CreateElicitationResult(result) => Ok(result),
            _ => Err(ErrorData::internal_error(
                "host returned a non-elicitation response",
                None,
            )),
        }
    }
}
