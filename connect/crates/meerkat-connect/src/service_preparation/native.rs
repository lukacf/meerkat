//! The standalone session's tools over its factory-created native adapter.

use std::{collections::BTreeSet, sync::{Arc, Mutex, OnceLock}};

use async_trait::async_trait;
use meerkat::{AgentFactory, McpAdapterComposeRefusal, McpAdapterComposer, McpRouterAdapter};
use meerkat_core::{
    AgentToolDispatcher, LiveBridgeEffectKind, LiveBridgeEffectOutcome, McpServerConfig,
    MobMemberBinding, RealmLocator, SessionId, ToolDispatchAdmission, ToolError,
    agent::ToolDispatchContext, tool_bundle::RegisteredToolBundleDispatcher, types::ToolCallView,
};

use super::{LazySetup, ServicePreparationPresenter, TerminalToolFactory, TOOLKIT_PREPARE_SERVICES};
use crate::{ConnectError, SessionOptions, approval::LocalApprovals,
    paired_host_tool::PairedHostFrontend, terminal_connector::TerminalConnectors};

pub(crate) const STANDALONE_BUNDLE: &str = "meerkat-connect-setup";

struct SelectedTools {
    paired_host: PairedHostFrontend,
    forge: (crate::forge::ForgeFrontend, crate::forge::ForgePresenter),
    terminal: TerminalToolFactory,
}

/// One native construction's handoff to the existing local setup owners.
/// It does not grant tools or own adapter shutdown. The native factory calls
/// this composer once; failed construction cannot rebind it to another actor.
pub struct NativeSetup {
    session: SessionId,
    realm: RealmLocator,
    selected: Mutex<Option<SelectedTools>>,
    published: Mutex<Option<(LazySetup, ServicePreparationPresenter)>>,
    approvals: Arc<LocalApprovals>,
    admission: Arc<PreparedAdmission>,
    paired_host: PairedHostFrontend,
    operations: BTreeSet<String>,
}

impl NativeSetup {
    /// Keep the existing Reading recipe. Embedding applications with their own
    /// loaded Forge binding use [`Self::with_forge`].
    pub fn new(
        session: SessionId, realm: RealmLocator, paired_host: PairedHostFrontend,
        connectors: &TerminalConnectors, approvals: Arc<LocalApprovals>,
    ) -> Result<Arc<Self>, ConnectError> {
        Self::with_forge(session, realm, paired_host, connectors, approvals,
            crate::forge::ForgeFrontend::new())
    }

    /// Use the application's existing Forge binding in this native setup.
    /// Construct the pair with [`crate::forge::ForgeFrontend::for_application`]
    /// from the exact loaded host and its declared private fields. The pair is
    /// moved here together. The binding must still be fresh and never handed
    /// off to another native setup; crossed, used or previously claimed pairs
    /// are refused without resetting their selection or login state.
    ///
    /// Pass this setup to [`crate::SessionStartup::open_native_setup`] or
    /// [`crate::session::ResumedStandalone::open_native_setup`]. After native
    /// construction, take the sole presenter through [`Self::take_composition`]
    /// and [`LazySetup::take_forge_presenter`]. A later setup needs a fresh pair
    /// from the same trusted application configuration, after the previous
    /// setup has closed. This selects no host origin, login, grant or runtime.
    pub fn with_forge(
        session: SessionId, realm: RealmLocator, paired_host: PairedHostFrontend,
        connectors: &TerminalConnectors, approvals: Arc<LocalApprovals>,
        forge: (crate::forge::ForgeFrontend, crate::forge::ForgePresenter),
    ) -> Result<Arc<Self>, ConnectError> {
        forge.0.claim_native_setup(&forge.1)?;
        let operations = paired_host.allowed_tools().into_iter()
            .chain(crate::recipe_discovery::RECIPE_TOOLS.iter().copied())
            .chain([crate::public_document::READ_PUBLIC_DOCUMENT, TOOLKIT_PREPARE_SERVICES])
            .chain(crate::forge::FORGE_TOOLS.iter().copied())
            .chain(crate::service_install::INSTALL_TOOLS.iter().copied())
            .chain([crate::service_install::SELECTED_CHECK])
            .chain(crate::terminal_connector::TOOLS.iter().copied())
            .map(str::to_owned).collect();
        Ok(Arc::new(Self {
            session, realm, paired_host: paired_host.clone(), operations,
            selected: Mutex::new(Some(SelectedTools {
                paired_host, forge, terminal: connectors.dispatcher_factory(),
            })),
            published: Mutex::new(None), approvals,
            admission: Arc::new(PreparedAdmission(OnceLock::new())),
        }))
    }

    pub(crate) fn session_id(&self) -> &SessionId { &self.session }

    /// The protected controller review names these exact host operations.
    /// MCP and catalog targets remain native independent grant targets.
    pub(crate) fn bundle_operations(&self) -> &BTreeSet<String> { &self.operations }

    pub(crate) fn configure_factory(self: &Arc<Self>, factory: AgentFactory) -> AgentFactory {
        factory.with_mcp_adapter_composer(self.clone())
    }

    pub(crate) fn session_options(
        &self, options: SessionOptions, server: &McpServerConfig,
    ) -> Result<SessionOptions, ConnectError> {
        let allowed: Vec<String> = self.operations.iter().cloned()
            .chain(crate::setup::PREPARATION_TOOLS.iter().map(|name| (*name).to_owned()))
            .chain(crate::setup::ACQUISITION_TOOLS.iter().map(|name| (*name).to_owned()))
            .chain([meerkat_core::tool_catalog::TOOL_CATALOG_SEARCH_NAME.to_owned(),
                meerkat_core::tool_catalog::TOOL_CATALOG_LOAD_NAME.to_owned()]).collect();
        let mut options = crate::setup::compose_paired_host_session(options, &self.paired_host, allowed)?;
        // All tools are returned by the native adapter composer. No second
        // router or inline duplicate of the PairedHost dispatcher is installed.
        options.build.external_tools = None;
        options.build.mcp_servers.push(server.clone());
        options.build.tool_dispatch_admission = Some(self.admission.clone());
        Ok(options)
    }

    pub(crate) fn research(&self) -> Option<crate::research::ResearchBinding> {
        self.paired_host.research()
    }

    pub fn take_composition(&self) -> Result<(LazySetup, ServicePreparationPresenter), ConnectError> {
        self.published.lock().map_err(|_| unavailable())?.take().ok_or_else(unavailable)
    }

    pub(crate) fn owner(&self) -> Result<Arc<super::ServicePreparationOwner>, ConnectError> {
        self.published.lock().map_err(|_| unavailable())?.as_ref()
            .map(|(setup, _)| setup.owner()).ok_or_else(unavailable)
    }

    pub(crate) fn retire(&self) {
        self.approvals.retire();
        if let Ok(published) = self.published.lock()
            && let Some((setup, _)) = published.as_ref()
        {
            setup.owner().retire();
        }
    }
}

impl McpAdapterComposer for NativeSetup {
    fn compose(
        &self, adapter: &Arc<McpRouterAdapter>, session: &SessionId,
        member: Option<&MobMemberBinding>,
    ) -> Result<Arc<dyn AgentToolDispatcher>, McpAdapterComposeRefusal> {
        let refused = || McpAdapterComposeRefusal::new("The exact standalone setup composition is unavailable");
        if session != &self.session || member.is_some() { return Err(refused()); }
        let selected = self.selected.lock().map_err(|_| refused())?.take().ok_or_else(refused)?;
        let installs = crate::service_install::CheckedInstalls::with_checked_route(
            adapter.clone(), session.clone(),
        ).map_err(|_| refused())?;
        let (mut setup, presenter) = LazySetup::compose_with_adapter(
            selected.paired_host, true, true, true, false, true, adapter.clone(), Some(installs),
            Some(selected.terminal), Some(selected.forge), true,
        );
        setup.claim().map_err(|_| refused())?;
        let installs = setup.installs().ok_or_else(refused)?;
        installs.bind_realm(&self.realm).map_err(|_| refused())?;
        self.approvals.check_session(session).map_err(|_| refused())?;
        let admission = self.approvals.native_setup_admission(&setup).map_err(|_| refused())?;
        // This real registration supplies the bundle witness and leaves nested
        // MCP/catalog witnesses intact. It does not widen either grant target.
        setup.dispatcher = Arc::new(RegisteredToolBundleDispatcher::new(
            STANDALONE_BUNDLE, setup.dispatcher(),
        ));
        let dispatcher = setup.dispatcher();
        let mut published = self.published.lock().map_err(|_| refused())?;
        if published.is_some() { return Err(refused()); }
        self.admission.0.set(admission).map_err(|_| refused())?;
        *published = Some((setup, presenter));
        Ok(dispatcher)
    }
}

/// Only forwards to LocalApprovals after the actual adapter/installation
/// composition exists. It introduces no independent decision or permission.
struct PreparedAdmission(OnceLock<Arc<dyn ToolDispatchAdmission>>);

#[async_trait]
impl ToolDispatchAdmission for PreparedAdmission {
    async fn await_dispatch_admission(
        &self, call: ToolCallView<'_>, context: Option<&ToolDispatchContext>, effect: LiveBridgeEffectKind,
    ) -> Result<(), ToolError> {
        self.0.get().ok_or_else(|| ToolError::access_denied(call.name))?
            .await_dispatch_admission(call, context, effect).await
    }

    async fn record_dispatch_outcome(
        &self, call: ToolCallView<'_>, context: Option<&ToolDispatchContext>,
        effect: LiveBridgeEffectKind, outcome: LiveBridgeEffectOutcome,
    ) -> Result<(), ToolError> {
        self.0.get().ok_or_else(|| ToolError::access_denied(call.name))?
            .record_dispatch_outcome(call, context, effect, outcome).await
    }
}

fn unavailable() -> ConnectError {
    ConnectError::InvalidInput("The native setup construction did not retain its exact tool composition".into())
}
