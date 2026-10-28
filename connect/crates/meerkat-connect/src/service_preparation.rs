//! Attach the supplied service tools only when the original agent asks for them.
//!
//! The native router owns connection state. This module retains only one local
//! handoff and its exact composition; it is not a readiness or approval owner.

use std::{
    future::Future,
    io::{self, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use meerkat::{McpRouter, McpRouterAdapter};
use meerkat_core::{
    AgentToolDispatcher, ResolvedStorage, ToolDef, ToolError, ToolMutationClass, ToolResult,
    gateway::DynamicToolComposite, ops::ToolDispatchOutcome, types::ToolCallView,
};
use tokio::sync::{mpsc, oneshot};

use crate::{
    ConnectError, SessionOptions, paired_host_tool::PairedHostFrontend,
    resources::InstalledResources, setup::PreparedSetupConnection,
};

pub const TOOLKIT_PREPARE_SERVICES: &str = "toolkit_prepare_services";

pub(crate) mod native;
pub use native::NativeSetup;

pub(crate) type TerminalToolFactory = Arc<
    dyn Fn(Arc<crate::service_install::CheckedInstalls>) -> Arc<dyn AgentToolDispatcher>
        + Send
        + Sync,
>;

enum HandoffState {
    Unrequested,
    Pending(Arc<()>),
    Accepted,
    Retired,
}

pub(crate) struct ServicePreparationOwner {
    adapter: Arc<McpRouterAdapter>,
    installs: Option<Arc<crate::service_install::CheckedInstalls>>,
    acquisition: bool,
    terminal_connectors: bool,
    state: Mutex<HandoffState>,
    requests: mpsc::Sender<ServicePreparationRequest>,
    native_adapter_owner: bool,
}

impl ServicePreparationOwner {
    pub(crate) fn allows_terminal_connectors(&self) -> bool {
        self.terminal_connectors
    }

    pub(crate) fn allows_acquisition(&self) -> bool {
        self.acquisition
    }

    pub(crate) fn requires_shutdown(&self) -> bool {
        self.state
            .lock()
            .map_or(true, |state| matches!(*state, HandoffState::Retired))
    }

    pub(crate) fn accepted(&self) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| matches!(*state, HandoffState::Accepted))
    }

    fn is_live(&self, token: &Arc<()>) -> bool {
        self.state.lock().is_ok_and(
            |state| matches!(&*state, HandoffState::Pending(active) if Arc::ptr_eq(active, token)),
        )
    }

    fn retire_request(&self, token: &Arc<()>) {
        if let Ok(mut state) = self.state.lock()
            && matches!(&*state, HandoffState::Pending(active) if Arc::ptr_eq(active, token))
        {
            *state = HandoffState::Retired;
        }
    }

    pub(crate) fn retire(&self) {
        if let Some(installs) = &self.installs {
            installs.retire();
        }
        if let Ok(mut state) = self.state.lock() {
            *state = HandoffState::Retired;
        }
    }

    pub(crate) async fn shutdown(&self) {
        self.retire();
        // Legacy lazy setup created this adapter and must join it explicitly.
        // Standalone setup uses the factory's adapter and its native shutdown
        // owner; both paths retire the same Toolkit admission above.
        if !self.native_adapter_owner {
            self.adapter.shutdown().await;
        }
    }

    async fn request(
        self: &Arc<Self>,
        call: ToolCallView<'_>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.args.get().len() > 1024
            || !call
                .parse_args::<serde_json::Map<String, serde_json::Value>>()
                .is_ok_and(|arguments| arguments.is_empty())
        {
            return Err(ToolError::execution_failed(
                "Service preparation takes only an empty object; no command, path, server or account can be supplied",
            ));
        }
        let token = Arc::new(());
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| ToolError::access_denied(call.name))?;
            match &*state {
                HandoffState::Accepted => {
                    return Ok(ToolResult::new(
                        call.id.into(),
                        r#"{"state":"requested","changed":false}"#.into(),
                        false,
                    )
                    .into());
                }
                HandoffState::Unrequested => *state = HandoffState::Pending(token.clone()),
                _ => return Err(ToolError::access_denied(call.name)),
            }
        }
        let lease = RequestLease {
            owner: self.clone(),
            token: token.clone(),
        };
        let (send, receive) = oneshot::channel();
        self.requests
            .try_send(ServicePreparationRequest {
                owner: self.clone(),
                token,
                response: Some(send),
            })
            .map_err(|_| {
                ToolError::execution_failed(
                    "The service preparation owner is unavailable; stop without retrying",
                )
            })?;
        let (connection, acknowledged) = receive.await.map_err(|_| {
            ToolError::execution_failed(
                "Service preparation ended without a confirmed handoff; stop without retrying",
            )
        })?;
        if !self.is_live(&lease.token) {
            return Err(ToolError::access_denied(call.name));
        }
        self.adapter.stage_add(connection.into_server()).await
            .map_err(|_| ToolError::execution_failed("The native service add could not be confirmed; stop without retrying and close this setup session"))?;
        if !self.is_live(&lease.token) {
            return Err(ToolError::access_denied(call.name));
        }
        // The generated owner permits ApplyBoundaryAddRunning. Only this one
        // retained server is staged; ordinary native polling publishes its facts.
        let applied = self.adapter.apply_staged().await
            .map_err(|_| ToolError::execution_failed("The native service boundary could not be confirmed; stop without retrying and close this setup session"))?;
        if !applied.delta.rejected_boundaries.is_empty() {
            return Err(ToolError::execution_failed(
                "The native owner refused the service boundary; stop without retrying and close this setup session",
            ));
        }
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| ToolError::access_denied(call.name))?;
            if !matches!(&*state, HandoffState::Pending(active) if Arc::ptr_eq(active, &lease.token))
            {
                return Err(ToolError::access_denied(call.name));
            }
            // No await separates successful acknowledgment from publication.
            // A dropped presenter cannot turn a queued configuration into usable
            // local tool authority. Native connection facts remain independent.
            if acknowledged.send(()).is_err() {
                *state = HandoffState::Retired;
                return Err(ToolError::access_denied(call.name));
            }
            *state = HandoffState::Accepted;
        }
        Ok(ToolResult::new(
            call.id.into(),
            serde_json::json!({
                "state":"requested", "changed":true, "native_pending_sources":applied.pending_count,
            })
            .to_string(),
            false,
        )
        .into())
    }
}

struct RequestLease {
    owner: Arc<ServicePreparationOwner>,
    token: Arc<()>,
}
impl Drop for RequestLease {
    fn drop(&mut self) {
        self.owner.retire_request(&self.token);
    }
}

struct PreparationTool(Arc<ServicePreparationOwner>);
#[async_trait]
impl AgentToolDispatcher for PreparationTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(TOOLKIT_PREPARE_SERVICES,
            "Request Connect's supplied runtime preparation tools in this same session when concrete preparation or help is needed. Read public recipe metadata first without this preparation. Supply only {}. The local owner verifies and prepares the installed resources, then the native router connects them. Requested means only that the native add was accepted, not readiness or installation. Use native catalog search/load and the resulting tools for observations. For an outside-catalog Python package, research and choose its exact distribution/version, call setup_acquire_python, await its settled result, and inspect the returned receipt with setup_inspect_acquired. Acquisition downloads inert wheels without running package code. When the supplied resources also enable npm, use setup_search_node for literal public research, choose an exact package/version, then call setup_acquire_node and inspect its settled receipt with setup_inspect_acquired. npm acquisition retains inert archives, not an installed or authorized service. If setup_acquire_node_prebuilt is supplied, it can retain explicitly selected digest-pinned public native payloads for the advertised host target and an exact current Node acquisition receipt. It does not run package scripts or authorize installation. If these tools are unavailable, report the supplied-resource limit; do not substitute runtimes or invent an installation route. Author exact package/activation metadata from the guide and evidence. If checked installation is enabled in this composition, setup_selected_check and setup_selected_install retain its separate exact approval. Otherwise report the installation limit. For a reconciled installed public service, toolkit_enable_public_service reviews exact raw operations for the chosen agent and persists access after this conversation closes. Library profiles require current setup_help explicitly declaring authentication kind none. Outside-catalog Python/Node selections require the manifest service export authentication kind none and portable_mcp_entries operation_tools mapping from every selected operation ID to its actual raw MCP name. The selected mapping is a declaration from inspected source evidence, not observed tools/list or readiness. Read setup_help again after setup_check so the current publisher metadata is retained. This action requires no account or sign-in. Account sign-in and attachment require their own supported owners. If this request is interrupted, fails or expires, stop without retrying; partial files or a pending native connection may remain until session cleanup.",
            serde_json::json!({"type":"object","additionalProperties":false,"properties":{}}),
        ))].into()
    }
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        // This closed inline leaf's default catalog is its complete current
        // registry. Native composition may then retain the MCP control plane.
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if name == TOOLKIT_PREPARE_SERVICES {
            ToolMutationClass::Mutating
        } else {
            ToolMutationClass::Unknown
        }
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != TOOLKIT_PREPARE_SERVICES {
            return Err(ToolError::not_found(call.name));
        }
        self.0.request(call).await
    }
}

/// One exact fresh-session composition. It cannot be cloned or opened twice.
/// The original native session binds the empty adapter's canonical owners.
pub struct LazySetup {
    paired_host: PairedHostFrontend,
    forge: crate::forge::ForgeFrontend,
    forge_presenter: Mutex<Option<crate::forge::ForgePresenter>>,
    dispatcher: Arc<dyn AgentToolDispatcher>,
    preparation_tools: Option<Vec<String>>,
    owner: Arc<ServicePreparationOwner>,
    claimed: AtomicBool,
}

impl LazySetup {
    pub fn new(paired_host: PairedHostFrontend) -> (Self, ServicePreparationPresenter) {
        Self::compose(paired_host, true)
    }

    /// Terminal setup uses the same checked-install owner for the software
    /// dispatcher and protected native connector presenter.
    pub fn terminal_services(
        paired_host: PairedHostFrontend,
        connectors: &crate::terminal_connector::TerminalConnectors,
    ) -> (Self, ServicePreparationPresenter) {
        Self::compose_selected_with_owner(
            paired_host,
            true,
            true,
            true,
            false,
            true,
            connectors.router(),
            None,
            Some(connectors.dispatcher_factory()),
            None,
        )
    }

    /// The initial private-pipe frontend has no service or paired host presenter.
    /// Omit those dispatchers rather than advertising an unanswerable prompt.
    pub(crate) fn reading() -> Self {
        let (paired_host, host_presenter) = PairedHostFrontend::deferred_with_research();
        drop(host_presenter);
        let (setup, presenter) = Self::compose(paired_host, false);
        drop(presenter);
        setup
    }

    /// Generic private frontend caller. The host supplies these exact loaded
    /// application facts; no inventory or model argument selects this mode.
    /// Installed gateways must compose their existing runtime/presenter instead
    /// of opening a standalone LazySetup session through this constructor.
    pub(crate) fn private_forge_application(
        expected: crate::forge::ForgeHost,
        private_fields: std::collections::BTreeSet<String>,
    ) -> Result<Self, crate::forge::ForgeError> {
        let forge = crate::forge::ForgeFrontend::for_application(expected, private_fields)?;
        let (paired_host, hosts) = PairedHostFrontend::deferred_with_research();
        drop(hosts);
        let (setup, presenter) = Self::compose_selected_with_owner(
            paired_host,
            false,
            false,
            true,
            false,
            false,
            McpRouter::new_with_stdio_profile(meerkat::McpStdioLaunchProfile::trusted_host()),
            None,
            None,
            Some(forge),
        );
        drop(presenter);
        Ok(setup)
    }

    /// Private-pipe frontend composition. The service worker ceiling is not
    /// enabled by adapting a frontend presenter.
    pub(crate) fn private_frontends(
        paired_host: PairedHostFrontend,
        include_paired_host: bool,
        include_forge: bool,
    ) -> Self {
        let (setup, presenter) = Self::compose_selected(
            paired_host,
            include_paired_host,
            false,
            include_forge,
            false,
            false,
            McpRouter::new_with_stdio_profile(meerkat::McpStdioLaunchProfile::trusted_host()),
        );
        drop(presenter);
        setup
    }

    /// Exact host-owned paired host preparation and compiled recipe readers.
    /// No service adapter, arbitrary document fetch, research or Forge tool is
    /// installed. The captured definitions also own the native access ceiling.
    pub(crate) fn paired_host_preparation(paired_host: PairedHostFrontend) -> Self {
        let (setup, presenter) = Self::compose_selected(
            paired_host,
            true,
            false,
            false,
            true,
            false,
            McpRouter::new_with_stdio_profile(meerkat::McpStdioLaunchProfile::trusted_host()),
        );
        drop(presenter);
        setup
    }

    /// Separate service preparation composition. The caller must retain the
    /// returned worker and qualify its native MCP shutdown before admitting it.
    /// Checked installation uses the existing native decision owner. Account
    /// authentication, usable attachment and full service availability remain
    /// separate; the private-pipe service mode is still unavailable.
    #[cfg(test)]
    pub(crate) fn private_services() -> (Self, ServicePreparationPresenter) {
        Self::private_services_with_router(McpRouter::new_with_stdio_profile(
            meerkat::McpStdioLaunchProfile::trusted_host(),
        ))
    }

    /// Retain the caller's fresh native router, configured before session
    /// binding, through the existing adapter and preparation owner.
    pub(crate) fn private_services_with_router(
        router: McpRouter,
    ) -> (Self, ServicePreparationPresenter) {
        let (paired_host, hosts) = PairedHostFrontend::deferred_with_research();
        drop(hosts);
        Self::compose_selected(paired_host, false, true, false, false, true, router)
    }

    #[cfg(test)]
    pub(crate) fn fixture_services_with_router(
        router: McpRouter,
    ) -> (Self, ServicePreparationPresenter) {
        // Connector controls retain the same service owners and router while
        // using TestClient. Optional research requires a native model client.
        let (paired_host, hosts) = PairedHostFrontend::deferred();
        drop(hosts);
        Self::compose_selected(paired_host, false, true, false, false, true, router)
    }

    fn compose(
        paired_host: PairedHostFrontend,
        complete: bool,
    ) -> (Self, ServicePreparationPresenter) {
        Self::compose_selected(
            paired_host,
            complete,
            complete,
            true,
            false,
            false,
            McpRouter::new_with_stdio_profile(meerkat::McpStdioLaunchProfile::trusted_host()),
        )
    }

    fn compose_selected(
        paired_host: PairedHostFrontend,
        include_paired_host: bool,
        services: bool,
        include_forge: bool,
        preparation_only: bool,
        checked_installs: bool,
        router: McpRouter,
    ) -> (Self, ServicePreparationPresenter) {
        Self::compose_selected_with_owner(
            paired_host,
            include_paired_host,
            services,
            include_forge,
            preparation_only,
            checked_installs,
            router,
            None,
            None,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn fixture_services(
        installs: Arc<crate::service_install::CheckedInstalls>,
    ) -> (Self, ServicePreparationPresenter) {
        let (paired_host, hosts) = PairedHostFrontend::deferred_with_research();
        drop(hosts);
        Self::compose_selected_with_owner(
            paired_host,
            false,
            true,
            false,
            false,
            true,
            McpRouter::new_with_stdio_profile(meerkat::McpStdioLaunchProfile::trusted_host()),
            Some(installs),
            None,
            None,
        )
    }

    fn compose_selected_with_owner(
        paired_host: PairedHostFrontend,
        include_paired_host: bool,
        services: bool,
        include_forge: bool,
        preparation_only: bool,
        checked_installs: bool,
        router: McpRouter,
        fixture_installs: Option<Arc<crate::service_install::CheckedInstalls>>,
        terminal_connectors: Option<TerminalToolFactory>,
        selected_forge: Option<(crate::forge::ForgeFrontend, crate::forge::ForgePresenter)>,
    ) -> (Self, ServicePreparationPresenter) {
        Self::compose_with_adapter(
            paired_host,
            include_paired_host,
            services,
            include_forge,
            preparation_only,
            checked_installs,
            Arc::new(McpRouterAdapter::new(router)),
            fixture_installs,
            terminal_connectors,
            selected_forge,
            false,
        )
    }

    fn compose_with_adapter(
        paired_host: PairedHostFrontend,
        include_paired_host: bool,
        services: bool,
        include_forge: bool,
        preparation_only: bool,
        checked_installs: bool,
        adapter: Arc<McpRouterAdapter>,
        retained_installs: Option<Arc<crate::service_install::CheckedInstalls>>,
        terminal_connectors: Option<TerminalToolFactory>,
        selected_forge: Option<(crate::forge::ForgeFrontend, crate::forge::ForgePresenter)>,
        native_adapter_owner: bool,
    ) -> (Self, ServicePreparationPresenter) {
        let (forge, forge_presenter) =
            selected_forge.unwrap_or_else(crate::forge::ForgeFrontend::new);
        forge.set_available(false);
        let installs = retained_installs.or_else(|| {
            checked_installs.then(|| crate::service_install::CheckedInstalls::new(adapter.clone()))
        });
        let (requests, receive) = mpsc::channel(1);
        let owner = Arc::new(ServicePreparationOwner {
            adapter: adapter.clone(),
            installs: installs.clone(),
            acquisition: services,
            terminal_connectors: terminal_connectors.is_some(),
            state: Mutex::new(if native_adapter_owner {
                HandoffState::Accepted
            } else {
                HandoffState::Unrequested
            }),
            requests,
            native_adapter_owner,
        });
        let mut members: Vec<Arc<dyn AgentToolDispatcher>> =
            vec![Arc::new(crate::recipe_discovery::RecipeTools::new())];
        if !preparation_only {
            members.push(Arc::new(crate::public_document::PublicDocumentTools::new()));
        }
        if include_forge {
            members.insert(0, forge.dispatcher());
        }
        if include_paired_host {
            members.insert(0, paired_host.dispatcher());
        }
        if services {
            members.push(Arc::new(PreparationTool(owner.clone())));
        }
        if let (Some(terminal), Some(installs)) = (terminal_connectors, installs.as_ref()) {
            members.push(terminal(installs.clone()));
        }
        if !preparation_only {
            match installs {
                Some(installs) => members.push(installs),
                None => members.push(adapter),
            }
        }
        let dispatcher = Arc::new(DynamicToolComposite::new(members));
        let preparation_tools = preparation_only.then(|| {
            dispatcher
                .tools()
                .iter()
                .map(|tool| tool.name.to_string())
                .collect()
        });
        (
            Self {
                paired_host,
                forge,
                forge_presenter: Mutex::new(Some(forge_presenter)),
                dispatcher,
                preparation_tools,
                owner: owner.clone(),
                claimed: AtomicBool::new(false),
            },
            ServicePreparationPresenter {
                requests: receive,
                owner,
            },
        )
    }

    pub fn session_options(&self, options: SessionOptions) -> Result<SessionOptions, ConnectError> {
        crate::setup::lazy_setup_session(options, self)
    }

    /// Transfer the single protected receiver to the existing CLI presenter.
    pub fn take_forge_presenter(&self) -> Result<crate::forge::ForgePresenter, ConnectError> {
        let presenter = self
            .forge_presenter
            .lock()
            .map_err(|_| ConnectError::InvalidInput("Forge presenter is unavailable".into()))?
            .take()
            .ok_or_else(|| {
                ConnectError::InvalidInput("Forge presenter was already taken".into())
            })?;
        self.forge.set_available(true);
        Ok(presenter)
    }

    /// Bind nonsecret recovery locators to the already reserved realm artifacts.
    /// This does not restore any host session or submission approval.
    pub fn set_forge_artifacts(&self, directory: &std::path::Path) -> Result<(), ConnectError> {
        self.forge.artifacts(directory)
    }
    pub(crate) fn forge(&self) -> &crate::forge::ForgeFrontend {
        &self.forge
    }

    pub(crate) fn paired_host(&self) -> &PairedHostFrontend {
        &self.paired_host
    }
    pub(crate) fn dispatcher(&self) -> Arc<dyn AgentToolDispatcher> {
        self.dispatcher.clone()
    }
    pub(crate) fn declared_tools(&self) -> Vec<String> {
        self.preparation_tools.clone().unwrap_or_else(|| {
            self.dispatcher
                .tools()
                .iter()
                .map(|tool| tool.name.to_string())
                .collect()
        })
    }
    pub(crate) fn allowed_tools(&self) -> Vec<String> {
        if let Some(tools) = &self.preparation_tools {
            return tools.clone();
        }
        let mut allowed = self.paired_host.allowed_tools();
        allowed.extend(crate::recipe_discovery::RECIPE_TOOLS.iter().copied());
        allowed.push(crate::public_document::READ_PUBLIC_DOCUMENT);
        allowed.push(TOOLKIT_PREPARE_SERVICES);
        allowed.extend(crate::forge::FORGE_TOOLS.iter().copied());
        allowed.extend(crate::setup::PREPARATION_TOOLS.iter().copied());
        if self.owner.allows_acquisition() {
            allowed.extend(crate::setup::ACQUISITION_TOOLS.iter().copied());
        }
        if self.owner.installs.is_some() {
            allowed.extend(crate::service_install::INSTALL_TOOLS.iter().copied());
            allowed.push(crate::service_install::SELECTED_CHECK);
        }
        if self.owner.allows_terminal_connectors() {
            allowed.extend(crate::terminal_connector::TOOLS.iter().copied());
        }
        allowed.extend([
            meerkat_core::tool_catalog::TOOL_CATALOG_SEARCH_NAME,
            meerkat_core::tool_catalog::TOOL_CATALOG_LOAD_NAME,
        ]);
        allowed.into_iter().map(str::to_owned).collect()
    }
    pub(crate) fn matches(&self, dispatcher: &Arc<dyn AgentToolDispatcher>) -> bool {
        Arc::ptr_eq(&self.dispatcher, dispatcher)
    }
    pub(crate) fn owner(&self) -> Arc<ServicePreparationOwner> {
        self.owner.clone()
    }
    pub(crate) fn installs(&self) -> Option<Arc<crate::service_install::CheckedInstalls>> {
        self.owner.installs.clone()
    }
    pub(crate) fn claim(&self) -> Result<(), ConnectError> {
        self.claimed.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ()).map_err(|_| ConnectError::InvalidInput("This lazy setup composition has already been used; it cannot bind another native session".into()))
    }
}

/// Sole receiver for the CLI or an embedding host's resource preparation owner.
pub struct ServicePreparationPresenter {
    requests: mpsc::Receiver<ServicePreparationRequest>,
    owner: Arc<ServicePreparationOwner>,
}
impl ServicePreparationPresenter {
    pub async fn next(&mut self) -> Option<ServicePreparationRequest> {
        self.requests.recv().await
    }

    /// A lost request requires closing this invocation, even if a queued prompt
    /// was never presented. This is not a native connection/readiness status.
    pub fn requires_shutdown(&self) -> bool {
        self.owner.requires_shutdown()
    }
}

/// One live request. Dropping it retires the pending handoff without retry.
pub struct ServicePreparationRequest {
    owner: Arc<ServicePreparationOwner>,
    token: Arc<()>,
    response: Option<oneshot::Sender<(PreparedSetupConnection, oneshot::Sender<()>)>>,
}
impl ServicePreparationRequest {
    /// Wait for the exact native tool waiter to end. An embedding preparer
    /// should feed this into its owned cancellation-and-join path, rather than
    /// dropping a running child/preparation future. Closure does not undo
    /// completed preparation or native connection effects.
    pub async fn closed(&mut self) {
        if let Some(response) = self.response.as_mut() {
            response.closed().await;
        }
        self.owner.retire_request(&self.token);
    }

    async fn handoff(&mut self, connection: PreparedSetupConnection) -> Result<(), ConnectError> {
        let (acknowledged, receive) = oneshot::channel();
        self.response
            .take()
            .ok_or_else(ended)?
            .send((connection, acknowledged))
            .map_err(|_| ended())?;
        receive.await.map_err(|_| ended())
    }

    /// Prepare this exact request using already verified product resources.
    /// The terminal and private-pipe owners share the same cancellation, child
    /// join and native handoff. The caller must drive this future to completion.
    pub(crate) async fn prepare_installed(
        mut self,
        resources: InstalledResources,
        storage: &ResolvedStorage,
        interrupted: impl Future<Output = io::Result<()>>,
    ) -> Result<bool, ConnectError> {
        if !self.owner.is_live(&self.token) {
            return Err(ended());
        }
        tokio::pin!(interrupted);
        let prepared = {
            let cancelled = async {
                tokio::select! {
                    biased;
                    _ = self.closed() => Ok(()),
                    signal = &mut interrupted => signal,
                }
            };
            resources.prepare(storage, cancelled).await?
        };
        tokio::select! {
            biased;
            signal = &mut interrupted => {
                signal.map_err(|error| ConnectError::native("interrupt service preparation", error))?;
                Ok(false)
            },
            result = self.handoff(prepared.connection) => result.map(|()| true),
        }
    }

    /// Trusted Native Rust embedding counterpart of the installed-resource
    /// presenter. The host must supply output from its selected verified
    /// preparer. Typed parsing alone establishes no provenance. Never accept
    /// model-selected configuration here. On failure, cancel/join the exact
    /// native input and close the session; do not retry or infer rollback.
    pub async fn attach_prepared(
        mut self,
        connection: PreparedSetupConnection,
    ) -> Result<(), ConnectError> {
        self.handoff(connection).await
    }
}
impl Drop for ServicePreparationRequest {
    fn drop(&mut self) {
        self.owner.retire_request(&self.token);
    }
}

fn ended() -> ConnectError {
    ConnectError::InvalidInput("The service preparation handoff is unconfirmed. Stop this setup session without retrying; preparation files and native connection effects are not claimed rolled back.".into())
}

/// Own verified preparation outside the disposable native tool future. Waiter
/// closure and interruption feed the existing preparer's SIGTERM-and-join path.
/// Do not cancel/drop this presenter future instead of driving that cleanup.
/// On false or error, cancel/join the input and close its native session.
pub async fn present_service_preparation(
    request: ServicePreparationRequest,
    storage: &ResolvedStorage,
    output: &mut impl Write,
    interrupted: impl Future<Output = io::Result<()>>,
) -> Result<bool, ConnectError> {
    if !request.owner.is_live(&request.token) {
        return Err(ended());
    }
    writeln!(
        output,
        "Preparing Connect's supplied service library for this conversation."
    )
    .map_err(|error| ConnectError::native("write service preparation", error))?;
    let resources = InstalledResources::locate()?;
    request
        .prepare_installed(resources, storage, interrupted)
        .await
}
