//! One eagerly prepared private helper, retained through the factory's actual
//! adapter construction. This owner grants nothing and cannot be reopened.
use super::{
    LazySetup, ServicePreparationOwner, ServicePreparationPresenter, TOOLKIT_PREPARE_SERVICES,
    TerminalToolFactory, native::PreparedAdmission, private_attempt::PrivateCustody,
};
use crate::{
    ConnectError, SessionOptions, approval::LocalApprovals, paired_host_tool::PairedHostFrontend,
};
use meerkat::{
    AgentFactory, McpAdapterComposeRefusal, McpAdapterComposer, McpAdapterCustody,
    McpAdapterCustodyClosed, McpRouterAdapter,
};
use meerkat_core::{
    AgentToolDispatcher, McpServerConfig, MobMemberBinding, RealmLocator, SessionId,
    tool_bundle::RegisteredToolBundleDispatcher,
};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) const BUNDLE: &str = "meerkat-connect-private-setup";

pub(crate) struct PrivateNativeSetup {
    session: SessionId,
    realm: RealmLocator,
    custody: Arc<PrivateCustody>,
    adapter: Mutex<Option<Arc<McpRouterAdapter>>>,
    terminal: Mutex<Option<TerminalToolFactory>>,
    published: Mutex<Option<(LazySetup, ServicePreparationPresenter)>>,
    approvals: Arc<LocalApprovals>,
    admission: Arc<PreparedAdmission>,
    paired_host: PairedHostFrontend,
    operations: BTreeSet<String>,
    #[cfg(target_os = "linux")]
    selected_input: OnceLock<Arc<crate::protected_parent::selected_input::SelectedInput>>,
    prepared: AtomicBool,
    closed: AtomicBool,
}

impl PrivateNativeSetup {
    pub(crate) fn new(
        session: SessionId,
        realm: RealmLocator,
        approvals: Arc<LocalApprovals>,
    ) -> Arc<Self> {
        let (paired_host, hosts) = PairedHostFrontend::deferred_with_research();
        drop(hosts);
        let operations = crate::recipe_discovery::RECIPE_TOOLS
            .iter()
            .copied()
            .chain([
                crate::public_document::READ_PUBLIC_DOCUMENT,
                TOOLKIT_PREPARE_SERVICES,
            ])
            .chain(crate::service_install::INSTALL_TOOLS.iter().copied())
            .chain([crate::service_install::SELECTED_CHECK])
            .chain(
                crate::terminal_connector::private_service::PRIVATE_TOOLS
                    .iter()
                    .copied(),
            )
            .map(str::to_owned)
            .collect();
        Arc::new(Self {
            custody: PrivateCustody::for_native_construction(session.clone()),
            session,
            realm,
            approvals,
            paired_host,
            operations,
            adapter: Mutex::new(None),
            terminal: Mutex::new(None),
            published: Mutex::new(None),
            admission: Arc::new(PreparedAdmission(OnceLock::new())),
            #[cfg(target_os = "linux")]
            selected_input: OnceLock::new(),
            prepared: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        })
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn bind_selected_input(
        &self,
        input: Arc<crate::protected_parent::selected_input::SelectedInput>,
    ) -> Result<(), ConnectError> {
        if self.prepared.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        self.selected_input.set(input).map_err(|_| unavailable())
    }

    /// The caller drives this entered preparation to completion on retirement.
    /// The returned descriptor and its real local owner are retained together.
    pub(crate) async fn prepare(
        &self,
        resources: crate::resources::InstalledResources,
        storage: &meerkat_core::ResolvedStorage,
        interrupted: impl std::future::Future<Output = std::io::Result<()>>,
    ) -> Result<(McpServerConfig, std::path::PathBuf), ConnectError> {
        if self.realm != storage.locator
            || self.closed.load(Ordering::Acquire)
            || self.prepared.swap(true, Ordering::AcqRel)
        {
            return Err(unavailable());
        }
        tokio::pin!(interrupted);
        let (prepared, owner) = resources
            .prepare_with_local_round_owner(storage, &mut interrupted)
            .await?;
        let mut server = prepared.connection.into_server();
        let operations = crate::setup::PREPARATION_TOOLS
            .iter()
            .chain(crate::setup::ACQUISITION_TOOLS.iter())
            .copied();
        server.access = Some(meerkat_core::McpServerAccess::allowing(operations));
        self.custody.retain(server.clone(), owner)?;
        #[cfg(target_os = "linux")]
        if let Some(input) = self.selected_input.get().cloned() {
            self.custody.stage_selected(input, &mut interrupted).await?;
        }
        server
            .validate_access()
            .map_err(|error| ConnectError::native("Validate private helper ceiling", error))?;
        Ok((server, prepared.artifact_directory))
    }

    pub(crate) fn operations(&self) -> &BTreeSet<String> {
        &self.operations
    }

    pub(crate) fn configure(
        self: &Arc<Self>,
        factory: AgentFactory,
        terminal: TerminalToolFactory,
    ) -> Result<AgentFactory, ConnectError> {
        let mut slot = self.terminal.lock().map_err(|_| unavailable())?;
        if slot.is_some() || self.closed.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        *slot = Some(terminal);
        Ok(factory
            .with_mcp_attempt_guards(self.custody.guards())
            .with_mcp_adapter_custody(self.clone())
            .with_mcp_adapter_composer(self.clone()))
    }

    pub(crate) fn options(
        &self,
        options: SessionOptions,
        server: &McpServerConfig,
    ) -> Result<SessionOptions, ConnectError> {
        let allowed = self
            .operations
            .iter()
            .cloned()
            .chain(
                crate::setup::PREPARATION_TOOLS
                    .iter()
                    .map(|name| (*name).to_owned()),
            )
            .chain(
                crate::setup::ACQUISITION_TOOLS
                    .iter()
                    .map(|name| (*name).to_owned()),
            )
            .chain([
                meerkat_core::tool_catalog::TOOL_CATALOG_SEARCH_NAME.to_owned(),
                meerkat_core::tool_catalog::TOOL_CATALOG_LOAD_NAME.to_owned(),
            ])
            .collect();
        let mut options =
            crate::setup::compose_paired_host_session(options, &self.paired_host, allowed)?;
        options.build.external_tools = None;
        options.build.mcp_servers.push(server.clone());
        options.build.tool_dispatch_admission = Some(self.admission.clone());
        Ok(options)
    }

    pub(crate) fn research(&self) -> Option<crate::research::ResearchBinding> {
        self.paired_host.research()
    }

    pub(crate) fn owner(&self) -> Result<Arc<ServicePreparationOwner>, ConnectError> {
        self.published
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .map(|(lazy, _)| lazy.owner())
            .ok_or_else(unavailable)
    }

    pub(crate) fn take_composition(
        &self,
    ) -> Result<(LazySetup, ServicePreparationPresenter), ConnectError> {
        self.published
            .lock()
            .map_err(|_| unavailable())?
            .take()
            .ok_or_else(unavailable)
    }

    pub(crate) async fn close(self: &Arc<Self>) -> Result<(), ConnectError> {
        self.closed.store(true, Ordering::Release);
        self.approvals.retire();
        if let Ok(published) = self.published.lock()
            && let Some((lazy, _)) = published.as_ref()
        {
            lazy.owner().retire();
        }
        let adapter = self
            .adapter
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        let outcome = match adapter.as_ref() {
            Some(adapter) => self.custody.close(adapter).await,
            None => self.custody.close_unopened().await,
        };
        outcome.map_err(|()| {
            ConnectError::native("Retain failed private native setup", Retained(self.clone()))
        })
    }
}

impl McpAdapterCustody for PrivateNativeSetup {
    fn adopt(&self, adapter: &Arc<McpRouterAdapter>) -> Result<(), McpAdapterCustodyClosed> {
        let mut slot = self.adapter.lock().map_err(|_| McpAdapterCustodyClosed)?;
        if self.closed.load(Ordering::Acquire) || slot.is_some() {
            return Err(McpAdapterCustodyClosed);
        }
        self.custody
            .bind_construction(adapter)
            .map_err(|_| McpAdapterCustodyClosed)?;
        *slot = Some(adapter.clone());
        Ok(())
    }
}

impl McpAdapterComposer for PrivateNativeSetup {
    fn compose(
        &self,
        adapter: &Arc<McpRouterAdapter>,
        session: &SessionId,
        member: Option<&MobMemberBinding>,
    ) -> Result<Arc<dyn AgentToolDispatcher>, McpAdapterComposeRefusal> {
        let refused = || {
            McpAdapterComposeRefusal::new("The original private setup composition is unavailable")
        };
        if session != &self.session
            || member.is_some()
            || self.closed.load(Ordering::Acquire)
            || !self
                .adapter
                .lock()
                .map_err(|_| refused())?
                .as_ref()
                .is_some_and(|held| Arc::ptr_eq(held, adapter))
        {
            return Err(refused());
        }
        let terminal = self
            .terminal
            .lock()
            .map_err(|_| refused())?
            .take()
            .ok_or_else(refused)?;
        let installs = crate::service_install::CheckedInstalls::with_checked_route(
            adapter.clone(),
            session.clone(),
        )
        .map_err(|_| refused())?;
        let (mut lazy, presenter) = LazySetup::compose_with_adapter(
            self.paired_host.clone(),
            false,
            true,
            false,
            false,
            true,
            adapter.clone(),
            Some(installs),
            Some(terminal),
            None,
            true,
        );
        lazy.owner
            .private_custody
            .set(self.custody.clone())
            .map_err(|_| refused())?;
        lazy.claim().map_err(|_| refused())?;
        lazy.installs()
            .ok_or_else(refused)?
            .bind_realm(&self.realm)
            .map_err(|_| refused())?;
        self.approvals
            .check_session(session)
            .map_err(|_| refused())?;
        self.admission
            .0
            .set(
                self.approvals
                    .native_setup_admission(&lazy)
                    .map_err(|_| refused())?,
            )
            .map_err(|_| refused())?;
        lazy.dispatcher = Arc::new(RegisteredToolBundleDispatcher::new(
            BUNDLE,
            lazy.dispatcher(),
        ));
        let dispatcher = lazy.dispatcher();
        let mut published = self.published.lock().map_err(|_| refused())?;
        if published.is_some() {
            return Err(refused());
        }
        *published = Some((lazy, presenter));
        Ok(dispatcher)
    }
}

fn unavailable() -> ConnectError {
    ConnectError::InvalidInput("The original eagerly prepared private setup is unavailable".into())
}
struct Retained(Arc<PrivateNativeSetup>);
impl std::fmt::Debug for Retained {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetainedPrivateNativeSetup")
    }
}
impl std::fmt::Display for Retained {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = &self.0;
        f.write_str(
            "Private native setup cleanup is unconfirmed; its original owner remains retained",
        )
    }
}
impl std::error::Error for Retained {}
