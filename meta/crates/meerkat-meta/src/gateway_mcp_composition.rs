//! Trusted host MCP composition for the gateway's member factories.
//!
//! A host-composed gateway builds its member agents through factories it
//! constructs during init, before any session service, bootstrap spec or
//! member exists. `GatewayHostComposition::prepare_bootstrap` runs only after
//! those factories are consumed, so a host's MCP adapter composer
//! (`AgentFactory::with_mcp_adapter_composer`) is offered here instead: once
//! per gateway factory, before the factory is cloned for live or handed to
//! its builder. Every first, restored and rebuilt member construction of that
//! factory then calls the host's composer for its own adapter. The same
//! composition can carry the host's MCP attempt guard installation
//! ([`GatewayMcpComposition::with_attempt_guards`]), installed on the factory
//! at the same point, so each of those constructions begins the host's guard
//! before any stdio server of its own is spawned.
//!
//! Every member factory's builder also gets the meerkat facade's canonical
//! MCP credential source for the installation's meerkat-level realm
//! ([`gateway_member_builder`]): a member launch of a stdio server that
//! realm's own config document defines reads the server's service secrets
//! from the factory's own credential store. The host administers those
//! definitions and values through
//! [`GatewayFactoryConstruction::realm_mcp_installation`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use meerkat::{
    AgentFactory, FactoryAgentBuilder, HostAuthError, HostAuthService, McpAdapterComposer,
    McpServiceSecretRemoval, McpServiceSecretUpdateError, ProviderAuthPersistence,
};
use meerkat_core::auth::McpServiceSecretValue;
use meerkat_core::storage_layout::{ResolvedStorage, StorageLayout};
use meerkat_core::{
    AdministrationEntry, AdministrationEntryRefusal, McpRealmPersistError, McpServerConfig, RealmId,
};
use meerkat_runtime::MeerkatMachine;

use crate::rpc_gateway::{GatewayHostComposition, GatewayHostError};

/// Which gateway member factory is being constructed.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayFactoryKind {
    /// The persistent launch's factory (`persistent_state` set).
    Persistent,
    /// The process-local launch's factory, behind the SDK callback builder.
    ProcessLocal,
}

/// The actual factory and native runtime a gateway member factory is built
/// on, offered to the host before the factory is cloned or consumed. It
/// grants nothing and hands out neither the factory nor the runtime.
pub struct GatewayFactoryConstruction<'a> {
    kind: GatewayFactoryKind,
    factory: &'a AgentFactory,
    runtime: &'a MeerkatMachine,
    storage: &'a ResolvedStorage,
    custody: &'a Arc<crate::member_mcp_custody::MemberMcpCustody>,
}

impl<'a> GatewayFactoryConstruction<'a> {
    pub(crate) fn new(
        kind: GatewayFactoryKind,
        factory: &'a AgentFactory,
        runtime: &'a MeerkatMachine,
        storage: &'a ResolvedStorage,
        custody: &'a Arc<crate::member_mcp_custody::MemberMcpCustody>,
    ) -> Self {
        Self {
            kind,
            factory,
            runtime,
            storage,
            custody,
        }
    }

    pub fn kind(&self) -> GatewayFactoryKind {
        self.kind
    }

    /// The installation's meerkat-level realm this factory serves.
    pub fn realm(&self) -> &RealmId {
        &self.storage.locator.realm
    }

    /// The account facade over this factory's own provider-auth persistence
    /// and this gateway machine's runtime authority, with MCP credentials
    /// scoped to the factory's realm. It is never built from a copied
    /// configuration or a new store; a factory without provider-auth
    /// persistence answers `HostAuthError::PersistenceUnavailable`.
    pub fn host_auth_service(&self) -> Result<HostAuthService, HostAuthError> {
        HostAuthService::from_factory(self.factory, self.runtime.provider_auth_runtime_authority())
            .map(|service| service.with_mcp_realm(self.realm().clone()))
    }

    /// The MCP server definitions of the factory's realm's own config
    /// document and the service secrets of their stdio servers, held in
    /// this factory's own provider-auth persistence: the credential store
    /// its members' launches read ([`gateway_member_builder`]). Like
    /// [`Self::host_auth_service`] it is never built from a copied
    /// configuration or a new store, and a factory without provider-auth
    /// persistence answers `HostAuthError::PersistenceUnavailable`.
    ///
    /// The handle authenticates and authorizes nothing itself: every
    /// mutation takes the host's [`AdministrationEntry`], which the mutation
    /// runs at its entry, after the custody it waits for, to re-check that
    /// the caller is still the authenticated administrator.
    pub fn realm_mcp_installation(&self) -> Result<GatewayRealmMcpInstallation, HostAuthError> {
        let persistence = self
            .factory
            .resolution_provider_auth_persistence()
            .map_err(HostAuthError::Factory)?
            .ok_or(HostAuthError::PersistenceUnavailable)?;
        Ok(GatewayRealmMcpInstallation {
            storage: self.storage.clone(),
            persistence,
        })
    }

    /// The gateway's member MCP custody: the one owner every member
    /// construction of this gateway adopts its MCP adapter into, before any
    /// server is staged. The host keeps it to tell the gateway's own members
    /// that a credential landed or was released, after a login or a logout
    /// through [`Self::host_auth_service`]
    /// ([`crate::MemberMcpCustody::credential_landed`],
    /// [`crate::MemberMcpCustody::credential_released`]), so they follow it in
    /// the same runtime. Hosts call only `credential_landed` and
    /// `credential_released` on it; adopting or installing their own adapters
    /// would put them under this gateway's shutdown settlement.
    pub fn member_mcp_custody(&self) -> Arc<crate::member_mcp_custody::MemberMcpCustody> {
        Arc::clone(self.custody)
    }
}

/// The installation realm's own MCP server definitions and the service
/// secrets of their stdio servers, for the host's authenticated
/// configuration administrator (see
/// [`GatewayFactoryConstruction::realm_mcp_installation`]).
///
/// A value is set for the exact definition the administrator reviewed:
/// [`Self::install`] the reviewed definition at the generation
/// [`Self::servers`] read, then [`Self::set_secret`] each slot it declares.
/// The value is stored in the factory's own credential store, bound to that
/// definition, and only while the realm's own document lists exactly that
/// definition; a member launch of the definition then reads it through
/// [`gateway_member_builder`]'s credential source. No value is returned,
/// written to configuration or shown by `Debug`.
///
/// Each mutation runs the host's [`AdministrationEntry`] at its entry, after
/// the custody it waits for and immediately before it writes, and holds the
/// entered admission until it settles. The steps are not atomic and their
/// outcomes stay separate: an install that succeeded says nothing about the
/// secret write after it. A secret write refused before it wrote leaves the
/// slot as it was. A secret write whose outcome is unknown may or may not
/// have stored the value, so a launch may already read it. Only a known
/// outcome that wrote nothing on a slot known to be empty, or a removal that
/// succeeded, establishes that a launch finds no value. Nothing is retried
/// or rolled back here, and an unknown outcome is never a success.
#[derive(Clone)]
pub struct GatewayRealmMcpInstallation {
    storage: ResolvedStorage,
    persistence: ProviderAuthPersistence,
}

/// Why the installation realm's own MCP servers could not be read or
/// written.
#[non_exhaustive]
#[derive(Debug)]
pub enum GatewayRealmMcpError {
    /// The realm's own config document has no location (the reserved
    /// `global` realm, without a user-global document here). Nothing was
    /// read or written.
    NoDocument,
    /// The realm's own servers could not be read, or the install was refused
    /// before its entry (a changed generation, a listed server of the same
    /// name, an environment reference, an invalid definition, or the
    /// document could not be held). Nothing was written.
    Persist(McpRealmPersistError),
    /// The host's entry check refused the caller at the install's entry.
    /// Nothing was written.
    NotAdmitted(AdministrationEntryRefusal),
    /// The install's write began after the caller was admitted and failed:
    /// the realm's own document may or may not list the server. Review again
    /// ([`GatewayRealmMcpInstallation::servers`]) before anything depends on
    /// it.
    OutcomeUnknown(String),
}

impl std::fmt::Display for GatewayRealmMcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDocument => {
                f.write_str("the installation realm's own config document has no location")
            }
            Self::Persist(error) => std::fmt::Display::fmt(error, f),
            Self::NotAdmitted(refusal) => std::fmt::Display::fmt(refusal, f),
            Self::OutcomeUnknown(reason) => {
                write!(f, "the realm MCP install's outcome is unknown: {reason}")
            }
        }
    }
}

impl std::error::Error for GatewayRealmMcpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoDocument | Self::NotAdmitted(_) | Self::OutcomeUnknown(_) => None,
            Self::Persist(error) => std::error::Error::source(error),
        }
    }
}

impl From<McpRealmPersistError> for GatewayRealmMcpError {
    fn from(error: McpRealmPersistError) -> Self {
        match error {
            McpRealmPersistError::NotAdmitted(refusal) => Self::NotAdmitted(refusal),
            McpRealmPersistError::OutcomeUnknown(error) => Self::OutcomeUnknown(error.to_string()),
            error => Self::Persist(error),
        }
    }
}

impl GatewayRealmMcpInstallation {
    /// The realm whose own config document this administers.
    pub fn realm(&self) -> &RealmId {
        &self.storage.locator.realm
    }

    /// The servers the realm's own document lists (never the ones it would
    /// inherit) and the document's generation, for review.
    pub async fn servers(&self) -> Result<(Vec<McpServerConfig>, u64), GatewayRealmMcpError> {
        let config = self.config()?;
        Ok(meerkat_core::McpRealmPersistTarget::new(&config)
            .servers()
            .await?)
    }

    /// Add `reviewed` to the realm's own document
    /// ([`meerkat_core::McpRealmPersistTarget::add_entered`]), admitted by
    /// `entry` after the document's locks and the check that it is still at
    /// `expected_generation`, immediately before it is written. Refused with
    /// nothing written when the generation changed, the document already
    /// lists a server of that name, or `entry` refuses. Returns the
    /// document's new generation.
    pub async fn install(
        &self,
        reviewed: McpServerConfig,
        expected_generation: u64,
        entry: Arc<dyn AdministrationEntry>,
    ) -> Result<u64, GatewayRealmMcpError> {
        let config = self.config()?;
        let snapshot = meerkat_core::McpRealmPersistTarget::new(&config)
            .add_entered(reviewed, Some(expected_generation), entry.as_ref())
            .await?;
        Ok(snapshot.generation)
    }

    /// Store `value` for slot `env` of `reviewed`
    /// ([`meerkat::set_mcp_service_secret_entered`]): refused with nothing
    /// written unless the realm's own document lists exactly `reviewed`,
    /// checked under the document's writer exclusion, and unless `entry`
    /// admits the caller at the write's entry, after that check and the
    /// credential owner's exclusion. Setting again replaces the value.
    pub async fn set_secret(
        &self,
        reviewed: &McpServerConfig,
        env: &str,
        value: McpServiceSecretValue,
        entry: Arc<dyn AdministrationEntry>,
    ) -> Result<(), McpServiceSecretUpdateError> {
        meerkat::set_mcp_service_secret_entered(
            &self.storage,
            &self.persistence,
            reviewed,
            env,
            value,
            entry,
        )
        .await
        .map(|_| ())
    }

    /// Remove the value of slot `env` of `reviewed`
    /// ([`meerkat::remove_mcp_service_secret_entered`]), admitted by `entry`
    /// at the removal's entry exactly as [`Self::set_secret`] is.
    pub async fn remove_secret(
        &self,
        reviewed: &McpServerConfig,
        env: &str,
        removal: McpServiceSecretRemoval,
        entry: Arc<dyn AdministrationEntry>,
    ) -> Result<(), McpServiceSecretUpdateError> {
        meerkat::remove_mcp_service_secret_entered(
            &self.storage,
            &self.persistence,
            reviewed,
            env,
            removal,
            entry,
        )
        .await
    }

    /// The native MCP target `target` names in this realm's effective
    /// configuration: its own document folded with the chain it inherits,
    /// the same configuration the realm's credential source reads. Meerkat's
    /// own resolver decides
    /// ([`meerkat::resolve_configured_mcp_target_in_realm_trusting`]): the
    /// server must be configured under that name, `server_url` must equal
    /// its configured URL, it must use OAuth login, and a named account or
    /// selection must equal the configured one. Gateway members read no
    /// project or user `mcp.toml`, so none is consulted. The identity is
    /// built from the configuration, never from the request; pass it to
    /// [`GatewayFactoryConstruction::host_auth_service`]'s MCP login, status
    /// and logout, which serve this realm's credential slot.
    pub async fn resolve_mcp_auth_target(
        &self,
        target: &meerkat_contracts::WireMcpAuthTarget,
    ) -> Result<meerkat::McpServerIdentity, HostAuthError> {
        let realm = &self.storage.locator.realm;
        let source = meerkat_store::FilesystemRealmConfigSource::new(
            self.storage.locator.state_root.clone(),
            self.storage.layout.global_config_path().unwrap_or_default(),
            meerkat_models::canonical(),
        );
        let config = meerkat_core::EffectiveConfigReader::new(Arc::new(source))
            .effective_config(realm)
            .await
            .map_err(|error| {
                HostAuthError::McpTarget(meerkat::HostMcpTargetRefusal::ConfigUnavailable(
                    meerkat_core::McpConfigError::Io(format!(
                        "the realm's configuration could not be composed: {error}"
                    )),
                ))
            })?;
        meerkat::resolve_configured_mcp_target_in_realm_trusting(
            target,
            &config.tools.mcp_servers,
            None,
            None,
            &|_| true,
        )
        .await
    }

    /// The config runtime of the realm's own document.
    fn config(&self) -> Result<meerkat_core::ConfigRuntime, GatewayRealmMcpError> {
        let document = realm_document(&self.storage).ok_or(GatewayRealmMcpError::NoDocument)?;
        let state = document.with_file_name("config_state.json");
        Ok(meerkat_core::ConfigRuntime::new(
            Arc::new(meerkat_core::FileConfigStore::new(
                document,
                meerkat_models::canonical(),
            )),
            state,
        ))
    }
}

impl std::fmt::Debug for GatewayRealmMcpInstallation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayRealmMcpInstallation")
            .field("realm", self.realm())
            .finish_non_exhaustive()
    }
}

/// The realm's own config document by the facade's realm-to-document
/// projection ([`meerkat_store::FilesystemRealmConfigSource`]), which the
/// service-secret writes and the member credential source resolve through
/// too: `None` for the reserved `global` realm without a user-global
/// document.
fn realm_document(storage: &ResolvedStorage) -> Option<PathBuf> {
    let global = storage.layout.global_config_path();
    if storage.locator.realm.is_global() && global.is_none() {
        return None;
    }
    Some(
        meerkat_store::FilesystemRealmConfigSource::new(
            storage.locator.state_root.clone(),
            global.unwrap_or_default(),
            meerkat_models::canonical(),
        )
        .config_doc_path(&storage.locator.realm),
    )
}

/// The installation's meerkat-level realm as the meerkat realm storage its
/// own config document and its members' service-secret custody resolve
/// through: the gateway state directory as the realm's state root (where the
/// gateway's meerkat-level stores open), and no user-global document, so the
/// realm inherits none of the operator's own servers.
pub(crate) fn meerkat_level_realm_storage(state_dir: &Path, realm: &RealmId) -> ResolvedStorage {
    ResolvedStorage {
        layout: StorageLayout::with_injected_roots(
            state_dir.to_path_buf(),
            None,
            None,
            state_dir.to_path_buf(),
        ),
        locator: meerkat_core::RealmLocator {
            state_root: state_dir.to_path_buf(),
            realm: realm.clone(),
        },
        root_choice: meerkat_core::RealmRootChoice::Explicit,
    }
}

/// The builder of a gateway member factory, with the meerkat facade's
/// canonical MCP credential source for `storage`'s realm
/// ([`meerkat::surface::install_default_mcp_auth_resolver_for_realm`]): bound
/// to the gateway runtime's AuthMachine owner and the factory's own
/// provider-auth persistence, and serving the service secrets stored there
/// for the definitions the realm's own document lists. Each construction
/// waits for its configured MCP servers to connect or fail before it is
/// prepared, so the surface its host decides on is captured after them.
pub(crate) fn gateway_member_builder(
    factory: AgentFactory,
    config: meerkat_core::Config,
    runtime: &Arc<MeerkatMachine>,
    storage: &ResolvedStorage,
) -> FactoryAgentBuilder {
    let mut builder = meerkat::surface::install_default_mcp_auth_resolver_for_realm(
        FactoryAgentBuilder::new(factory, config),
        runtime,
        storage,
    );
    builder.wait_for_mcp = true;
    builder
}

/// The host's trusted MCP composition for one gateway member factory: its
/// adapter composer and, optionally, a per-call MCP context provider and an
/// MCP attempt guard installation.
pub struct GatewayMcpComposition {
    composer: Arc<dyn McpAdapterComposer>,
    call_context_provider: Option<Arc<dyn meerkat_mcp::McpCallContextProvider>>,
    attempt_guards: Option<meerkat_mcp::McpAttemptGuards>,
}

impl GatewayMcpComposition {
    /// Compose every MCP adapter this factory's constructions build. The
    /// composer runs once per construction, including a same-session rebuild;
    /// it must select a fresh private review owner for each call and never
    /// reuse a single-use one. It is not called for a construction without
    /// configured MCP servers.
    pub fn new(composer: Arc<dyn McpAdapterComposer>) -> Self {
        Self {
            composer,
            call_context_provider: None,
            attempt_guards: None,
        }
    }

    /// Also install the factory-wide per-call MCP context provider. Without
    /// one the factory keeps its native provider selection.
    pub fn with_call_context_provider(
        mut self,
        provider: Arc<dyn meerkat_mcp::McpCallContextProvider>,
    ) -> Self {
        self.call_context_provider = Some(provider);
        self
    }

    /// Also guard every stdio MCP server attempt of this factory's
    /// constructions with the host's installation (see
    /// [`meerkat_mcp::McpAttemptGuards`]). Each first, restored and rebuilt
    /// member construction begins the guard for its own session and member
    /// before the attempt's secret resolution, physical entry or spawn; the
    /// guard is finished once the attempt's open outcome and process end are
    /// both known. The installation covers the routers these constructions
    /// build. The gateway's member session services do not offer it to any
    /// other router: they do not forward
    /// `meerkat_session::McpAttemptGuardSource`. Remote (HTTP and SSE)
    /// attempts are neither guarded nor refused. The host keeps its own clone
    /// of the installation and closes and joins it
    /// ([`meerkat_mcp::McpAttemptGuards::close_and_join`]) at its shutdown.
    pub fn with_attempt_guards(mut self, guards: meerkat_mcp::McpAttemptGuards) -> Self {
        self.attempt_guards = Some(guards);
        self
    }

    fn install(self, factory: AgentFactory) -> AgentFactory {
        let factory = factory.with_mcp_adapter_composer(self.composer);
        let factory = match self.call_context_provider {
            Some(provider) => factory.mcp_call_context_provider(provider),
            None => factory,
        };
        match self.attempt_guards {
            Some(guards) => factory.with_mcp_attempt_guards(guards),
            None => factory,
        }
    }
}

/// A gateway member factory as the host composed it, with the approval owner
/// that issues its review's human consent when the host's review routes R3
/// to human consent: the launch hands that issuer to its runtime delivery
/// composition, whose delivery owner projects the consent notices the issuer
/// owes ([`crate::MobRuntimeDelivery::with_consent_notice_issuer`]).
pub(crate) struct ComposedGatewayMemberFactory {
    pub(crate) factory: AgentFactory,
    pub(crate) consent_notice_issuer: Option<Arc<meerkat_core::ApprovalService>>,
}

/// Offer a freshly constructed gateway member factory to the host. This is
/// the only place the gateway sets a member factory's MCP composer,
/// call-context provider, attempt guards, operation refusal observer or
/// operation review, and the gateway sets none of them (nor a connector
/// authority) on these factories before it, so a host selection replaces no
/// existing choice. The host answers every question about the same
/// construction before any answer is installed. A review that routes R3 to
/// human consent first installs its consent owner-loss settlement on
/// `runtime`, the gateway's runtime this factory's sessions register on,
/// while no session exists on it; a runtime that refuses it refuses the
/// factory. Such a review's consent notice issuer comes back with the
/// factory. Every factory adopts its MCP adapters into the gateway's
/// `custody` at its ownership seam, before any server is staged, whatever
/// the host answers. No host, or a host answering `None` to every question,
/// leaves the factory otherwise exactly as constructed, with no issuer.
pub(crate) fn compose_gateway_member_factory(
    host: Option<&Arc<dyn GatewayHostComposition>>,
    kind: GatewayFactoryKind,
    factory: AgentFactory,
    runtime: &MeerkatMachine,
    storage: &ResolvedStorage,
    custody: &Arc<crate::member_mcp_custody::MemberMcpCustody>,
) -> Result<ComposedGatewayMemberFactory, GatewayHostError> {
    let factory = custody.install(factory);
    let Some(host) = host else {
        return Ok(ComposedGatewayMemberFactory {
            factory,
            consent_notice_issuer: None,
        });
    };
    let construction = GatewayFactoryConstruction::new(kind, &factory, runtime, storage, custody);
    let composition = host.compose_member_factory(&construction)?;
    let observer = host.operation_refusal_observer(&construction)?;
    let review = host.operation_review(&construction)?;
    if let Some(settlement) = review
        .as_ref()
        .and_then(|review| review.consent_owner_loss_settlement())
    {
        runtime
            .install_consent_owner_loss_settlement(settlement)
            .map_err(|refusal| {
                tracing::warn!(
                    %refusal,
                    "the gateway runtime refused the consent owner-loss settlement"
                );
                GatewayHostError::ConsentSettlementRefused
            })?;
    }
    let consent_notice_issuer = review
        .as_ref()
        .and_then(|review| review.consent_notice_issuer());
    let factory = match composition {
        Some(composition) => composition.install(factory),
        None => factory,
    };
    let factory = match observer {
        Some(observer) => factory.with_operation_refusal_observer(observer),
        None => factory,
    };
    Ok(ComposedGatewayMemberFactory {
        factory: match review {
            Some(review) => factory.with_operation_review(review),
            None => factory,
        },
        consent_notice_issuer,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use meerkat::{AgentToolDispatcher, FactoryAgentBuilder, McpAdapterComposeRefusal};
    use meerkat_client::types::LlmStream;
    use meerkat_client::{LlmClient, LlmRequest};
    use meerkat_core::types::SessionId;
    use meerkat_session::SessionAgentBuilder as _;

    use super::*;
    use crate::MobBootstrapSpec;
    use crate::UnifiedRuntime;
    use crate::rpc_gateway::{GatewayBootstrapReady, GatewayHostCloseReason};

    struct Recording {
        composed: Mutex<Vec<SessionId>>,
    }

    impl McpAdapterComposer for Recording {
        fn compose(
            &self,
            adapter: &Arc<meerkat_mcp::McpRouterAdapter>,
            session_id: &SessionId,
            _member: Option<&meerkat_core::MobMemberBinding>,
        ) -> Result<Arc<dyn AgentToolDispatcher>, McpAdapterComposeRefusal> {
            self.composed.lock().unwrap().push(session_id.clone());
            Ok(Arc::clone(adapter) as Arc<dyn AgentToolDispatcher>)
        }
    }

    enum Answer {
        Compose(Arc<Recording>),
        Guarded(Arc<Recording>, meerkat_mcp::McpAttemptGuards),
        Ordinary,
        Refuse,
    }

    struct ComposingHost {
        answer: Answer,
        seen: Mutex<Vec<(GatewayFactoryKind, String, bool)>>,
        installations: Mutex<Vec<GatewayRealmMcpInstallation>>,
        /// The member MCP custody each construction handed the host.
        custodies: Mutex<Vec<Arc<crate::member_mcp_custody::MemberMcpCustody>>>,
        /// What the host answers when asked for its refusal observer.
        observer: ObserverAnswer,
        /// The construction kind of each observer offer, in order.
        observer_offers: Mutex<Vec<GatewayFactoryKind>>,
        /// What the host answers when asked for its operation review.
        review: ReviewAnswer,
        /// The construction kind of each review offer, in order.
        review_offers: Mutex<Vec<GatewayFactoryKind>>,
    }

    enum ObserverAnswer {
        Install(Arc<RecordingObserver>),
        Ordinary,
        Refuse,
    }

    enum ReviewAnswer {
        Install(Arc<meerkat_core::approval::review::BoundOperationReview>),
        Ordinary,
        Refuse,
    }

    fn composing_host(answer: Answer) -> Arc<ComposingHost> {
        observing_host(answer, ObserverAnswer::Ordinary)
    }

    fn observing_host(answer: Answer, observer: ObserverAnswer) -> Arc<ComposingHost> {
        host_answering(answer, observer, ReviewAnswer::Ordinary)
    }

    fn reviewing_host(review: ReviewAnswer) -> Arc<ComposingHost> {
        host_answering(Answer::Ordinary, ObserverAnswer::Ordinary, review)
    }

    fn host_answering(
        answer: Answer,
        observer: ObserverAnswer,
        review: ReviewAnswer,
    ) -> Arc<ComposingHost> {
        Arc::new(ComposingHost {
            answer,
            seen: Mutex::new(Vec::new()),
            installations: Mutex::new(Vec::new()),
            custodies: Mutex::new(Vec::new()),
            observer,
            observer_offers: Mutex::new(Vec::new()),
            review,
            review_offers: Mutex::new(Vec::new()),
        })
    }

    #[async_trait]
    impl GatewayHostComposition for ComposingHost {
        async fn prepare_bootstrap(
            &self,
            spec: MobBootstrapSpec,
        ) -> Result<MobBootstrapSpec, GatewayHostError> {
            Ok(spec)
        }
        fn native_grant_work_configuration(
            &self,
        ) -> Result<
            Option<meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration>,
            GatewayHostError,
        > {
            Ok(None)
        }
        async fn bind_runtime(
            &self,
            _runtime: &mut UnifiedRuntime,
        ) -> Result<(), GatewayHostError> {
            Ok(())
        }
        fn publish_bootstrap_ready(
            &self,
            _publication: GatewayBootstrapReady<'_>,
        ) -> Result<(), GatewayHostError> {
            Err(GatewayHostError::Denied)
        }
        fn begin_close(&self, _reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
            Ok(())
        }
        async fn join_close(&self) -> Result<(), GatewayHostError> {
            Ok(())
        }
        async fn wait_closed(&self) {
            std::future::pending::<()>().await;
        }
        fn compose_member_factory(
            &self,
            construction: &GatewayFactoryConstruction<'_>,
        ) -> Result<Option<GatewayMcpComposition>, GatewayHostError> {
            self.seen.lock().unwrap().push((
                construction.kind(),
                construction.realm().as_str().to_string(),
                construction.host_auth_service().is_ok(),
            ));
            if let Ok(installation) = construction.realm_mcp_installation() {
                self.installations.lock().unwrap().push(installation);
            }
            self.custodies
                .lock()
                .unwrap()
                .push(construction.member_mcp_custody());
            match &self.answer {
                Answer::Compose(recording) => Ok(Some(GatewayMcpComposition::new(Arc::clone(
                    recording,
                )
                    as Arc<dyn McpAdapterComposer>))),
                Answer::Guarded(recording, guards) => {
                    Ok(Some(
                        GatewayMcpComposition::new(
                            Arc::clone(recording) as Arc<dyn McpAdapterComposer>
                        )
                        .with_attempt_guards(guards.clone()),
                    ))
                }
                Answer::Ordinary => Ok(None),
                Answer::Refuse => Err(GatewayHostError::Denied),
            }
        }
        fn operation_refusal_observer(
            &self,
            construction: &GatewayFactoryConstruction<'_>,
        ) -> Result<Option<Arc<dyn meerkat_core::OperationRefusalObserver>>, GatewayHostError>
        {
            self.observer_offers
                .lock()
                .unwrap()
                .push(construction.kind());
            match &self.observer {
                ObserverAnswer::Install(observer) => Ok(Some(
                    Arc::clone(observer) as Arc<dyn meerkat_core::OperationRefusalObserver>
                )),
                ObserverAnswer::Ordinary => Ok(None),
                ObserverAnswer::Refuse => Err(GatewayHostError::Unavailable),
            }
        }
        fn operation_review(
            &self,
            construction: &GatewayFactoryConstruction<'_>,
        ) -> Result<
            Option<Arc<meerkat_core::approval::review::BoundOperationReview>>,
            GatewayHostError,
        > {
            self.review_offers.lock().unwrap().push(construction.kind());
            match &self.review {
                ReviewAnswer::Install(review) => Ok(Some(Arc::clone(review))),
                ReviewAnswer::Ordinary => Ok(None),
                ReviewAnswer::Refuse => Err(GatewayHostError::Unavailable),
            }
        }
    }

    #[derive(Default)]
    struct BuildOnlyLlmClient;

    #[async_trait]
    impl LlmClient for BuildOnlyLlmClient {
        fn stream<'a>(&'a self, _request: &'a LlmRequest) -> LlmStream<'a> {
            Box::pin(futures::stream::empty())
        }
        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }
        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    fn runtime_machine() -> MeerkatMachine {
        crate::mob_handle_runtime::persistent_runtime_machine(
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
            None,
        )
        .expect("runtime machine")
    }

    fn realm() -> RealmId {
        RealmId::parse("gateway-composition").expect("realm")
    }

    fn storage(dir: &std::path::Path) -> ResolvedStorage {
        meerkat_level_realm_storage(&dir.join("state"), &realm())
    }

    /// A construction with one configured MCP server. The server never
    /// answers; the construction does not wait for it, and the composer runs
    /// when the adapter is created, before any connection settles.
    fn mcp_request() -> meerkat_core::service::CreateSessionRequest {
        request_with(vec![McpServerConfig::stdio(
            "silent",
            "sh",
            vec!["-c".to_string(), "sleep 30".to_string()],
            std::collections::HashMap::new(),
        )])
    }

    /// A deferred construction with `servers` configured.
    fn request_with(servers: Vec<McpServerConfig>) -> meerkat_core::service::CreateSessionRequest {
        let build = meerkat_core::service::SessionBuildOptions {
            mcp_servers: servers,
            ..Default::default()
        };
        meerkat_core::service::CreateSessionRequest {
            model: "mock-model".to_string(),
            prompt: meerkat_core::ContentInput::Text("noop".to_string()),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            build: Some(build),
            labels: None,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            injected_context: Vec::new(),
        }
    }

    fn composed(
        host: &Arc<ComposingHost>,
        dir: &std::path::Path,
        machine: &MeerkatMachine,
        custody: &Arc<crate::member_mcp_custody::MemberMcpCustody>,
    ) -> Result<AgentFactory, GatewayHostError> {
        let host: Arc<dyn GatewayHostComposition> = host.clone();
        compose_gateway_member_factory(
            Some(&host),
            GatewayFactoryKind::Persistent,
            AgentFactory::new(dir).builtins(false).comms(true),
            machine,
            &storage(dir),
            custody,
        )
        .map(|composed| composed.factory)
    }

    /// The host's composer reaches every construction of the factory it was
    /// offered: a first build and a second construction each call it once,
    /// with their own session, through the builder that consumed the factory.
    #[tokio::test]
    async fn the_host_composer_runs_for_each_construction_of_its_factory() {
        let dir = tempfile::tempdir().unwrap();
        let machine = runtime_machine();
        let recording = Arc::new(Recording {
            composed: Mutex::new(Vec::new()),
        });
        let host = composing_host(Answer::Compose(Arc::clone(&recording)));
        let custody = crate::member_mcp_custody::MemberMcpCustody::new();
        let factory = composed(&host, dir.path(), &machine, &custody).expect("composed");
        assert_eq!(
            *host.seen.lock().unwrap(),
            [(
                GatewayFactoryKind::Persistent,
                "gateway-composition".to_string(),
                true
            )],
            "offered once, with its realm and the factory's own account facade"
        );

        let mut builder = FactoryAgentBuilder::new(factory, meerkat::Config::default());
        builder.default_llm_client = Some(Arc::new(BuildOnlyLlmClient));
        let mut sessions = Vec::new();
        for _ in 0..2 {
            let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
            let built = builder
                .build_agent(&mcp_request(), event_tx)
                .await
                .unwrap_or_else(|error| panic!("offline MCP construction: {error}"));
            sessions.push(built.agent().session().id().clone());
        }
        assert_eq!(
            *recording.composed.lock().unwrap(),
            sessions,
            "composed once per construction, for that construction's session"
        );
        assert_eq!(
            custody.len(),
            2,
            "every construction's adapter is adopted into the gateway's custody"
        );
    }

    /// No host, or a host answering `None`, leaves constructions ordinary; a
    /// refusing host fails the factory before anything is built from it.
    #[tokio::test]
    async fn an_ordinary_or_refusing_host_is_honoured() {
        let dir = tempfile::tempdir().unwrap();
        let machine = runtime_machine();
        assert!(
            compose_gateway_member_factory(
                None,
                GatewayFactoryKind::ProcessLocal,
                AgentFactory::new(dir.path()),
                &machine,
                &storage(dir.path()),
                &crate::member_mcp_custody::MemberMcpCustody::new(),
            )
            .is_ok()
        );
        let ordinary = composing_host(Answer::Ordinary);
        let custody = crate::member_mcp_custody::MemberMcpCustody::new();
        let factory = composed(&ordinary, dir.path(), &machine, &custody).expect("ordinary");
        let mut builder = FactoryAgentBuilder::new(factory, meerkat::Config::default());
        builder.default_llm_client = Some(Arc::new(BuildOnlyLlmClient));
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
        builder
            .build_agent(&mcp_request(), event_tx)
            .await
            .unwrap_or_else(|error| panic!("an ordinary MCP construction builds: {error}"));
        assert_eq!(ordinary.seen.lock().unwrap().len(), 1);
        assert_eq!(
            custody.len(),
            1,
            "a host answering None leaves the construction ordinary, still in custody"
        );

        let refusing = composing_host(Answer::Refuse);
        assert_eq!(
            composed(
                &refusing,
                dir.path(),
                &machine,
                &crate::member_mcp_custody::MemberMcpCustody::new()
            )
            .err(),
            Some(GatewayHostError::Denied)
        );
    }

    /// Every operation refusal observation a host receives, in order.
    #[derive(Default)]
    struct RecordingObserver(Mutex<Vec<meerkat_core::OperationRefusalObservation>>);

    impl meerkat_core::OperationRefusalObserver for RecordingObserver {
        fn observe(&self, observation: &meerkat_core::OperationRefusalObservation) {
            self.0.lock().unwrap().push(observation.clone());
        }
    }

    impl RecordingObserver {
        /// Session, surface, outcome and disposition of each observation.
        fn observed(
            &self,
        ) -> Vec<(
            SessionId,
            meerkat_core::OperationRefusalSurface,
            meerkat_core::OperationRefusalOutcome,
            meerkat_core::OperationRefusalDisposition,
        )> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|observation| {
                    (
                        observation.session_id().clone(),
                        observation.surface().clone(),
                        observation.outcome(),
                        observation.disposition(),
                    )
                })
                .collect()
        }
    }

    const REFUSED_TOOL: &str = "refused_probe";
    const REFUSED_CALL: &str = "refused-call";

    /// One tool whose owner refuses every call.
    struct RefusingTool;

    #[async_trait]
    impl AgentToolDispatcher for RefusingTool {
        fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
            vec![Arc::new(meerkat_core::types::ToolDef::new(
                REFUSED_TOOL,
                "A tool whose owner refuses every call.",
                serde_json::json!({"type": "object", "properties": {}}),
            ))]
            .into()
        }

        async fn dispatch(
            &self,
            _call: meerkat_core::types::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            Err(meerkat_core::ToolError::AuthorizationRefused {
                refusal: meerkat_core::OperationRefused::new(
                    meerkat_core::OperationRefusalKind::Denied,
                ),
            })
        }
    }

    /// Calls the refused tool on its first request and ends the turn on the
    /// next.
    #[derive(Default)]
    struct RefusedCallClient(std::sync::atomic::AtomicUsize);

    #[async_trait]
    impl LlmClient for RefusedCallClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, _request: &'a LlmRequest) -> LlmStream<'a> {
            let first = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            let events = if first {
                vec![
                    meerkat_client::LlmEvent::ToolCallComplete {
                        id: REFUSED_CALL.to_string(),
                        name: REFUSED_TOOL.to_string(),
                        args: serde_json::json!({}),
                        meta: None,
                    },
                    meerkat_client::LlmEvent::Done {
                        outcome: meerkat_client::LlmDoneOutcome::success(
                            meerkat_core::StopReason::ToolUse,
                        ),
                    },
                ]
            } else {
                vec![
                    meerkat_client::LlmEvent::TextDelta {
                        delta: "done".to_string(),
                        meta: None,
                    },
                    meerkat_client::LlmEvent::Done {
                        outcome: meerkat_client::LlmDoneOutcome::success(
                            meerkat_core::StopReason::EndTurn,
                        ),
                    },
                ]
            };
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }
        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }
        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    /// `kind`'s gateway factory, offered to `host`.
    fn composed_kind(
        host: &Arc<ComposingHost>,
        kind: GatewayFactoryKind,
        dir: &std::path::Path,
        machine: &MeerkatMachine,
    ) -> Result<AgentFactory, GatewayHostError> {
        composed_member(host, kind, dir, machine).map(|composed| composed.factory)
    }

    /// [`composed_kind`] with the consent notice issuer it hands back.
    fn composed_member(
        host: &Arc<ComposingHost>,
        kind: GatewayFactoryKind,
        dir: &std::path::Path,
        machine: &MeerkatMachine,
    ) -> Result<ComposedGatewayMemberFactory, GatewayHostError> {
        let host: Arc<dyn GatewayHostComposition> = host.clone();
        compose_gateway_member_factory(
            Some(&host),
            kind,
            AgentFactory::new(dir).builtins(false),
            machine,
            &storage(dir),
            &crate::member_mcp_custody::MemberMcpCustody::new(),
        )
    }

    /// Runs one turn of an agent `factory` builds. Its model calls the
    /// refused tool once; returns the turn's session.
    async fn run_refused_call(factory: AgentFactory) -> SessionId {
        let mut builder = FactoryAgentBuilder::new(factory, meerkat::Config::default());
        builder.default_llm_client = Some(Arc::new(RefusedCallClient::default()));
        builder.default_tool_dispatcher = Some(Arc::new(RefusingTool));
        let service = meerkat::EphemeralSessionService::new(builder, 2);
        let request = meerkat_core::service::CreateSessionRequest {
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            prompt: meerkat_core::ContentInput::Text("call the tool".to_string()),
            build: None,
            ..request_with(Vec::new())
        };
        meerkat_core::service::SessionService::create_session(&service, request)
            .await
            .unwrap_or_else(|error| panic!("the turn completes: {error}"))
            .session_id
    }

    /// Each gateway factory kind installs the host's observer: an agent it
    /// builds whose tool owner refuses a call reports exactly that refusal,
    /// for its own session. The host is asked once per factory, with that
    /// factory's construction.
    #[tokio::test]
    async fn every_gateway_factory_kind_installs_the_host_refusal_observer() {
        for kind in [
            GatewayFactoryKind::Persistent,
            GatewayFactoryKind::ProcessLocal,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let machine = runtime_machine();
            let observer = Arc::new(RecordingObserver::default());
            let host = observing_host(
                Answer::Ordinary,
                ObserverAnswer::Install(Arc::clone(&observer)),
            );
            let factory = composed_kind(&host, kind, dir.path(), &machine).expect("composed");
            assert_eq!(*host.observer_offers.lock().unwrap(), [kind]);
            assert_eq!(
                host.seen.lock().unwrap().len(),
                1,
                "{kind:?}: the MCP composition is still offered"
            );
            let session = run_refused_call(factory).await;
            assert_eq!(
                observer.observed(),
                [(
                    session,
                    meerkat_core::OperationRefusalSurface::ToolCall {
                        tool_use_id: REFUSED_CALL.to_string(),
                    },
                    meerkat_core::OperationRefusalOutcome::Refused {
                        kind: meerkat_core::OperationRefusalKind::Denied,
                        dispatch: meerkat_core::RefusalDispatch::Unknown,
                    },
                    meerkat_core::OperationRefusalDisposition::ToolResult,
                )],
                "{kind:?}: exactly the refused call, as its tool result"
            );
        }
    }

    /// No host, or a host answering `None`, installs no observer: the factory
    /// stays as constructed. A host refusing the observer refuses the factory.
    #[tokio::test]
    async fn without_a_host_observer_the_factory_is_left_as_constructed() {
        let dir = tempfile::tempdir().unwrap();
        let machine = runtime_machine();
        let installed = |factory: &AgentFactory| {
            format!("{factory:?}").contains("operation_refusal_observer: true")
        };
        let bare = compose_gateway_member_factory(
            None,
            GatewayFactoryKind::Persistent,
            AgentFactory::new(dir.path()),
            &machine,
            &storage(dir.path()),
            &crate::member_mcp_custody::MemberMcpCustody::new(),
        )
        .expect("no host")
        .factory;
        assert!(format!("{bare:?}").contains("operation_refusal_observer: false"));
        let ordinary = observing_host(Answer::Ordinary, ObserverAnswer::Ordinary);
        let factory = composed_kind(
            &ordinary,
            GatewayFactoryKind::Persistent,
            dir.path(),
            &machine,
        )
        .expect("ordinary");
        assert!(
            !installed(&factory),
            "a host answering None installs nothing"
        );
        assert_eq!(ordinary.observer_offers.lock().unwrap().len(), 1);
        let observing = observing_host(
            Answer::Ordinary,
            ObserverAnswer::Install(Arc::new(RecordingObserver::default())),
        );
        let factory = composed_kind(
            &observing,
            GatewayFactoryKind::Persistent,
            dir.path(),
            &machine,
        )
        .expect("observing");
        assert!(
            installed(&factory),
            "the control: the same check sees an install"
        );
        let refusing = observing_host(Answer::Ordinary, ObserverAnswer::Refuse);
        assert_eq!(
            composed_kind(
                &refusing,
                GatewayFactoryKind::Persistent,
                dir.path(),
                &machine,
            )
            .err(),
            Some(GatewayHostError::Unavailable),
            "a host that cannot provide its observer refuses the factory"
        );
    }

    /// A reviewer the rows below never reach: they install a review and run
    /// no reviewed operation.
    struct UnusedReviewer;

    #[async_trait]
    impl meerkat_core::approval::review::OperationReviewer for UnusedReviewer {
        async fn review(
            &self,
            _candidate: &meerkat_core::approval::review::ReviewCandidate<'_>,
        ) -> Result<
            meerkat_core::approval::review::ReviewVerdict,
            meerkat_core::approval::review::ReviewerFailure,
        > {
            Err(meerkat_core::approval::review::ReviewerFailure::Unavailable)
        }
    }

    /// A human consent route the rows below never reach.
    struct UnusedConsentRoute;

    impl meerkat_core::approval::consent::HumanConsentRoute for UnusedConsentRoute {
        fn present(
            &self,
            _candidate: &meerkat_core::approval::consent::ConsentPresentationCandidate<'_>,
        ) -> Result<
            meerkat_core::approval::consent::ConsentPresentation,
            meerkat_core::approval::consent::ConsentRouteUnavailable,
        > {
            Err(meerkat_core::approval::consent::ConsentRouteUnavailable)
        }

        fn eligibility(
            &self,
            _approver: &meerkat_core::approval::consent::AuthenticatedApprover,
            _approval: &meerkat_core::ApprovalRecord,
        ) -> Result<
            Arc<dyn meerkat_core::approval::consent::ConsentEligibility>,
            meerkat_core::approval::consent::ConsentIneligible,
        > {
            Err(meerkat_core::approval::consent::ConsentIneligible)
        }

        fn notice(&self, _notice: meerkat_core::approval::consent::ConsentNotice) {}
    }

    /// An operation review over its own approval owner, with R3 routed to
    /// human consent when `consent`.
    fn operation_review(
        consent: bool,
    ) -> Arc<meerkat_core::approval::review::BoundOperationReview> {
        let review = meerkat_core::approval::review::BoundOperationReview::new(
            Arc::new(UnusedReviewer),
            meerkat_core::ApprovalService::new(),
            std::time::Duration::from_secs(30),
        );
        Arc::new(if consent {
            review.with_human_consent(
                Arc::new(UnusedConsentRoute),
                std::time::Duration::from_mins(1),
            )
        } else {
            review
        })
    }

    fn reviewed(factory: &AgentFactory) -> bool {
        format!("{factory:?}").contains("operation_review: true")
    }

    /// A host review that routes R3 to human consent: its consent
    /// owner-loss settlement is installed on the gateway's runtime and the
    /// review on the factory. Each factory kind asks once, on its own
    /// runtime.
    #[tokio::test]
    async fn a_host_review_with_human_consent_installs_its_settlement_and_the_review() {
        for kind in [
            GatewayFactoryKind::Persistent,
            GatewayFactoryKind::ProcessLocal,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let machine = runtime_machine();
            let host = reviewing_host(ReviewAnswer::Install(operation_review(true)));
            let composed = composed_member(&host, kind, dir.path(), &machine).expect("composed");
            assert_eq!(*host.review_offers.lock().unwrap(), [kind]);
            assert!(
                reviewed(&composed.factory),
                "{kind:?}: the review is on the factory"
            );
            assert!(
                machine.has_consent_owner_loss_settlement(),
                "{kind:?}: the review's consent settlement is on the runtime"
            );
            assert!(
                composed.consent_notice_issuer.is_some(),
                "{kind:?}: the review's consent notice issuer comes back for the launch's delivery"
            );
        }
    }

    /// A host review without a human consent route is installed on the
    /// factory alone: it issues no consent, so no settlement is owed.
    #[tokio::test]
    async fn a_host_review_without_human_consent_installs_the_review_alone() {
        let dir = tempfile::tempdir().unwrap();
        let machine = runtime_machine();
        let host = reviewing_host(ReviewAnswer::Install(operation_review(false)));
        let composed = composed_member(&host, GatewayFactoryKind::Persistent, dir.path(), &machine)
            .expect("composed");
        assert!(reviewed(&composed.factory), "the review is on the factory");
        assert!(
            !machine.has_consent_owner_loss_settlement(),
            "no consent route, no settlement"
        );
        assert!(
            composed.consent_notice_issuer.is_none(),
            "no consent route, no consent notice issuer"
        );
    }

    /// No host, or a host answering `None`, installs no review: the factory
    /// stays as constructed.
    #[tokio::test]
    async fn without_a_host_review_the_factory_is_left_as_constructed() {
        let dir = tempfile::tempdir().unwrap();
        let machine = runtime_machine();
        let bare = compose_gateway_member_factory(
            None,
            GatewayFactoryKind::Persistent,
            AgentFactory::new(dir.path()),
            &machine,
            &storage(dir.path()),
            &crate::member_mcp_custody::MemberMcpCustody::new(),
        )
        .expect("no host");
        assert!(
            bare.consent_notice_issuer.is_none(),
            "no host, no consent notice issuer"
        );
        let bare = bare.factory;
        assert!(format!("{bare:?}").contains("operation_review: false"));
        let host = reviewing_host(ReviewAnswer::Ordinary);
        let composed = composed_member(&host, GatewayFactoryKind::Persistent, dir.path(), &machine)
            .expect("ordinary");
        assert_eq!(host.review_offers.lock().unwrap().len(), 1);
        assert!(
            !reviewed(&composed.factory),
            "a host answering None installs nothing"
        );
        assert!(
            composed.consent_notice_issuer.is_none(),
            "a host answering None hands back no consent notice issuer"
        );
    }

    /// A runtime that already holds a consent settlement refuses the host
    /// review's: the factory is refused, so init fails.
    #[tokio::test]
    async fn a_runtime_holding_a_settlement_refuses_the_host_review() {
        let dir = tempfile::tempdir().unwrap();
        let machine = runtime_machine();
        let earlier = operation_review(true)
            .consent_owner_loss_settlement()
            .expect("a consent route owes a settlement");
        machine
            .install_consent_owner_loss_settlement(earlier)
            .expect("the first settlement installs");
        let host = reviewing_host(ReviewAnswer::Install(operation_review(true)));
        assert_eq!(
            composed_kind(&host, GatewayFactoryKind::Persistent, dir.path(), &machine).err(),
            Some(GatewayHostError::ConsentSettlementRefused)
        );
    }

    /// A host that cannot provide its review refuses the factory, so init
    /// fails.
    #[tokio::test]
    async fn a_host_that_cannot_provide_its_review_refuses_the_factory() {
        let dir = tempfile::tempdir().unwrap();
        let machine = runtime_machine();
        let host = reviewing_host(ReviewAnswer::Refuse);
        assert_eq!(
            composed_kind(&host, GatewayFactoryKind::Persistent, dir.path(), &machine).err(),
            Some(GatewayHostError::Unavailable)
        );
    }

    /// Every begin and finish of a host's attempt guard installation.
    #[derive(Default)]
    struct GuardRecord {
        /// The server, its construction's session, and whether its process
        /// had already started (written its probe) when the begin ran.
        begun: Mutex<Vec<(String, SessionId, bool)>>,
        /// The server, the finish called, and how its process ended.
        finished: Mutex<Vec<(String, &'static str, &'static str)>>,
    }

    /// A host's guard factory: begins a recording guard for every attempt.
    struct RecordingGuards {
        record: Arc<GuardRecord>,
        probes: std::path::PathBuf,
    }

    #[async_trait]
    impl meerkat_mcp::McpAttemptGuardFactory for RecordingGuards {
        async fn begin(
            &self,
            request: meerkat_mcp::McpAttemptGuardRequest,
            _cancel: meerkat_mcp::McpAttemptCancellation,
        ) -> Result<
            Option<Box<dyn meerkat_mcp::McpAttemptGuard>>,
            meerkat_core::mcp_attachment::McpAttachmentRefusal,
        > {
            let server = request.key.server.clone();
            let started = self.probes.join(&server).exists();
            self.record.begun.lock().unwrap().push((
                server.clone(),
                request.owner.session_id.clone(),
                started,
            ));
            Ok(Some(Box::new(RecordingGuard {
                record: Arc::clone(&self.record),
                server,
            })))
        }
    }

    struct RecordingGuard {
        record: Arc<GuardRecord>,
        server: String,
    }

    impl RecordingGuard {
        fn finish(&self, call: &'static str, disposition: &meerkat_mcp::McpAttemptDisposition) {
            let process = match disposition.process {
                meerkat_mcp::McpAttemptProcessEnd::NotSpawned => "not spawned",
                meerkat_mcp::McpAttemptProcessEnd::Closed(_) => "closed",
                _ => "ended otherwise",
            };
            self.record
                .finished
                .lock()
                .unwrap()
                .push((self.server.clone(), call, process));
        }
    }

    #[async_trait]
    impl meerkat_mcp::McpAttemptGuard for RecordingGuard {
        async fn settle(
            self: Box<Self>,
            disposition: meerkat_mcp::McpAttemptDisposition,
        ) -> Result<(), meerkat_mcp::McpGuardFinishError> {
            self.finish("settle", &disposition);
            Ok(())
        }

        async fn cancel(
            self: Box<Self>,
            disposition: meerkat_mcp::McpAttemptDisposition,
        ) -> Result<(), meerkat_mcp::McpGuardFinishError> {
            self.finish("cancel", &disposition);
            Ok(())
        }
    }

    fn recording_guards(
        record: &Arc<GuardRecord>,
        probes: &std::path::Path,
    ) -> meerkat_mcp::McpAttemptGuards {
        meerkat_mcp::McpAttemptGuards::new(
            Arc::new(RecordingGuards {
                record: Arc::clone(record),
                probes: probes.to_path_buf(),
            }),
            meerkat_mcp::McpAttemptGuardBounds::default(),
        )
    }

    /// A stdio server whose process writes its probe (`probes/<name>`) as soon
    /// as it starts, then idles for two seconds without speaking MCP, so its
    /// connection fails after one second and its process ends soon after: a
    /// construction waiting for its MCP servers waits for that end.
    fn started_server(name: &str, probes: &std::path::Path) -> McpServerConfig {
        let mut server = McpServerConfig::stdio(
            name,
            "sh",
            vec![
                "-c".to_string(),
                format!(
                    "printf started > '{}'; exec sleep 2",
                    probes.join(name).display()
                ),
            ],
            std::collections::HashMap::new(),
        );
        server.connect_timeout_secs = Some(1);
        server
    }

    /// The host's attempt guard installation reaches every construction of
    /// the factory it composed: a first construction and a rebuild of the
    /// same session each begin the guard for their own server, with that
    /// session, before the server's process starts, and each guard is
    /// finished once, after its process has ended.
    #[tokio::test]
    async fn host_attempt_guards_begin_before_each_construction_spawns_its_server() {
        let dir = tempfile::tempdir().unwrap();
        let probes = dir.path().join("probes");
        std::fs::create_dir_all(&probes).unwrap();
        let machine = runtime_machine();
        let record = Arc::new(GuardRecord::default());
        let guards = recording_guards(&record, &probes);
        let recording = Arc::new(Recording {
            composed: Mutex::new(Vec::new()),
        });
        let host = composing_host(Answer::Guarded(Arc::clone(&recording), guards.clone()));
        let custody = crate::member_mcp_custody::MemberMcpCustody::new();
        let factory = composed(&host, dir.path(), &machine, &custody).expect("composed");
        let mut builder = FactoryAgentBuilder::new(factory, meerkat::Config::default());
        builder.default_llm_client = Some(Arc::new(BuildOnlyLlmClient));
        builder.wait_for_mcp = true;

        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
        let first = builder
            .build_agent(
                &request_with(vec![started_server("first", &probes)]),
                event_tx,
            )
            .await
            .unwrap_or_else(|error| panic!("first construction: {error}"));
        let session = first.agent().session().clone();
        let mut rebuild = request_with(vec![started_server("rebuild", &probes)]);
        rebuild
            .build
            .as_mut()
            .expect("build options")
            .resume_existing_session(session.clone());
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
        let _rebuilt = builder
            .build_agent(&rebuild, event_tx)
            .await
            .unwrap_or_else(|error| panic!("same-session rebuild: {error}"));

        assert_eq!(
            *record.begun.lock().unwrap(),
            [
                ("first".to_string(), session.id().clone(), false),
                ("rebuild".to_string(), session.id().clone(), false),
            ],
            "each construction begins the guard for its server and session before the process starts"
        );
        assert!(
            probes.join("first").exists() && probes.join("rebuild").exists(),
            "both servers started after their guard was begun"
        );
        assert_eq!(
            *recording.composed.lock().unwrap(),
            [session.id().clone(), session.id().clone()],
            "the host's composer still composes each construction"
        );

        // Closing the gateway's custody ends both processes.
        custody.settle().await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while record.finished.lock().unwrap().len() < 2 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let mut finished = record.finished.lock().unwrap().clone();
        finished.sort();
        assert_eq!(
            finished.len(),
            2,
            "each begun guard is finished exactly once: {finished:?}"
        );
        assert!(
            finished
                .iter()
                .zip(["first", "rebuild"])
                .all(|((server, _, process), expected)| server == expected
                    && *process != "not spawned"),
            "each guard is finished after its process ended: {finished:?}"
        );
        guards
            .close_and_join(tokio::time::Instant::now() + std::time::Duration::from_secs(5))
            .await;
    }

    /// A composition without attempt guards installs none: the factory
    /// carries no installation and its builder offers none to the session
    /// routers Meerkat's surfaces build. With them, both carry the host's.
    #[tokio::test]
    async fn attempt_guards_reach_the_factory_only_when_the_host_installs_them() {
        let dir = tempfile::tempdir().unwrap();
        let machine = runtime_machine();
        let recording = Arc::new(Recording {
            composed: Mutex::new(Vec::new()),
        });
        let custody = crate::member_mcp_custody::MemberMcpCustody::new();
        let plain = composed(
            &composing_host(Answer::Compose(Arc::clone(&recording))),
            dir.path(),
            &machine,
            &custody,
        )
        .expect("composed");
        assert!(plain.mcp_attempt_guards().is_none());
        let builder = FactoryAgentBuilder::new(plain, meerkat::Config::default());
        assert!(meerkat_session::McpAttemptGuardSource::mcp_attempt_guards(&builder).is_none());

        let guards = recording_guards(&Arc::new(GuardRecord::default()), dir.path());
        let guarded = composed(
            &composing_host(Answer::Guarded(recording, guards)),
            dir.path(),
            &machine,
            &custody,
        )
        .expect("composed");
        assert!(guarded.mcp_attempt_guards().is_some());
        let builder = FactoryAgentBuilder::new(guarded, meerkat::Config::default());
        assert!(meerkat_session::McpAttemptGuardSource::mcp_attempt_guards(&builder).is_some());
    }

    const SLOT: &str = "SEARCH_API_KEY";
    /// A test-only service secret value, never a real credential.
    const SECRET: &str = "sk-console-set-value-0d4c";

    fn secret(value: &str) -> McpServiceSecretValue {
        McpServiceSecretValue::new(value.to_owned())
    }

    /// The host's entry check in a test, standing in for the Console's: the
    /// caller is admitted while `admin` holds (a retired login or a removed
    /// administrator refuses). It counts its calls, and while an admission
    /// it granted is held, `held` is set.
    struct ConsoleAdmin {
        admin: std::sync::atomic::AtomicBool,
        calls: std::sync::atomic::AtomicUsize,
        held: Arc<std::sync::atomic::AtomicBool>,
    }

    impl ConsoleAdmin {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                admin: std::sync::atomic::AtomicBool::new(true),
                calls: std::sync::atomic::AtomicUsize::new(0),
                held: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            })
        }

        fn retire(&self) {
            self.admin.store(false, std::sync::atomic::Ordering::SeqCst);
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// Clears the check's `held` flag when the mutation releases the
    /// admission.
    struct HeldUntilSettled(Arc<std::sync::atomic::AtomicBool>);

    impl Drop for HeldUntilSettled {
        fn drop(&mut self) {
            self.0.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl AdministrationEntry for ConsoleAdmin {
        fn enter(&self) -> Result<meerkat_core::EnteredAdministration, AdministrationEntryRefusal> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if !self.admin.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(AdministrationEntryRefusal::NotAdministrator);
            }
            self.held.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(meerkat_core::EnteredAdministration::holding(
                HeldUntilSettled(Arc::clone(&self.held)),
            ))
        }
    }

    /// The entry check of a caller that stays the administrator.
    fn admin() -> Arc<dyn AdministrationEntry> {
        ConsoleAdmin::new()
    }

    /// A file credential store under `dir` with its own refresh authority:
    /// the production backend kind, never the operator's own store.
    fn credentials(dir: &std::path::Path) -> ProviderAuthPersistence {
        meerkat::TokenStoreBackend::File {
            root: dir.join("credentials"),
        }
        .open_with_refresh_authority()
        .expect("credential store")
    }

    /// Whether this process runs the body of the installation test `name`.
    ///
    /// The realm document's writer exclusion is one try: held custody refuses
    /// as not current. A child process that another test spawns holds copies
    /// of this process's descriptors, a just-released config lock included,
    /// until its exec, so a test that writes the realm document runs as the
    /// only test of a child process of its own.
    fn runs_isolated(name: &str) -> bool {
        crate::test_process_isolation::run_body_in_isolated_process(module_path!(), name)
    }

    /// A member factory over `persistence` composed for `storage`, and the
    /// realm MCP administration the gateway offered its host with it.
    fn composed_with_installation(
        dir: &std::path::Path,
        machine: &MeerkatMachine,
        storage: &ResolvedStorage,
        persistence: &ProviderAuthPersistence,
    ) -> (AgentFactory, GatewayRealmMcpInstallation) {
        let host = composing_host(Answer::Ordinary);
        let offered: Arc<dyn GatewayHostComposition> = host.clone();
        let factory = compose_gateway_member_factory(
            Some(&offered),
            GatewayFactoryKind::Persistent,
            AgentFactory::new(dir)
                .builtins(false)
                .comms(true)
                .with_provider_auth_persistence(persistence.clone()),
            machine,
            storage,
            &crate::member_mcp_custody::MemberMcpCustody::new(),
        )
        .expect("composed")
        .factory;
        let installation = host
            .installations
            .lock()
            .unwrap()
            .pop()
            .expect("offered with the factory");
        (factory, installation)
    }

    /// A stdio server declaring one service secret. Its process writes the
    /// value it was launched with to `probe`, then idles without speaking
    /// MCP, so its connection fails after one second.
    fn probe_server(name: &str, probe: &std::path::Path) -> McpServerConfig {
        let mut server = McpServerConfig::stdio(
            name,
            "sh",
            vec![
                "-c".to_string(),
                format!(
                    "printf %s \"$SEARCH_API_KEY\" > '{}'; sleep 30",
                    probe.display()
                ),
            ],
            std::collections::HashMap::new(),
        );
        server.connect_timeout_secs = Some(1);
        if let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) = &mut server.transport {
            stdio.secret_env.insert(SLOT.to_string());
        }
        server
    }

    /// Build one member of `builder` with `server` configured, waiting for
    /// its connection to settle, and answer what the server's process was
    /// launched with (`None`: it was never launched).
    async fn launched_with(
        mut builder: FactoryAgentBuilder,
        server: McpServerConfig,
        probe: &std::path::Path,
    ) -> Option<String> {
        builder.default_llm_client = Some(Arc::new(BuildOnlyLlmClient));
        builder.wait_for_mcp = true;
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
        let built = builder
            .build_agent(&request_with(vec![server]), event_tx)
            .await
            .unwrap_or_else(|error| panic!("member construction: {error}"));
        let notices = format!("{:?}", built.agent().session().messages());
        assert!(
            !notices.contains(SECRET),
            "a member's MCP notices never carry the value"
        );
        // A launched process writes its probe at once; the build returned
        // only after the connection settled, a second after the launch.
        for _ in 0..40 {
            match std::fs::read_to_string(probe) {
                Ok(value) if !value.is_empty() => return Some(value),
                _ => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }
        None
    }

    /// The host's Console path in its order: review the realm's own
    /// servers, install the reviewed definition at that generation, then set
    /// its secret. A member build of the definition launches it with the
    /// value, read from the factory's own credential store.
    #[tokio::test]
    async fn a_member_launch_reads_the_secret_set_for_the_installed_definition() {
        if !runs_isolated("a_member_launch_reads_the_secret_set_for_the_installed_definition") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let machine = Arc::new(runtime_machine());
        let storage = storage(dir.path());
        let persistence = credentials(dir.path());
        let (factory, installation) =
            composed_with_installation(dir.path(), &machine, &storage, &persistence);
        assert_eq!(installation.realm(), &realm());

        let probe = dir.path().join("launched-with");
        let reviewed = probe_server("search", &probe);
        let reference =
            meerkat_core::auth::McpServiceSecretRef::for_definition(realm(), &reviewed, SLOT)
                .expect("declared slot");
        let stored = || async {
            meerkat_core::auth::load_mcp_service_secret(
                persistence.token_store().as_ref(),
                &reference,
            )
            .await
            .expect("credential store")
        };
        let (listed, generation) = installation.servers().await.expect("review");
        assert!(listed.is_empty());
        assert!(
            matches!(
                installation
                    .set_secret(&reviewed, SLOT, secret(SECRET), admin())
                    .await,
                Err(McpServiceSecretUpdateError::DefinitionChanged(_))
            ),
            "a definition the realm's own document does not list holds no value"
        );
        assert_eq!(stored().await, None);
        let installed_at = installation
            .install(reviewed.clone(), generation, admin())
            .await
            .expect("installed");
        assert!(installed_at > generation);
        assert!(
            matches!(
                installation
                    .install(probe_server("other", &probe), generation, admin())
                    .await,
                Err(GatewayRealmMcpError::Persist(_))
            ),
            "an install checked against a stale review writes nothing"
        );
        assert_eq!(
            installation.servers().await.expect("review").0,
            vec![reviewed.clone()]
        );
        installation
            .set_secret(&reviewed, SLOT, secret(SECRET), admin())
            .await
            .expect("stored for the installed definition");
        assert_eq!(
            stored().await.as_deref(),
            Some(SECRET),
            "the value is in the factory's own credential store"
        );

        let builder =
            gateway_member_builder(factory, meerkat::Config::default(), &machine, &storage);
        assert_eq!(
            launched_with(builder, reviewed, &probe).await.as_deref(),
            Some(SECRET)
        );
    }

    /// Controls: no launch reads a value for a definition the realm's own
    /// document does not list (an unlisted name, or a changed definition
    /// under the listed one), and a builder without the realm credential
    /// source, as every gateway member builder was before, launches nothing
    /// even for the installed definition the gateway's builder launches.
    #[tokio::test]
    async fn no_launch_reads_a_secret_without_the_listed_definition_and_the_realm_source() {
        if !runs_isolated(
            "no_launch_reads_a_secret_without_the_listed_definition_and_the_realm_source",
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let machine = Arc::new(runtime_machine());
        let storage = storage(dir.path());
        let persistence = credentials(dir.path());
        let (_, installation) =
            composed_with_installation(dir.path(), &machine, &storage, &persistence);
        let installed_probe = dir.path().join("installed");
        let installed = probe_server("search", &installed_probe);
        let (_, generation) = installation.servers().await.expect("review");
        installation
            .install(installed.clone(), generation, admin())
            .await
            .expect("installed");
        installation
            .set_secret(&installed, SLOT, secret(SECRET), admin())
            .await
            .expect("stored");
        let member_factory = || {
            AgentFactory::new(dir.path())
                .builtins(false)
                .comms(true)
                .with_provider_auth_persistence(persistence.clone())
        };
        let gateway_builder = || {
            gateway_member_builder(
                member_factory(),
                meerkat::Config::default(),
                &machine,
                &storage,
            )
        };

        let unlisted = dir.path().join("unlisted");
        assert_eq!(
            launched_with(
                gateway_builder(),
                probe_server("unlisted", &unlisted),
                &unlisted
            )
            .await,
            None,
            "an unlisted name has no defining realm"
        );
        let changed = dir.path().join("changed");
        assert_eq!(
            launched_with(
                gateway_builder(),
                probe_server("search", &changed),
                &changed
            )
            .await,
            None,
            "a changed definition under the listed name is not the current one"
        );
        assert_eq!(
            launched_with(
                FactoryAgentBuilder::new(member_factory(), meerkat::Config::default()),
                installed.clone(),
                &installed_probe
            )
            .await,
            None,
            "without the realm source no custody serves the slot"
        );
        assert_eq!(
            launched_with(gateway_builder(), installed, &installed_probe)
                .await
                .as_deref(),
            Some(SECRET),
            "the same definition through the gateway's member builder"
        );
    }

    /// A changed document refuses the write, and no value appears in the
    /// realm's own document, in a refusal or in the handle's `Debug`.
    #[tokio::test]
    async fn a_changed_definition_refuses_the_write_and_no_value_is_shown() {
        if !runs_isolated("a_changed_definition_refuses_the_write_and_no_value_is_shown") {
            return;
        }
        const SECOND: &str = "sk-console-set-second-value-0d4c";
        let dir = tempfile::tempdir().unwrap();
        let machine = Arc::new(runtime_machine());
        let storage = storage(dir.path());
        let persistence = credentials(dir.path());
        let (_, installation) =
            composed_with_installation(dir.path(), &machine, &storage, &persistence);
        let reviewed = probe_server("search", &dir.path().join("reviewed"));
        let not_installed = installation
            .set_secret(&reviewed, SLOT, secret(SECRET), admin())
            .await
            .expect_err("not installed");
        let (_, generation) = installation.servers().await.expect("review");
        installation
            .install(reviewed.clone(), generation, admin())
            .await
            .expect("installed");
        installation
            .set_secret(&reviewed, SLOT, secret(SECRET), admin())
            .await
            .expect("stored");

        // Another writer replaces the definition under the same name.
        let document = realm_document(&storage).expect("the realm's own document");
        let config = meerkat_core::ConfigRuntime::new(
            Arc::new(meerkat_core::FileConfigStore::new(
                document.clone(),
                meerkat_models::canonical(),
            )),
            document.with_file_name("config_state.json"),
        );
        let target = meerkat_core::McpRealmPersistTarget::new(&config);
        target.remove("search", None).await.expect("removed");
        target
            .add(
                probe_server("search", &dir.path().join("replacement")),
                None,
            )
            .await
            .expect("replaced");
        let changed = installation
            .set_secret(&reviewed, SLOT, secret(SECOND), admin())
            .await
            .expect_err("the reviewed definition is no longer listed");
        assert!(matches!(
            changed,
            McpServiceSecretUpdateError::DefinitionChanged(_)
        ));
        let current_removal = installation
            .remove_secret(&reviewed, SLOT, McpServiceSecretRemoval::Current, admin())
            .await
            .expect_err("not the current definition");
        installation
            .remove_secret(&reviewed, SLOT, McpServiceSecretRemoval::Obsolete, admin())
            .await
            .expect("the replaced definition's value is cleaned up");

        for shown in [
            std::fs::read_to_string(&document).expect("document"),
            format!("{not_installed} {not_installed:?}"),
            format!("{changed} {changed:?}"),
            format!("{current_removal} {current_removal:?}"),
            format!("{installation:?}"),
        ] {
            assert!(
                !shown.contains(SECRET) && !shown.contains(SECOND),
                "no value is shown: {shown}"
            );
        }
    }

    /// C2, at the mutation entry: a Console administrator retired while an
    /// install or a secret write waits for its custody is refused at the
    /// mutation's entry, with nothing written; a check made before the wait
    /// would have admitted it. An administrator still current at the entry
    /// is admitted, and the admission is released once the write settled.
    #[tokio::test]
    async fn an_administrator_retired_while_a_mutation_waits_is_refused_at_its_entry() {
        if !runs_isolated("an_administrator_retired_while_a_mutation_waits_is_refused_at_its_entry")
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let machine = Arc::new(runtime_machine());
        let storage = storage(dir.path());
        let persistence = credentials(dir.path());
        let (_, installation) =
            composed_with_installation(dir.path(), &machine, &storage, &persistence);
        let reviewed = probe_server("search", &dir.path().join("reviewed"));
        let (_, generation) = installation.servers().await.expect("review");

        // An install waits behind another writer's custody of the document.
        let document = realm_document(&storage).expect("the realm's own document");
        let held = meerkat_core::config_runtime::ConfigWriterLock::try_hold(
            &document.with_file_name("config_state.lock"),
        )
        .unwrap()
        .expect("config writer custody");
        let retired = ConsoleAdmin::new();
        let install = tokio::spawn({
            let (installation, reviewed) = (installation.clone(), reviewed.clone());
            let entry = Arc::clone(&retired) as Arc<dyn AdministrationEntry>;
            async move { installation.install(reviewed, generation, entry).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!install.is_finished(), "the install waits for the custody");
        assert_eq!(retired.calls(), 0, "nothing is admitted before the custody");
        retired.retire();
        drop(held);
        assert!(matches!(
            install.await.unwrap(),
            Err(GatewayRealmMcpError::NotAdmitted(
                AdministrationEntryRefusal::NotAdministrator
            ))
        ));
        assert_eq!(retired.calls(), 1);
        assert_eq!(
            installation.servers().await.expect("review"),
            (Vec::new(), generation),
            "nothing was installed"
        );

        // A current administrator installs; a secret write then waits behind
        // the credential owner of its exact reference.
        let current = ConsoleAdmin::new();
        installation
            .install(
                reviewed.clone(),
                generation,
                Arc::clone(&current) as Arc<dyn AdministrationEntry>,
            )
            .await
            .expect("admitted while current");
        assert!(!current.held.load(std::sync::atomic::Ordering::SeqCst));
        let reference =
            meerkat_core::auth::McpServiceSecretRef::for_definition(realm(), &reviewed, SLOT)
                .expect("declared slot");
        let (holding, custody_held) = tokio::sync::oneshot::channel::<()>();
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let coordinator = persistence.refresh_coordinator();
        let key = meerkat_core::auth::TokenKey::mcp_service_secret(&reference);
        let holder = tokio::spawn(async move {
            coordinator
                .with_exclusive_mutation_for(
                    key,
                    meerkat_core::auth::TokenStoreAccess::Interactive,
                    Box::new(move || {
                        Box::pin(async move {
                            let _ = holding.send(());
                            let _ = released.await;
                            Ok(meerkat_core::auth::CredentialMutationOutcome::Cleared)
                        })
                    }),
                )
                .await
        });
        custody_held.await.unwrap();
        let retired = ConsoleAdmin::new();
        let write = tokio::spawn({
            let (installation, reviewed) = (installation.clone(), reviewed.clone());
            let entry = Arc::clone(&retired) as Arc<dyn AdministrationEntry>;
            async move {
                installation
                    .set_secret(&reviewed, SLOT, secret(SECRET), entry)
                    .await
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!write.is_finished(), "the write waits for the custody");
        assert_eq!(retired.calls(), 0, "nothing is admitted before the custody");
        retired.retire();
        release.send(()).unwrap();
        holder.await.unwrap().unwrap();
        assert!(matches!(
            write.await.unwrap(),
            Err(McpServiceSecretUpdateError::NotAdmitted(_))
        ));
        assert_eq!(retired.calls(), 1);
        assert_eq!(
            meerkat_core::auth::load_mcp_service_secret(
                persistence.token_store().as_ref(),
                &reference
            )
            .await
            .expect("credential store"),
            None,
            "nothing was stored"
        );

        // Still current at the entry: stored, the admission released after.
        installation
            .set_secret(
                &reviewed,
                SLOT,
                secret(SECRET),
                Arc::clone(&current) as Arc<dyn AdministrationEntry>,
            )
            .await
            .expect("admitted while current");
        assert!(!current.held.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            meerkat_core::auth::load_mcp_service_secret(
                persistence.token_store().as_ref(),
                &reference
            )
            .await
            .expect("credential store")
            .as_deref(),
            Some(SECRET)
        );
    }

    /// The host's handle to the gateway's member MCP custody is the custody
    /// every construction of this gateway adopts its adapters into, so a
    /// credential landed through it reaches the members' own routers.
    #[tokio::test]
    async fn the_construction_hands_the_host_the_gateways_own_member_custody() {
        let dir = tempfile::tempdir().unwrap();
        let machine = Arc::new(runtime_machine());
        let storage = storage(dir.path());
        let custody = crate::member_mcp_custody::MemberMcpCustody::new();
        let host = composing_host(Answer::Ordinary);
        let offered: Arc<dyn GatewayHostComposition> = host.clone();
        for kind in [
            GatewayFactoryKind::Persistent,
            GatewayFactoryKind::ProcessLocal,
        ] {
            compose_gateway_member_factory(
                Some(&offered),
                kind,
                AgentFactory::new(dir.path()).builtins(false).comms(true),
                &machine,
                &storage,
                &custody,
            )
            .expect("composed");
        }
        let handed = host.custodies.lock().unwrap().clone();
        assert_eq!(handed.len(), 2, "one handle per construction");
        assert!(
            handed.iter().all(|each| Arc::ptr_eq(each, &custody)),
            "every construction hands out the gateway's own custody"
        );
    }

    /// An OAuth MCP server the realm's configuration defines resolves, from
    /// the host's wire target, to the native identity Meerkat builds from
    /// that configuration: the identity the realm's credential slot and the
    /// members' connections use. Anything the configuration does not define
    /// exactly is refused typed, and nothing is resolved from the request.
    #[tokio::test]
    async fn a_configured_oauth_server_resolves_to_its_native_target() {
        if !runs_isolated("a_configured_oauth_server_resolves_to_its_native_target") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let machine = Arc::new(runtime_machine());
        let storage = storage(dir.path());
        let persistence = credentials(dir.path());
        let (_factory, installation) =
            composed_with_installation(dir.path(), &machine, &storage, &persistence);
        let mut hosted = McpServerConfig::streamable_http(
            "hosted",
            "https://mcp.example.invalid/mcp",
            std::collections::HashMap::new(),
        );
        if let meerkat_core::mcp_config::McpTransportConfig::Http(http) = &mut hosted.transport {
            http.oauth_account = Some("subject-7".to_string());
        }
        let wire =
            |name: &str, url: &str, account: Option<&str>| meerkat_contracts::WireMcpAuthTarget {
                server_name: name.to_string(),
                server_url: url.to_string(),
                oauth_account: account.map(str::to_string),
                oauth_account_selection: None,
            };
        // Not configured yet: refused, never invented from the request.
        assert!(matches!(
            installation
                .resolve_mcp_auth_target(&wire(
                    "hosted",
                    "https://mcp.example.invalid/mcp",
                    Some("subject-7")
                ))
                .await,
            Err(HostAuthError::McpTarget(
                meerkat::HostMcpTargetRefusal::UnknownServer { .. }
            ))
        ));
        let (_, generation) = installation.servers().await.expect("review");
        installation
            .install(hosted.clone(), generation, admin())
            .await
            .expect("installed");

        let resolved = installation
            .resolve_mcp_auth_target(&wire(
                "hosted",
                "https://mcp.example.invalid/mcp",
                Some("subject-7"),
            ))
            .await
            .expect("a configured server resolves");
        assert_eq!(
            resolved,
            meerkat::McpServerIdentity::from_config(&hosted).expect("identity"),
            "the identity comes from the configuration"
        );
        assert!(matches!(
            installation
                .resolve_mcp_auth_target(&wire("hosted", "https://other.example.invalid/mcp", None))
                .await,
            Err(HostAuthError::McpTarget(
                meerkat::HostMcpTargetRefusal::UrlMismatch { .. }
            ))
        ));
        assert!(matches!(
            installation
                .resolve_mcp_auth_target(&wire(
                    "hosted",
                    "https://mcp.example.invalid/mcp",
                    Some("subject-8")
                ))
                .await,
            Err(HostAuthError::McpTarget(
                meerkat::HostMcpTargetRefusal::AccountMismatch { .. }
            ))
        ));
        assert!(matches!(
            installation
                .resolve_mcp_auth_target(&wire("absent", "https://mcp.example.invalid/mcp", None))
                .await,
            Err(HostAuthError::McpTarget(
                meerkat::HostMcpTargetRefusal::UnknownServer { .. }
            ))
        ));
    }
}
