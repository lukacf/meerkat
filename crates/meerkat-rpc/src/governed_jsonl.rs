//! Fixed, host-commissioned native JSONL ingress.
//!
//! This additive entry owns one private runtime and one callback connection.
//! Wire requests cannot choose principals, grants, factories, setup tools or
//! observation authority. Mutable/cold/MCP setup is outside this profile,
//! with one bounded extension: when the trusted setup supplies each session's
//! scoped destination, this connection binds it at create, and (with the MCP
//! feature) may stage a live `mcp/add` for that session under an
//! authenticated attachment control prepared by the native owners.

use crate::{
    error,
    protocol::{RpcError, RpcId, RpcResponse},
    router::NotificationSink,
    server::{RpcServer, ServerError},
    session_runtime::SessionRuntime,
};
use meerkat::{AgentFactory, PersistenceBundle};
use meerkat_authorization_contracts::{
    evidence::HistoricalEvidenceRef,
    work_association::{InputAuthorityAssociation, NativeWorkTarget},
};
use meerkat_core::connection::RealmId;
use meerkat_core::{
    Config, ConfigStore, ControllerModelClient, MemoryConfigStore, PrincipalRef, ToolDef,
};
use meerkat_llm_core::LlmClient;
use meerkat_runtime::input_authority::{NativeAdmissionError, NativeIngressContext};
use meerkat_runtime::{
    MeerkatMachine, RuntimeDriverError, identifiers::LogicalRuntimeId, input::Input,
};
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

/// The trusted host's binding of the sessions this connection creates.
///
/// The host owns each session's scoped destination: it binds the exact
/// native target on the session's current registration and returns it. A
/// host may also bind each governed input itself (for example to an
/// authenticated original it records); otherwise the connection binds the
/// input with its own association. Nothing is inferred from the realm, the
/// wire or input claims.
#[async_trait::async_trait]
pub trait GovernedSessionBinder: Send + Sync {
    /// Bind `session_id`, just created by this connection, to its
    /// destination before any scoped input or MCP entry, and return the
    /// exact bound target.
    async fn bind_session(
        &self,
        machine: &MeerkatMachine,
        session_id: &meerkat_core::SessionId,
    ) -> Result<NativeWorkTarget, RuntimeDriverError>;

    /// Bind one governed input of `session_id` with the session's pinned
    /// controller, or `None` to let the connection bind it.
    async fn bind_input(
        &self,
        _machine: &MeerkatMachine,
        _session_id: &meerkat_core::SessionId,
        _input: Input,
        _pin: ControllerModelClient,
    ) -> Result<Option<GovernedBoundInput>, RuntimeDriverError> {
        Ok(None)
    }

    /// Whether this host's connection may serve no fixed callback tools.
    /// Only an explicit stock-governed host, whose own native owners decide
    /// every operation, may; other profiles keep requiring them.
    fn allows_empty_callback_tools(&self) -> bool {
        false
    }

    /// The realm's own MCP installations, as the realm's installation
    /// authority holds them: the current installation a resumed session's
    /// attachment restore resolves (a live-attach installation is not one
    /// of the host's configured servers). `None` (the default) restores
    /// against the runtime's configured installations.
    fn realm_mcp_installations(&self) -> Option<Vec<meerkat_core::McpServerConfig>> {
        None
    }

    /// Bind a deferred session of this connection again, on its actual
    /// current registration, right before its promotion builds the agent
    /// (a registration replaced since the create is not bound by the
    /// create's binding), and return the exact bound target. `None` (the
    /// default) keeps the create's binding.
    async fn bind_promoting_session(
        &self,
        _machine: &MeerkatMachine,
        _session_id: &meerkat_core::SessionId,
    ) -> Result<Option<NativeWorkTarget>, RuntimeDriverError> {
        Ok(None)
    }

    /// Whether this host's composition can execute `server` at all, decided
    /// on the actual descriptor before any attachment effect (a live add or
    /// a resumed session's restore). A descriptor the composition does not
    /// support is refused locally; every other check still applies to the
    /// rest. All descriptors by default.
    fn supports_attachment_descriptor(&self, _server: &meerkat_core::McpServerConfig) -> bool {
        true
    }

    /// The full configured MCP server descriptors (access ceiling and
    /// connector binding included) this host's composition carries into
    /// each created session's build. Each is a configured row, not
    /// authority: the installed governed host decides it at its
    /// connection's physical entry. None by default.
    fn configured_mcp_servers(&self) -> Vec<meerkat_core::McpServerConfig> {
        Vec::new()
    }
}

/// One input the host bound: the bound input, the exact destination it names
/// (now the session's current binding) and its custody, settled once the
/// runtime decided the input.
pub struct GovernedBoundInput {
    pub input: Input,
    pub target: NativeWorkTarget,
    pub custody: Box<dyn GovernedInputCustody>,
}

/// A host's custody of one bound input, settled from the definitive native
/// settlement outside the native guards. Dropping it withdraws it.
pub trait GovernedInputCustody: Send {
    /// The runtime accepted exactly this input: promote the original. A
    /// failure is a typed post-admission owner fault; the input stands.
    fn admit(self: Box<Self>) -> Result<(), GovernedCustodyFault>;

    /// The native owner may have admitted the input (an uncertain
    /// settlement, or an error returned after admission): keep the candidate
    /// as it is, neither promoted nor withdrawn.
    fn retain(self: Box<Self>);
}

/// The host could not publish the original of an input the runtime admitted.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct GovernedCustodyFault(pub String);

/// Process-only authentication for one commissioned connection. No Serde impl.
pub struct GovernedConnection {
    requester: PrincipalRef,
    represented_subject: Option<PrincipalRef>,
    ingress_actor: PrincipalRef,
    realm: RealmId,
    authentication: HistoricalEvidenceRef,
    association: Arc<InputAssociationProducer>,
    binder: Option<Arc<dyn GovernedSessionBinder>>,
    /// This pipe's part of R3 human consent, when the host composed one: its
    /// operator decides on it through `approval/consent_decide`.
    consent: Option<Arc<stock_consent::StdioConsentSurface>>,
    /// The pipe's operator for durable decisions, when the host named one:
    /// the connection's decision operator is built from it when the
    /// connection starts.
    #[cfg(unix)]
    decision_approver: Option<meerkat_core::approval::consent::AuthenticatedApprover>,
    /// The connection's operator for durable decisions: `decision/list`
    /// presents to it and `decision/decide` decides as it.
    #[cfg(unix)]
    decisions: Option<Arc<crate::handlers::decision::operator::DecisionOperator>>,
    /// The exact target each session was last bound to by this connection,
    /// retained for its attachment controls.
    bound: std::sync::Mutex<std::collections::HashMap<meerkat_core::SessionId, NativeWorkTarget>>,
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
            binder: None,
            consent: None,
            #[cfg(unix)]
            decision_approver: None,
            #[cfg(unix)]
            decisions: None,
            bound: std::sync::Mutex::default(),
        })
    }

    /// Serve durable decisions on this pipe to its operator `approver`, who
    /// must carry the pipe's own authentication scope: `decision/list`
    /// presents the decisions of each listed page the operator acts for, and
    /// `decision/decide` decides as the operator. The approver ids the
    /// operator acts for are the host's configuration as the pipe started
    /// (`[governance.decisions]`); a change reaches the pipe at its next
    /// start.
    #[cfg(unix)]
    pub(crate) fn serve_decisions_as(
        &mut self,
        approver: meerkat_core::approval::consent::AuthenticatedApprover,
    ) {
        self.decision_approver = Some(approver);
    }

    /// The connection closed: its pipe's consent scope retires here, at the
    /// close boundary, so no eligibility observed for the pipe stays current
    /// while the connection's handlers and sessions drain. So does its
    /// operator for durable decisions.
    pub(crate) fn close(&self) {
        if let Some(consent) = &self.consent {
            consent.retire();
        }
        #[cfg(unix)]
        if let Some(decisions) = &self.decisions {
            decisions.close();
        }
    }

    /// Whether this pipe serves durable decisions to its operator.
    #[cfg(unix)]
    fn serves_decisions(&self) -> bool {
        self.decisions.is_some()
    }

    #[cfg(not(unix))]
    fn serves_decisions(&self) -> bool {
        false
    }

    /// The pipe's operator for durable decisions, or the unsupported refusal
    /// of a pipe that has none.
    #[cfg(unix)]
    fn decision_operator(
        &self,
    ) -> Result<&crate::handlers::decision::operator::DecisionOperator, RpcError> {
        self.decisions
            .as_deref()
            .ok_or_else(|| crate::session_runtime::runtime_driver_error_to_rpc(unsupported()))
    }

    /// Live `mcp/add` needs the scoped destination its control is joined to.
    /// Without one, a build with the MCP feature refuses the setup as before.
    #[cfg(feature = "mcp")]
    fn mcp_commissioned(&self) -> bool {
        self.binder.is_some()
    }

    /// The realm's own MCP installations this connection's composition
    /// resolves a resumed session's attachment restore against, if any.
    #[cfg(feature = "mcp")]
    pub(crate) fn realm_mcp_installations(&self) -> Option<Vec<meerkat_core::McpServerConfig>> {
        self.binder
            .as_ref()
            .and_then(|binder| binder.realm_mcp_installations())
    }

    #[cfg(feature = "mcp")]
    fn bound_target(&self, session_id: &meerkat_core::SessionId) -> Option<NativeWorkTarget> {
        self.bound
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned()
    }

    /// Bind the trusted destination of a session this connection just
    /// created to its current registration, before any scoped input or MCP
    /// entry, and retain the exact target. A session that cannot be bound is
    /// archived through the staged cleanup path (its capacity and callback
    /// route included); the refusal keeps any cleanup failure.
    async fn bind_created_session(
        &self,
        runtime: &Arc<SessionRuntime>,
        created: &RpcResponse,
    ) -> Result<(), RpcError> {
        let Some(binder) = &self.binder else {
            return Ok(());
        };
        let refused = || crate::session_runtime::runtime_driver_error_to_rpc(unsupported());
        let session_id = created
            .result
            .as_ref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw.get()).ok())
            .and_then(|result| result["session_id"].as_str().map(str::to_owned))
            .and_then(|raw| meerkat_core::SessionId::parse(&raw).ok())
            .ok_or_else(refused)?;
        let bound = binder
            .bind_session(&runtime.runtime_adapter(), &session_id)
            .await
            .map_err(crate::session_runtime::runtime_driver_error_to_rpc);
        match bound {
            Ok(target) => {
                self.retain_target(session_id, target);
                Ok(())
            }
            Err(mut error) => {
                if let Err(cleanup) = runtime.archive_session(&session_id).await {
                    error.message = format!(
                        "{}; staged session cleanup failed: {}",
                        error.message, cleanup.message
                    );
                }
                Err(error)
            }
        }
    }

    #[cfg(not(feature = "mcp"))]
    fn mcp_commissioned(&self) -> bool {
        false
    }

    /// The host's configured MCP descriptors for a session this connection
    /// creates.
    pub(crate) fn configured_mcp_servers(&self) -> Vec<meerkat_core::McpServerConfig> {
        self.binder
            .as_ref()
            .map(|binder| binder.configured_mcp_servers())
            .unwrap_or_default()
    }

    /// The host's binding of a deferred session right before its promotion
    /// builds the agent ([`GovernedSessionBinder::bind_promoting_session`]),
    /// retaining the exact target it names. The caller holds the session's
    /// registration lock and fails the promotion on error.
    pub(crate) async fn bind_promoting_session(
        &self,
        machine: &MeerkatMachine,
        session_id: &meerkat_core::SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let Some(binder) = &self.binder else {
            return Ok(());
        };
        if let Some(target) = binder.bind_promoting_session(machine, session_id).await? {
            self.retain_target(session_id.clone(), target);
        }
        Ok(())
    }

    fn retain_target(&self, session_id: meerkat_core::SessionId, target: NativeWorkTarget) {
        self.bound
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id, target);
    }

    /// Bind one governed input: through the host binder when it binds inputs
    /// (retaining the destination it names), otherwise with this
    /// connection's association.
    pub(crate) async fn bind_input(
        &self,
        machine: &MeerkatMachine,
        session_id: &meerkat_core::SessionId,
        input: Input,
        pin: ControllerModelClient,
    ) -> Result<(Input, Option<Box<dyn GovernedInputCustody>>), RuntimeDriverError> {
        if let Some(binder) = &self.binder
            && let Some(bound) = binder
                .bind_input(machine, session_id, input.clone(), pin.clone())
                .await?
        {
            self.retain_target(session_id.clone(), bound.target);
            return Ok((bound.input, Some(bound.custody)));
        }
        self.bind(&LogicalRuntimeId::for_session(session_id), input, pin)
            .map(|input| (input, None))
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

/// Trusted fixed recipe with an already governed persistence owner.
/// The factory is constructed internally as minimal.
pub struct GovernedJsonlSetup {
    pub config: Config,
    pub persistence: PersistenceBundle,
    pub client: Arc<dyn LlmClient>,
    pub connection: GovernedConnection,
    pub tools: Vec<ToolDef>,
    /// The trusted host's binding of each created session (bound at create)
    /// and, optionally, of its inputs.
    pub session_binder: Option<Arc<dyn GovernedSessionBinder>>,
    /// The host's MCP attachment identity-key custody: the protected store of
    /// the realm's key that binds an accepted attachment's credential-bearing
    /// target (its command arguments or URL credentials), so that a later
    /// host can prove and restore it. It serves only those attachment
    /// identity operations, never provider or MCP credential resolution.
    /// `None` records such an attachment unbound, and a restore refuses it as
    /// needing renewal.
    pub mcp_attachment_identity_key_custody: Option<meerkat::ProviderAuthPersistence>,
    /// The host's connector authority for connector-bound configured MCP
    /// servers: each session's build supplies the connector call context of
    /// each call's own staged binding from it. `None` composes none (a
    /// connector-bound server's calls are then refused at the router).
    pub connector_authority: Option<meerkat::ConnectorOAuthAuthority>,
    /// The realm owner's operation review, installed on the connection's
    /// factory before anything clones it; `None` keeps every operation at
    /// its owners' own tier with no reviewer.
    pub operation_review: Option<Arc<meerkat_core::approval::review::BoundOperationReview>>,
    /// This pipe's consent surface, when the review routes R3 to it.
    pub consent: Option<Arc<stock_consent::StdioConsentSurface>>,
    /// The host's OS process confinement, installed on the connection's
    /// factory: its live MCP stdio servers launch inside it.
    pub host_confinement: meerkat::HostConfinement,
}

/// Process-only receipt of what this connection's trusted ingress
/// authenticated, carried as a governed MCP attachment control's evidence.
/// The installed owners downcast it and verify it against their own records.
/// Its authentication reference only identifies the original observation; it
/// is not authentication, custody or currentness, so an owner resolves the
/// current observation it names through its own installed records before
/// allowing anything. No Serde impl.
#[cfg(feature = "mcp")]
pub struct GovernedJsonlAttachmentReceipt {
    requester: PrincipalRef,
    ingress_actor: PrincipalRef,
    realm: RealmId,
    authentication: HistoricalEvidenceRef,
    session_id: meerkat_core::SessionId,
    target: NativeWorkTarget,
}

#[cfg(feature = "mcp")]
impl GovernedJsonlAttachmentReceipt {
    pub fn requester(&self) -> &PrincipalRef {
        &self.requester
    }
    pub fn ingress_actor(&self) -> &PrincipalRef {
        &self.ingress_actor
    }
    pub fn realm(&self) -> &RealmId {
        &self.realm
    }
    pub fn authentication(&self) -> &HistoricalEvidenceRef {
        &self.authentication
    }
    pub fn session_id(&self) -> &meerkat_core::SessionId {
        &self.session_id
    }
    /// The exact scoped destination the session was bound to.
    pub fn target(&self) -> &NativeWorkTarget {
        &self.target
    }
}
#[derive(Debug, thiserror::Error)]
pub enum GovernedJsonlError {
    #[error(transparent)]
    Runtime(#[from] RuntimeDriverError),
    #[error(transparent)]
    Server(#[from] ServerError),
    #[error(transparent)]
    Composition(#[from] meerkat::surface::RuntimeBackedCompositionError),
}

pub(crate) fn unsupported() -> RuntimeDriverError {
    RuntimeDriverError::ControllerReadinessUnavailable {
        reason: meerkat_runtime::traits::ControllerReadinessFailure::UnsupportedScope,
    }
}

/// Serve exactly one privately owned fixed connection. This does not activate
/// governance in any existing stdio/TCP/WS constructor or expose runtime aliases.
///
/// The returned future owns the pipe's consent retirement from the moment it
/// is created: dropped at any point, never polled included, it retires the
/// pipe's consent scope.
pub fn serve_governed_jsonl<R, W>(
    reader: R,
    writer: W,
    setup: GovernedJsonlSetup,
) -> impl std::future::Future<Output = Result<(), GovernedJsonlError>>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let retirement = consent_retirement(&setup);
    async move {
        let (serving, _runtime) = governed_serving(reader, writer, setup, None, retirement)?;
        serving.await?;
        Ok(())
    }
}

/// [`serve_governed_jsonl`] that also stops on `shutdown` (process
/// termination): the connection stops reading and drains, and the runtime
/// shuts down, as the ordinary stdio server does. The returned future owns
/// the pipe's consent retirement from its creation, as there.
pub fn serve_governed_jsonl_until<R, W>(
    reader: R,
    writer: W,
    setup: GovernedJsonlSetup,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> impl std::future::Future<Output = Result<(), GovernedJsonlError>>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let retirement = consent_retirement(&setup);
    async move {
        let (fire, signal) = tokio::sync::watch::channel(false);
        let (serving, _runtime) =
            governed_serving(reader, writer, setup, Some(signal), retirement)?;
        let watcher = tokio::spawn(async move {
            shutdown.await;
            fire.send_replace(true);
        });
        let served = serving.await;
        watcher.abort();
        served?;
        Ok(())
    }
}

/// The guard that retires `setup`'s pipe consent scope when it is dropped,
/// taken when the pipe's serving future is created.
fn consent_retirement(setup: &GovernedJsonlSetup) -> Option<stock_consent::StdioConsentRetirement> {
    setup
        .consent
        .as_ref()
        .map(stock_consent::StdioConsentSurface::retirement)
}

/// The serving future of one governed connection, and the connection's
/// runtime. The future owns `retirement`, the pipe's consent retirement its
/// caller took when it created the pipe's serving future: the scope retires
/// at the connection's close boundary, before its handlers drain
/// ([`RpcServer::run`]), and whenever the future is dropped, or construction
/// fails, before that.
fn governed_serving<R, W>(
    reader: R,
    writer: W,
    setup: GovernedJsonlSetup,
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
    retirement: Option<stock_consent::StdioConsentRetirement>,
) -> Result<
    (
        impl std::future::Future<Output = Result<(), ServerError>>,
        Arc<SessionRuntime>,
    ),
    GovernedJsonlError,
>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (mut server, runtime) = construct(reader, writer, setup)?;
    if let Some(signal) = shutdown {
        server.set_shutdown_signal(signal);
    }
    Ok((
        async move {
            let _retirement = retirement;
            server.run().await
        },
        runtime,
    ))
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
        mcp_attachment_identity_key_custody,
        persistence,
        client,
        mut connection,
        mut tools,
        session_binder,
        connector_authority,
        operation_review,
        consent,
        host_confinement,
    } = setup;
    connection.binder = session_binder;
    connection.consent = consent;
    // This check precedes factory, router, channel and materialization effects.
    // The MCP feature is accepted only for a connection commissioned for live
    // attachment; configured servers stay outside this profile's own config
    // either way (a trusted host composition carries its own descriptors).
    if (cfg!(feature = "mcp") && !connection.mcp_commissioned())
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
    let empty_allowed = connection
        .binder
        .as_ref()
        .is_some_and(|binder| binder.allows_empty_callback_tools());
    if (tools.is_empty() && !empty_allowed)
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
            artifact: None,
        });
    }
    if !persistence
        .runtime_adapter()
        .has_native_work_authorization_host()
    {
        return Err(unsupported().into());
    }
    let max_sessions = config.max_sessions();
    let config_store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
        config.clone(),
        meerkat_models::canonical(),
    ));
    // The connector owner only: the credential store stays detached, so
    // the fixed governed profile gains no other provider or MCP auth.
    #[cfg(feature = "mcp")]
    let factory = match connector_authority {
        Some(authority) => AgentFactory::minimal().connector_oauth_authority(authority),
        None => AgentFactory::minimal(),
    };
    // Without MCP no server connects; there is nothing to authorize.
    #[cfg(not(feature = "mcp"))]
    let factory = {
        let _ = connector_authority;
        AgentFactory::minimal()
    };
    let factory = factory.with_host_confinement(&host_confinement);
    // The realm owner's review is installed before the runtime clones the
    // factory, so the runtime installs its issuing consent owner.
    let factory = match operation_review {
        Some(review) => factory.with_operation_review(review),
        None => factory,
    };
    let mut runtime = SessionRuntime::new_with_config_store(
        factory,
        config,
        config_store.clone(),
        max_sessions,
        persistence,
        NotificationSink::noop(),
    )?;
    runtime.use_commissioned_config_store(config_store.clone());
    runtime.set_realm_context(Some(connection.realm.clone()), None, None);
    runtime.set_default_llm_client(Some(client));
    // The identity-key custody reaches only the runtime's attachment identity
    // paths; the factory above stays minimal.
    #[cfg(feature = "mcp")]
    runtime.use_mcp_attachment_identity_key_custody(mcp_attachment_identity_key_custody);
    #[cfg(not(feature = "mcp"))]
    let _ = mcp_attachment_identity_key_custody;
    let runtime = Arc::new(runtime);
    let server = RpcServer::new(reader, writer, runtime.clone(), config_store)?;
    // The pipe's operator for durable decisions presents through this
    // connection's own notices, each written before it is recorded.
    #[cfg(unix)]
    if let Some(approver) = connection.decision_approver.take() {
        connection.decisions = crate::handlers::decision::operator::DecisionOperator::new(
            approver,
            server.acknowledged_notification_sender(),
        )
        .map(Arc::new);
    }
    let server = server.with_governed_connection(Arc::new(connection), tools)?;
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch(
    connection: &Arc<GovernedConnection>,
    method: &str,
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
    sink: &NotificationSink,
    callback_route: Option<crate::callback_dispatcher::CallbackRoute>,
    context: Option<meerkat::surface::RequestContext>,
) -> RpcResponse {
    if let Some(consent) = &connection.consent {
        consent.attach_notices(sink);
    }
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
                    .chain(connection.mcp_commissioned().then_some("mcp/add"))
                    .chain(
                        connection
                            .consent
                            .is_some()
                            .then_some(crate::router::CONSENT_DECIDE_METHOD),
                    )
                    .chain(connection.serves_decisions().then_some("decision/list"))
                    .chain(
                        connection
                            .serves_decisions()
                            .then_some(crate::router::DECISION_DECIDE_METHOD),
                    )
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
                let route = callback_route
                    .filter(|route| !route.is_closed())
                    .ok_or_else(|| {
                        crate::session_runtime::runtime_driver_error_to_rpc(unsupported())
                    })?;
                params.external_tools = Some(route.registry().snapshot());
                params.auth_binding = runtime
                    .default_llm_client()
                    .and_then(|client| client.controller_model_selection())
                    .and_then(|selection| selection.auth_binding().cloned())
                    .map(Into::into);
                let created =
                    crate::handlers::session::create_session_with_params_on_route_and_configured_mcp(
                        id.clone(),
                        params,
                        runtime.clone(),
                        sink,
                        &runtime.runtime_adapter(),
                        context,
                        Some(route),
                        connection.configured_mcp_servers(),
                        None,
                    )
                    .await;
                if created.error.is_none() {
                    connection.bind_created_session(runtime, &created).await?;
                }
                Ok(created)
            }
            "turn/start" => {
                let raw = checked_params(params, &["session_id", "prompt", "injected_context"])?;
                let params: crate::handlers::turn::StartTurnParams =
                    serde_json::from_str(raw.get())
                        .map_err(|_| invalid("invalid turn parameters"))?;
                let session_id = meerkat_core::SessionId::parse(&params.session_id)
                    .map_err(|_| invalid("invalid session id"))?;
                // A previous host's durable session is recovered first, bound
                // on its new registration before its agent is built.
                runtime
                    .recover_cold_governed_session(
                        &session_id,
                        connection,
                        callback_route.filter(|route| !route.is_closed()),
                        context.as_ref(),
                    )
                    .await?;
                // The session's MCP lifecycle events at this turn's boundary
                // (staged operations connecting, restore outcomes) reach the
                // client as `session/event`s, as on the ordinary turn path.
                let (mcp_event_tx, mut mcp_event_rx) = tokio::sync::mpsc::channel::<
                    meerkat_core::EventEnvelope<meerkat_core::event::AgentEvent>,
                >(crate::NOTIFICATION_CHANNEL_CAPACITY);
                let mcp_sink = sink.clone();
                let mcp_session = session_id.clone();
                tokio::spawn(async move {
                    while let Some(event) = mcp_event_rx.recv().await {
                        mcp_sink.emit_event(&mcp_session, &event).await;
                    }
                });
                let run = runtime
                    .start_governed_turn(
                        &session_id,
                        params.prompt,
                        params.injected_context.unwrap_or_default(),
                        connection.clone(),
                        mcp_event_tx,
                        context,
                    )
                    .await?;
                let mut result: crate::handlers::turn::TurnResult = run.into();
                result.session_ref = runtime
                    .realm_id()
                    .map(|realm| meerkat_contracts::format_session_ref(&realm, &result.session_id));
                Ok(RpcResponse::success(id.clone(), result))
            }
            #[cfg(feature = "mcp")]
            "mcp/add" if connection.mcp_commissioned() => {
                let raw = checked_params(params, &["session_id", "server_config", "persisted"])?;
                let params: meerkat_contracts::McpAddParams = serde_json::from_str(raw.get())
                    .map_err(|_| invalid("invalid mcp/add parameters"))?;
                // A configuration write is its own effect under the mutation
                // owner; this profile attaches live only.
                if params.persisted {
                    return Err(crate::session_runtime::runtime_driver_error_to_rpc(
                        unsupported(),
                    ));
                }
                let session_id = meerkat_core::SessionId::parse(&params.session_id)
                    .map_err(|_| invalid("invalid session id"))?;
                let server_name = params.server_config.name.clone();
                let outcome =
                    governed_mcp_add(connection, runtime, &session_id, params.server_config)
                        .await?;
                Ok(RpcResponse::success(
                    id.clone(),
                    meerkat::surface::mcp_live_response(
                        params.session_id,
                        meerkat_contracts::McpLiveOperation::Add,
                        Some(server_name),
                        outcome.persisted,
                    ),
                ))
            }
            crate::router::CONSENT_DECIDE_METHOD if connection.consent.is_some() => {
                let raw = checked_params(params, &["approval_id", "decision"])?;
                let decide: ConsentDecideRequest = serde_json::from_str(raw.get())
                    .map_err(|_| invalid("invalid consent decision parameters"))?;
                // The connection names the approver, its assurance and its
                // scope: this pipe's operator. The request names only the
                // approval and the decision.
                let approver = connection
                    .consent
                    .as_ref()
                    .map(|consent| consent.approver())
                    .ok_or_else(|| crate::session_runtime::runtime_driver_error_to_rpc(unsupported()))?
                    .map_err(|_| crate::session_runtime::runtime_driver_error_to_rpc(unsupported()))?;
                let event = meerkat_core::approval::consent::AuthenticatedConsentEvent::new(
                    decide.approval_id,
                    meerkat_core::approval::consent::AuthenticatedConsentDecision {
                        approver,
                        decision: decide.decision,
                        reason: None,
                    },
                );
                Ok(crate::handlers::approval::handle_consent_decide(
                    id.clone(),
                    Some(raw),
                    runtime.clone(),
                    event,
                ))
            }
            // The pipe's operator for durable decisions: a list presents the
            // listed page's decisions it acts for before it answers, and a
            // decide decides as the operator, in the pipe's scope.
            #[cfg(unix)]
            "decision/list" if connection.serves_decisions() => {
                let operator = connection.decision_operator()?;
                Ok(operator.handle_list(id.clone(), params, runtime).await)
            }
            #[cfg(unix)]
            crate::router::DECISION_DECIDE_METHOD if connection.serves_decisions() => {
                let operator = connection.decision_operator()?;
                Ok(operator
                    .handle_decide(id.clone(), params, runtime.clone())
                    .await)
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

/// Stage one live MCP add for `session_id` under the attachment control
/// [`governed_mcp_attachment_control`] prepares for this connection, before
/// anything is discovered, spawned or staged.
#[cfg(feature = "mcp")]
async fn governed_mcp_add(
    connection: &GovernedConnection,
    runtime: &Arc<SessionRuntime>,
    session_id: &meerkat_core::SessionId,
    server: meerkat_core::McpServerConfig,
) -> Result<meerkat_core::mcp_attachment::McpAttachmentOutcome, RpcError> {
    // The attachment facts carry no represented subject; a delegated
    // connection cannot be narrowed to them.
    if connection.represented_subject.is_some() {
        return Err(crate::session_runtime::runtime_driver_error_to_rpc(
            unsupported(),
        ));
    }
    let authority =
        governed_mcp_attachment_control(connection, runtime, session_id, server.clone())
            .await
            .map_err(|refusal| RpcError {
                code: error::INVALID_REQUEST,
                message: meerkat::surface::McpLiveAddError::Refused(refusal).to_string(),
                data: None,
            })?;
    runtime
        .mcp_stage_add_governed(session_id, server, false, authority)
        .await
}

/// The native attachment control of `server` for `session_id`, prepared now
/// for this connection: built from the connection's retained facts and
/// evidence, the exact target it bound the session to and the session's
/// current registration, and decided by the native owners (the registration's
/// bound agent and its selection for the server, the installation, the
/// account and current grants) at preparation and again at every effect
/// entry. A live `mcp/add` and a resumed session's attachment restore use this
/// same control; nothing is saved between them, so each restore decides
/// afresh. Typed and local: a session this connection did not bind, a
/// delegated connection or a registration that is not current is refused.
#[cfg(feature = "mcp")]
pub(crate) async fn governed_mcp_attachment_control(
    connection: &GovernedConnection,
    runtime: &Arc<SessionRuntime>,
    session_id: &meerkat_core::SessionId,
    server: meerkat_core::McpServerConfig,
) -> Result<
    Arc<dyn meerkat_core::mcp_attachment::McpAttachmentAuthority>,
    meerkat_core::mcp_attachment::McpAttachmentRefusal,
> {
    use meerkat_core::mcp_attachment::McpAttachmentRefusal;
    use meerkat_core::mcp_attachment_control::{McpAttachmentControlRequest, McpAttachmentFacts};
    // The attachment facts carry no represented subject.
    if connection.represented_subject.is_some() {
        return Err(McpAttachmentRefusal::Mismatch);
    }
    // A descriptor the host's composition cannot execute is refused before
    // any owner decision or effect.
    if connection
        .binder
        .as_ref()
        .is_some_and(|binder| !binder.supports_attachment_descriptor(&server))
    {
        return Err(McpAttachmentRefusal::Unavailable);
    }
    // Only a session this connection created and bound has a destination.
    let target = connection
        .bound_target(session_id)
        .ok_or(McpAttachmentRefusal::NotCurrent)?;
    let machine = runtime.runtime_adapter();
    let registration = machine
        .current_session_registration_witness(session_id)
        .await
        .ok_or(McpAttachmentRefusal::NotCurrent)?;
    let control = McpAttachmentControlRequest::from_trusted_ingress(
        session_id.clone(),
        server,
        McpAttachmentFacts {
            requester: connection.requester.clone(),
            actor: connection.ingress_actor.clone(),
            realm: connection.realm.clone(),
            destination: meerkat_runtime::meerkat_machine::scoped_destination_target(&target),
        },
        Arc::new(GovernedJsonlAttachmentReceipt {
            requester: connection.requester.clone(),
            ingress_actor: connection.ingress_actor.clone(),
            realm: connection.realm.clone(),
            authentication: connection.authentication.clone(),
            session_id: session_id.clone(),
            target: target.clone(),
        }),
    )
    .map_err(|_| McpAttachmentRefusal::Mismatch)?;
    machine
        .prepare_mcp_attachment(&registration, &target, control)
        .await
}

#[path = "governed_jsonl/stock.rs"]
pub mod stock;

#[path = "governed_jsonl/stock_consent.rs"]
pub mod stock_consent;

/// The one decision a pipe's operator makes on a consent approval.
#[derive(serde::Deserialize)]
struct ConsentDecideRequest {
    approval_id: meerkat_core::ApprovalId,
    decision: meerkat_core::ApprovalDecision,
}

#[cfg(test)]
#[path = "governed_jsonl/tests.rs"]
mod tests;
