//! Fixed, host-commissioned native JSONL ingress.
//!
//! This additive entry owns one private runtime and one callback connection.
//! Wire requests cannot choose principals, grants, factories, setup tools or
//! observation authority. Mutable/cold/MCP setup is outside this profile.

use crate::{
    error,
    protocol::{RpcError, RpcId, RpcResponse},
    router::NotificationSink,
    server::{RpcServer, ServerError},
    session_runtime::SessionRuntime,
};
use meerkat::{AgentFactory, PersistenceBundle};
use meerkat_authorization_contracts::{
    evidence::HistoricalEvidenceRef, work_association::InputAuthorityAssociation,
};
use meerkat_core::connection::RealmId;
use meerkat_core::{
    Config, ConfigStore, ControllerModelClient, MemoryConfigStore, PrincipalRef, ToolDef,
};
use meerkat_llm_core::LlmClient;
use meerkat_runtime::input_authority::{NativeAdmissionError, NativeIngressContext};
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;
use meerkat_runtime::{RuntimeDriverError, identifiers::LogicalRuntimeId, input::Input};
use serde_json::value::RawValue;
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncWrite};

/// Historical attribution supplied by the trusted embedding for the final
/// input and exact pinned controller. It cannot grant native permission.
/// The installed native ingress owner authenticates the result independently.
pub type InputAssociationProducer = dyn Fn(
        &LogicalRuntimeId,
        &Input,
        &ControllerModelClient,
    ) -> Result<InputAuthorityAssociation, NativeAdmissionError>
    + Send
    + Sync;

/// Process-only authentication for one commissioned connection. No Serde impl.
pub struct GovernedConnection {
    requester: PrincipalRef,
    represented_subject: Option<PrincipalRef>,
    ingress_actor: PrincipalRef,
    realm: RealmId,
    authentication: HistoricalEvidenceRef,
    association: Arc<InputAssociationProducer>,
}
impl GovernedConnection {
    pub fn new(
        requester: PrincipalRef,
        represented_subject: Option<PrincipalRef>,
        ingress_actor: PrincipalRef,
        realm: RealmId,
        authentication: HistoricalEvidenceRef,
        association: Arc<InputAssociationProducer>,
    ) -> Result<Self, RuntimeDriverError> {
        for principal in std::iter::once(&requester)
            .chain(std::iter::once(&ingress_actor))
            .chain(represented_subject.iter())
            .chain(std::iter::once(&authentication.resource.domain.authority))
        {
            principal.validate_qualified().map_err(|_| unsupported())?;
        }
        Ok(Self {
            requester,
            represented_subject,
            ingress_actor,
            realm,
            authentication,
            association,
        })
    }

    pub(crate) fn bind(
        &self,
        runtime: &LogicalRuntimeId,
        mut input: Input,
        pin: ControllerModelClient,
    ) -> Result<Input, RuntimeDriverError> {
        let association = (self.association)(runtime, &input, &pin)?;
        let candidate = association.candidate();
        if candidate.requester != self.requester
            || candidate.represented_subject != self.represented_subject
            || candidate.ingress_actor != self.ingress_actor
            || candidate.ingress_namespace.realm != self.realm
            || candidate.controller_model.as_ref() != Some(pin.selection())
        {
            return Err(
                NativeAdmissionError::Refused(meerkat_core::OperationRefused::new(
                    meerkat_core::OperationRefusalKind::MalformedFacts,
                ))
                .into(),
            );
        }
        let Input::Prompt(prompt) = &mut input else {
            return Err(unsupported());
        };
        prompt.header.authority_association = Some(association);
        let ingress = NativeIngressContext::from_trusted_ingress(
            &input,
            self.requester.clone(),
            self.ingress_actor.clone(),
            self.realm.clone(),
            self.authentication.clone(),
        )?
        .with_controller_client(&input, pin)?;
        let Input::Prompt(prompt) = &mut input else {
            return Err(unsupported());
        };
        prompt.header.ingress_context = Some(Arc::new(ingress));
        Ok(input)
    }
}

/// Trusted fixed recipe. The factory is constructed internally as minimal;
/// sharing the supplied bundle before installation is rejected by its owner.
pub struct GovernedJsonlSetup {
    pub config: Config,
    pub persistence: PersistenceBundle,
    pub authorization: NativeGrantWorkConfiguration,
    pub client: Arc<dyn LlmClient>,
    pub connection: GovernedConnection,
    pub tools: Vec<ToolDef>,
}
#[derive(Debug, thiserror::Error)]
pub enum GovernedJsonlError {
    #[error(transparent)]
    Runtime(#[from] RuntimeDriverError),
    #[error(transparent)]
    Server(#[from] ServerError),
}

pub(crate) fn unsupported() -> RuntimeDriverError {
    RuntimeDriverError::ControllerReadinessUnavailable {
        reason: meerkat_runtime::traits::ControllerReadinessFailure::UnsupportedScope,
    }
}

/// Serve exactly one privately owned fixed connection. This does not activate
/// governance in any existing stdio/TCP/WS constructor or expose runtime aliases.
pub async fn serve_governed_jsonl<R, W>(
    reader: R,
    writer: W,
    setup: GovernedJsonlSetup,
) -> Result<(), GovernedJsonlError>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (mut server, _runtime) = construct(reader, writer, setup)?;
    server.run().await?;
    Ok(())
}

fn construct<R, W>(
    reader: R,
    writer: W,
    setup: GovernedJsonlSetup,
) -> Result<(RpcServer<R, W>, Arc<SessionRuntime>), GovernedJsonlError>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let GovernedJsonlSetup {
        config,
        persistence,
        authorization,
        client,
        connection,
        mut tools,
    } = setup;
    // This check precedes factory, router, channel and materialization effects.
    if cfg!(feature = "mcp")
        || config.skills.enabled
        || !config.skills.repositories.is_empty()
        || !config.hooks.entries.is_empty()
        || !config.tools.mcp_servers.is_empty()
        || config.tools.builtins_enabled
        || config.tools.shell_enabled
        || config.tools.comms_enabled
        || config.tools.mob_enabled
        || config.tools.schedule_enabled
        || config.tools.workgraph_enabled
        || config.agent.system_prompt_file.is_some()
        || config.model_fallback.is_enabled()
    {
        return Err(unsupported().into());
    }
    let selection = client
        .controller_model_selection()
        .ok_or_else(unsupported)?;
    if selection.model() != config.agent.model
        || selection.auth_binding().is_none()
        || selection
            .auth_binding()
            .is_some_and(|binding| binding.realm != connection.realm)
    {
        return Err(unsupported().into());
    }
    let mut names = std::collections::BTreeSet::new();
    if tools.is_empty()
        || tools
            .iter()
            .any(|tool| tool.name.is_empty() || !names.insert(tool.name.clone()))
    {
        return Err(unsupported().into());
    }
    for tool in &mut tools {
        tool.provenance = Some(meerkat_core::types::ToolProvenance {
            kind: meerkat_core::types::ToolSourceKind::Callback,
            source_id: "callback".into(),
        });
    }
    let persistence = persistence.with_local_grant_authorization(authorization)?;
    let max_sessions = config.max_sessions();
    let config_store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
        config.clone(),
        meerkat_models::canonical(),
    ));
    let mut runtime = SessionRuntime::new_with_config_store(
        AgentFactory::minimal(),
        config,
        config_store.clone(),
        max_sessions,
        persistence,
        NotificationSink::noop(),
    );
    runtime.use_commissioned_config_store(config_store.clone());
    runtime.set_realm_context(Some(connection.realm.clone()), None, None);
    runtime.set_default_llm_client(Some(client));
    let runtime = Arc::new(runtime);
    let server = RpcServer::new(reader, writer, runtime.clone(), config_store)
        .with_governed_connection(Arc::new(connection));
    // RpcServer::new first installs its sole callback channel and resets the
    // registry. This is the only catalog mutation in the commissioned lifetime.
    runtime
        .callback_tool_registry()
        .replace_or_add(tools)
        .map_err(|_| unsupported())?;
    Ok((server, runtime))
}

fn checked_params<'a>(
    params: Option<&'a RawValue>,
    allowed: &[&str],
) -> Result<&'a RawValue, RpcError> {
    let params = params.ok_or_else(|| invalid("missing params"))?;
    let value: serde_json::Value =
        serde_json::from_str(params.get()).map_err(|_| invalid("invalid params"))?;
    let object = value
        .as_object()
        .ok_or_else(|| invalid("params must be an object"))?;
    if object.keys().any(|name| !allowed.contains(&name.as_str())) {
        return Err(invalid("unsupported parameter for fixed governed JSONL"));
    }
    Ok(params)
}
fn invalid(message: &str) -> RpcError {
    RpcError {
        code: error::INVALID_PARAMS,
        message: message.into(),
        data: None,
    }
}

pub(crate) async fn dispatch(
    connection: &Arc<GovernedConnection>,
    method: &str,
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
    sink: &NotificationSink,
    context: Option<meerkat::surface::RequestContext>,
) -> RpcResponse {
    let result = async {
        match method {
            "initialize" => Ok(RpcResponse::success(
                id.clone(),
                crate::handlers::initialize::ServerCapabilities {
                    server_info: crate::handlers::initialize::ServerInfo {
                        name: "meerkat-rpc".into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                    },
                    contract_version: meerkat_contracts::ContractVersion::CURRENT.to_string(),
                    methods: [
                        "initialize",
                        "initialized",
                        "cancel",
                        "session/create",
                        "turn/start",
                    ]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                },
            )),
            "session/create" => {
                let raw = checked_params(params, &["prompt", "injected_context", "initial_turn"])?;
                let mut params: crate::handlers::session::CreateSessionParams =
                    serde_json::from_str(raw.get())
                        .map_err(|_| invalid("invalid create parameters"))?;
                if params.initial_turn != Some(crate::handlers::session::InitialTurn::Deferred) {
                    return Err(invalid("fixed governed JSONL requires deferred create"));
                }
                params.external_tools = Some(runtime.callback_tool_registry().snapshot());
                params.auth_binding = runtime
                    .default_llm_client()
                    .and_then(|client| client.controller_model_selection())
                    .and_then(|selection| selection.auth_binding().cloned())
                    .map(Into::into);
                Ok(crate::handlers::session::create_session_with_params(
                    id.clone(),
                    params,
                    runtime.clone(),
                    sink,
                    &runtime.runtime_adapter(),
                    context,
                )
                .await)
            }
            "turn/start" => {
                let raw = checked_params(params, &["session_id", "prompt", "injected_context"])?;
                let params: crate::handlers::turn::StartTurnParams =
                    serde_json::from_str(raw.get())
                        .map_err(|_| invalid("invalid turn parameters"))?;
                let session_id = meerkat_core::SessionId::parse(&params.session_id)
                    .map_err(|_| invalid("invalid session id"))?;
                let run = runtime
                    .start_governed_turn(
                        &session_id,
                        params.prompt,
                        params.injected_context.unwrap_or_default(),
                        connection.clone(),
                        context,
                    )
                    .await?;
                let mut result: crate::handlers::turn::TurnResult = run.into();
                result.session_ref = runtime
                    .realm_id()
                    .map(|realm| meerkat_contracts::format_session_ref(&realm, &result.session_id));
                Ok(RpcResponse::success(id.clone(), result))
            }
            _ => Err(crate::session_runtime::runtime_driver_error_to_rpc(
                unsupported(),
            )),
        }
    }
    .await;
    match result {
        Ok(response) => response,
        Err(error) => RpcResponse::from_error(id, error),
    }
}

#[cfg(test)]
#[path = "governed_jsonl/tests.rs"]
mod tests;
