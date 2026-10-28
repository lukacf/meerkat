//! Governed posture for a realm opened by a fresh process (stock rkat and
//! embedding hosts).
//!
//! The trusted host selects the posture before the realm opens:
//! [`RealmGovernance`]. It is never inferred from whether a grant file exists.
//! For a governed realm the facade opens the realm exactly as an ungoverned
//! one (manifest admission and reservation, the store set, durability and
//! session-profile checks, every subsystem store), then restores the realm's
//! grant owner from [`meerkat_store::grant_owner_store_path`] and builds the realm's sole
//! machine from the host's owners through
//! [`ScopedGrantComposition::into_configuration`].
//!
//! When the owners are not available (the grant owner is missing, unreadable,
//! locked, corrupt or incompatible, or the host selected governance without
//! supplying its owners) the realm still opens governed. Its machine refuses
//! locally and typed, before any MCP discovery or spawn
//! ([`MeerkatMachine::persistent_with_unavailable_scoped_governance`]). No
//! grant is issued, no owner or store is created in its place, and the realm
//! never falls back to ungoverned behavior or to a different realm.
//!
//! The grant owner supplies grants only. The originals source, the invocation
//! policy, the operation owner and the controller come from the trusted host;
//! none of them is derivable from stored grants.
//!
//! A governed realm with restored owners admits a configured MCP server only
//! when it is one of the realm's own persisted installations (written by
//! `rkat mcp add --scope realm`) that carries an access ceiling, exactly as
//! installed, while the realm's config document is still at the generation
//! read when the realm opened. The server's connection then enforces the
//! ceiling at every invocation. Any other server, an installation without a
//! ceiling, an inherited one, and every server once the document has moved
//! to another generation (until the realm is reopened) are refused locally
//! before discovery or spawn.

pub mod helper_admission;
pub mod helper_child_issuance;
pub mod staged;
pub mod stock;
pub mod stock_host;

/// Whether `refusal`, met at a synchronous governed effect leaf, is a
/// denial. Such a leaf runs under an owner-change exclusion it must not wait
/// in, so a refusal to rebuild is no verdict on the operation: the leaf
/// answers unavailable, and a later attempt rebuilds from current owners. A
/// policy denial and malformed facts stay denials, and so does any refusal
/// kind this version does not know.
pub(crate) fn effect_leaf_denies(refusal: meerkat_core::authorization::OperationRefused) -> bool {
    !matches!(
        refusal.kind(),
        meerkat_core::authorization::OperationRefusalKind::ReprepareRequired
    )
}

/// Whether `error`, met at a governed effect leaf on a tool dispatch's own
/// path, is a review settlement that is final for that dispatch: a review
/// this entry cannot carry, a review already spent or a dispatch already
/// ended, or a review that refused or was retired. Such a settlement is a
/// denial, as on the prepared path. It is final for this dispatch only: a
/// new call is reviewed anew. Only the review owner's own unavailability
/// (contention, an unconfirmed store, unreadable material, a failed
/// reviewer) leaves a later attempt a chance, and is not a denial; neither
/// is any error that is not a review settlement. Every unavailable kind is
/// named here, so a new kind is classified on purpose. The error is read as
/// returned, like the refusal arms next to this check.
pub(crate) fn review_settlement_denies(error: &meerkat_core::ToolError) -> bool {
    use meerkat_core::ReviewUnavailableKind as Unavailable;
    match error {
        meerkat_core::ToolError::ReviewUnsatisfied { .. } => true,
        meerkat_core::ToolError::ReviewUnavailable { kind } => match kind {
            Unavailable::DeadlineExpired
            | Unavailable::ReviewerMissing
            | Unavailable::UnsupportedEntry
            | Unavailable::DispatchEnded => true,
            Unavailable::OwnerUnavailable
            | Unavailable::ReviewerFailed
            | Unavailable::StoreConflict
            | Unavailable::StoreCommitUnknown
            | Unavailable::ContextUnavailable => false,
        },
        _ => false,
    }
}

use std::path::{Path, PathBuf};
use std::sync::Arc;

use meerkat_authorization::clock::LocalAuthorizationClock;
use meerkat_authorization::grant_policy::OperationPolicyOwner;
use meerkat_authorization::grants::{
    FileGrantOwnerStore, GrantDurabilityError, LocalGrantAuthority,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::constraints::ResourceDomain;
use meerkat_core::BlobStore;
use meerkat_core::mcp_attachment::{ConfiguredMcpAttachmentAuthority, McpAttachmentRefusal};
use meerkat_core::{ConfigRuntime, FileConfigStore, McpRealmPersistTarget, McpServerConfig};
use meerkat_runtime::meerkat_machine::{
    AuthenticatedOriginalSource, NativeRetainedResumeCheck, ScopedGrantComposition,
    ScopedInvocation,
};
use meerkat_runtime::{MeerkatMachine, RuntimeDriverError, RuntimeStore};
use meerkat_store::{RealmBackend, RealmOrigin, RealmPaths, grant_owner_store_path};

use crate::PersistenceBundle;
use crate::persistence::PersistenceError;

/// The trusted host's owners for a governed realm. Every field comes from the
/// host's own composition; none is read from the realm's stores.
pub struct GovernedRealmBootstrap {
    /// The installed ingress authority and namespace of scoped originals.
    pub ingress_domain: ResourceDomain,
    /// The source owner's custody of authenticated originals.
    pub originals: Arc<dyn AuthenticatedOriginalSource>,
    /// One concrete invocation policy, both faces from the same `Arc`.
    pub invocation: ScopedInvocation,
    /// Current account, resource, sink and controller rules.
    pub operation_owner: Arc<dyn OperationPolicyOwner>,
    /// Governed retained-work resume: the check that admits a continuation of
    /// an earlier run (the approved notice of consent parked at that run's
    /// close, a fork_off or council completion) as a resume of exactly that
    /// run. Without it the realm admits no retained resume: such a
    /// continuation stays pending (`UnsupportedScope`), or is refused once
    /// its run's destination is superseded. Build it over the
    /// same originals custody and invocation policy as above with
    /// `ScopedRetainedResume`, as `ScopedGrantComposition::with_retained_resume`
    /// does. [`RealmGovernance::Stock`] builds its own.
    pub retained_resume: Option<Arc<NativeRetainedResumeCheck>>,
    /// The restarted host's fresh publication, shared with its other owners.
    pub publication: LocalAuthorizationPublication,
    pub clock: Arc<dyn LocalAuthorizationClock>,
}

/// The posture the trusted host selected for a realm, before it opens.
pub enum RealmGovernance {
    /// No native governance: the ordinary persistent machine.
    Ungoverned,
    /// Governed, with the host's owners; the grant owner is restored.
    Governed(Box<GovernedRealmBootstrap>),
    /// Governed by stock rkat's own host composition (see [`stock`]); the
    /// grant owner is restored and the stock owners are built over it.
    Stock(Arc<stock::StockGovernance>),
    /// Governed, but the host has no owners to supply. Every governed effect
    /// is refused locally as unavailable.
    GovernedWithoutOwners,
}

impl std::fmt::Debug for RealmGovernance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Ungoverned => "Ungoverned",
            Self::Governed(_) => "Governed",
            Self::Stock(_) => "Stock",
            Self::GovernedWithoutOwners => "GovernedWithoutOwners",
        })
    }
}

/// The realm's governed composition, read from its restored owners before
/// its machine is built: the grant owner restored, the realm's own MCP
/// installations read and, for stock governance, the agent's grant selection
/// snapshotted. Built once per open.
pub(crate) enum GovernedComposition {
    /// No native governance: the ordinary persistent machine.
    Ungoverned,
    /// Governed, with owners that are unavailable now.
    Unavailable,
    /// Governed, with the restored native/grant composition.
    LocalGrant {
        configuration: meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration,
        installed: Result<Arc<RealmInstalledMcp>, meerkat_core::McpRealmPersistError>,
        per_agent: Option<(
            meerkat_runtime::meerkat_machine::AgentInstallationGrant,
            Option<Arc<dyn ConfiguredMcpAttachmentAuthority>>,
        )>,
    },
}

/// Read the governed composition of `governance` for the realm at `paths`.
pub(crate) async fn governed_composition(
    governance: RealmGovernance,
    review_read: Option<Result<(), stock::StockGovernanceError>>,
    paths: &RealmPaths,
) -> GovernedComposition {
    let (publication, clock) = match &governance {
        RealmGovernance::Ungoverned => return GovernedComposition::Ungoverned,
        RealmGovernance::GovernedWithoutOwners => {
            tracing::warn!(
                "governed realm opened without host owners: governed work and configured \
                 MCP attachments are unavailable"
            );
            return GovernedComposition::Unavailable;
        }
        RealmGovernance::Governed(bootstrap) => {
            (bootstrap.publication.clone(), Arc::clone(&bootstrap.clock))
        }
        RealmGovernance::Stock(stock) => (
            stock.authorization_publication(),
            Arc::new(meerkat_authorization::clock::HostAuthorizationClock)
                as Arc<dyn LocalAuthorizationClock>,
        ),
    };
    let store = Arc::new(FileGrantOwnerStore::new(grant_owner_store_path(
        &paths.root,
    )));
    let grants = match LocalGrantAuthority::restore(store, publication, clock) {
        Ok(grants) => Arc::new(grants),
        Err(error) => {
            tracing::warn!(
                reason = restore_failure(&error),
                error = %error,
                "governed realm's grant owner did not restore: governed work and configured \
                 MCP attachments are unavailable"
            );
            return GovernedComposition::Unavailable;
        }
    };
    let installed = RealmInstalledMcp::read(paths).await;
    // The stock composition decides configured MCP servers per agent too.
    let mut per_agent = None;
    let composition = match governance {
        RealmGovernance::Governed(bootstrap) => {
            let bootstrap = *bootstrap;
            ScopedGrantComposition {
                grants,
                ingress_domain: bootstrap.ingress_domain,
                originals: bootstrap.originals,
                invocation: bootstrap.invocation,
                operation_owner: bootstrap.operation_owner,
                retained_resume: bootstrap.retained_resume,
            }
        }
        RealmGovernance::Stock(stock) => {
            // The realm owner's review rules as this open's own opener read
            // them, carried here by that open alone; when no opener read them
            // (a host's own stores), read now from the realm's effective
            // configuration under its realms root. A read that failed, or
            // rules other than those this composition already serves, make it
            // unavailable: never another document's rules, another open's
            // read or an earlier open's rules.
            let read = match review_read {
                Some(read) => read,
                None => configure_stock_review_at(&stock, paths).await,
            };
            if let Err(error) = read {
                tracing::warn!(
                    error = %error,
                    "stock governed realm's review rules are unavailable: governed work and \
                     configured MCP attachments are unavailable"
                );
                return GovernedComposition::Unavailable;
            }
            let installations = match &installed {
                Ok(installed) => {
                    stock.record_installed_mcp_servers(installed.servers().to_vec());
                    Arc::clone(installed) as Arc<dyn ConfiguredMcpAttachmentAuthority>
                }
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "stock governed realm's MCP installations could not be read: governed \
                         work and configured MCP attachments are unavailable"
                    );
                    return GovernedComposition::Unavailable;
                }
            };
            let agent_grant = meerkat_runtime::meerkat_machine::AgentInstallationGrant::new(
                Arc::clone(&grants),
                Arc::new(meerkat_authorization::clock::HostAuthorizationClock),
                stock.principals().operator.clone(),
            );
            match stock.composition(grants, installations, paths).await {
                Ok(composition) => {
                    // The composition's own operation policy for attaching a
                    // configured server; absent, every one is unavailable.
                    per_agent = Some((agent_grant, stock.configured_mcp_policy()));
                    composition
                }
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "stock governed realm's owners are unavailable: governed work and \
                         configured MCP attachments are unavailable"
                    );
                    return GovernedComposition::Unavailable;
                }
            }
        }
        RealmGovernance::Ungoverned | RealmGovernance::GovernedWithoutOwners => {
            return GovernedComposition::Unavailable;
        }
    };
    GovernedComposition::LocalGrant {
        configuration: composition.into_configuration(),
        installed,
        per_agent,
    }
}

/// The realm's sole machine for the selected posture.
async fn governed_machine(
    governance: RealmGovernance,
    review_read: Option<Result<(), stock::StockGovernanceError>>,
    runtime_store: Arc<dyn RuntimeStore>,
    blob_store: Arc<dyn BlobStore>,
    paths: RealmPaths,
) -> Result<MeerkatMachine, RuntimeDriverError> {
    match governed_composition(governance, review_read, &paths).await {
        GovernedComposition::Ungoverned => MeerkatMachine::persistent(runtime_store, blob_store),
        GovernedComposition::Unavailable => {
            MeerkatMachine::persistent_with_unavailable_scoped_governance(
                runtime_store,
                Some(blob_store),
            )
        }
        GovernedComposition::LocalGrant {
            configuration,
            installed,
            per_agent,
        } => match installed {
            Ok(installed) => match per_agent {
                Some((agent_grant, policy)) => {
                    MeerkatMachine::persistent_with_local_grant_authorization_and_agent_mcp(
                        runtime_store,
                        Some(blob_store),
                        configuration,
                        installed,
                        agent_grant,
                        policy,
                    )
                }
                None => {
                    MeerkatMachine::persistent_with_local_grant_authorization_and_configured_mcp(
                        runtime_store,
                        Some(blob_store),
                        configuration,
                        installed,
                    )
                }
            },
            Err(error) => {
                // The governed machine still opens; its configured servers are
                // all refused as unavailable (the governed host's default).
                warn_installations_unreadable(&error);
                MeerkatMachine::persistent_with_local_grant_authorization(
                    runtime_store,
                    Some(blob_store),
                    configuration,
                )
            }
        },
    }
}

/// Install a governed composition on the realm's staged machine, with the
/// same host choice as [`governed_machine`] makes for a fresh open.
pub(crate) fn install_governed_composition(
    staged: meerkat_runtime::StagedMeerkatMachine,
    composition: GovernedComposition,
) -> Result<MeerkatMachine, RuntimeDriverError> {
    match composition {
        // A staged machine is governed by construction.
        GovernedComposition::Ungoverned | GovernedComposition::Unavailable => {
            staged.install_unavailable_scoped_governance()
        }
        GovernedComposition::LocalGrant {
            configuration,
            installed,
            per_agent,
        } => match installed {
            Ok(installed) => match per_agent {
                Some((agent_grant, policy)) => staged
                    .install_local_grant_authorization_and_agent_mcp(
                        configuration,
                        installed,
                        agent_grant,
                        policy,
                    ),
                None => staged
                    .install_local_grant_authorization_and_configured_mcp(configuration, installed),
            },
            Err(error) => {
                warn_installations_unreadable(&error);
                staged.install_local_grant_authorization(configuration)
            }
        },
    }
}

fn warn_installations_unreadable(error: &meerkat_core::McpRealmPersistError) {
    tracing::warn!(
        error = %error,
        "governed realm's MCP installations could not be read: configured MCP \
         attachments are unavailable"
    );
}

/// The realm config document's generation sidecar, beside the document.
const CONFIG_STATE_FILE: &str = "config_state.json";

/// The realm's own persisted MCP installations, at the generation of its
/// config document read when the realm opened. Servers it inherits from a
/// parent realm or a `mcp.toml` file are not its installations.
pub struct RealmInstalledMcp {
    generation: u64,
    servers: Vec<McpServerConfig>,
    document_path: PathBuf,
    state_path: PathBuf,
}

impl RealmInstalledMcp {
    /// The realm's own installations, as the config document held them when
    /// they were read.
    pub fn servers(&self) -> &[McpServerConfig] {
        &self.servers
    }

    async fn read(paths: &RealmPaths) -> Result<Arc<Self>, meerkat_core::McpRealmPersistError> {
        let state_path = paths.config_path.with_file_name(CONFIG_STATE_FILE);
        // An absent document reads as one with no servers. This store only
        // reads, so it composes no realm chain: no write goes through it.
        let runtime = ConfigRuntime::new(
            Arc::new(FileConfigStore::new(
                paths.config_path.clone(),
                meerkat_models::canonical(),
            )),
            state_path.clone(),
        );
        let (servers, generation) = McpRealmPersistTarget::new(&runtime).servers().await?;
        Ok(Arc::new(Self {
            generation,
            servers,
            document_path: paths.config_path.clone(),
            state_path,
        }))
    }

    /// Whether `server` is exactly one of the realm's own installations with
    /// an explicit operation ceiling, as the realm's config document holds it
    /// now and as it was when the realm opened.
    ///
    /// The decision takes the config document's shared read custody (which
    /// excludes every `ConfigRuntime` writer of this document, in any
    /// process, but not other reads) without waiting, and compares the actual
    /// current document and its generation under it. A mutation in progress
    /// refuses as not current, and a document changed by a write whose
    /// generation publication failed no longer matches, so a removed or
    /// changed installation is never certified by an old generation. The
    /// custody is released when the decision returns, before any connection
    /// work.
    pub fn check(&self, server: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        let installed = self
            .servers
            .iter()
            .find(|installed| installed.name == server.name)
            .filter(|installed| *installed == server)
            .filter(|installed| installed.access.is_some() && installed.validate_access().is_ok());
        if installed.is_none() {
            return Err(McpAttachmentRefusal::Unavailable);
        }
        let _exclusion = ConfigWriterExclusion::try_read(&self.state_path)?;
        #[cfg(test)]
        read_custody_tests::hold_after_custody(&self.state_path);
        match config_generation(&self.state_path) {
            Some(current) if current == self.generation => {}
            Some(_) => return Err(McpAttachmentRefusal::NotCurrent),
            None => return Err(McpAttachmentRefusal::Unavailable),
        }
        let current = document_servers(&self.document_path)?;
        if current.iter().any(|installed| installed == server) {
            Ok(())
        } else {
            Err(McpAttachmentRefusal::NotCurrent)
        }
    }
}

/// The config document's writer exclusion: custody of the lock
/// `ConfigRuntime` writers take exclusively beside the generation sidecar,
/// so writers in every process are excluded while it is held, however long.
/// A currentness decision holds it shared
/// ([`meerkat_core::config_runtime::ConfigReadLock`]), so decisions never
/// exclude each other; an administrative command that acts on the snapshot
/// holds it exclusively ([`meerkat_core::config_runtime::ConfigWriterLock`]).
/// Either is taken without waiting: a held incompatible custody is a
/// mutation whose outcome is not known, and the caller refuses.
pub(crate) struct ConfigWriterExclusion {
    _read: Option<meerkat_core::config_runtime::ConfigReadLock>,
    _exclusive: Option<meerkat_core::config_runtime::ConfigWriterLock>,
}

impl ConfigWriterExclusion {
    fn try_read(state_path: &Path) -> Result<Self, McpAttachmentRefusal> {
        match meerkat_core::config_runtime::ConfigReadLock::try_hold(
            &state_path.with_extension("lock"),
        ) {
            Ok(Some(custody)) => Ok(Self {
                _read: Some(custody),
                _exclusive: None,
            }),
            Ok(None) => Err(McpAttachmentRefusal::NotCurrent),
            Err(_) => Err(McpAttachmentRefusal::Unavailable),
        }
    }

    fn try_exclusive(state_path: &Path) -> Result<Self, McpAttachmentRefusal> {
        match meerkat_core::config_runtime::ConfigWriterLock::try_hold(
            &state_path.with_extension("lock"),
        ) {
            Ok(Some(custody)) => Ok(Self {
                _read: None,
                _exclusive: Some(custody),
            }),
            Ok(None) => Err(McpAttachmentRefusal::NotCurrent),
            Err(_) => Err(McpAttachmentRefusal::Unavailable),
        }
    }
}

/// The realm's own installation named `name`, read from its config document
/// under the document's exclusive custody, which the returned guard keeps
/// holding: no config writer or other administrative command in any process
/// changes the installation while the caller acts on this snapshot. Held
/// custody (a write or a read in progress) refuses as not current. A
/// document listing the name more than once installs its last entry, the
/// one the composition makes effective.
pub(crate) fn installation_under_writer_exclusion(
    paths: &RealmPaths,
    name: &str,
) -> Result<(ConfigWriterExclusion, Option<McpServerConfig>), McpAttachmentRefusal> {
    installation_under_writer_exclusion_in(&paths.config_path, name)
        .map(|(exclusion, installed, _)| (exclusion, installed))
}

/// [`installation_under_writer_exclusion`] for the config document at
/// `document` (the user-global document for the reserved `global` realm),
/// also returning the document's generation read under the same exclusion
/// (`None` when it cannot be read).
pub(crate) fn installation_under_writer_exclusion_in(
    document: &Path,
    name: &str,
) -> Result<(ConfigWriterExclusion, Option<McpServerConfig>, Option<u64>), McpAttachmentRefusal> {
    let state_path = document.with_file_name(CONFIG_STATE_FILE);
    let exclusion = ConfigWriterExclusion::try_exclusive(&state_path)?;
    let installed = document_servers(document)?
        .into_iter()
        .rfind(|server| server.name == name);
    Ok((exclusion, installed, config_generation(&state_path)))
}

/// The realm document's own `[[tools.mcp_servers]]`, read now. An absent
/// document has none.
fn document_servers(path: &Path) -> Result<Vec<McpServerConfig>, McpAttachmentRefusal> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(McpAttachmentRefusal::Unavailable),
    };
    let document: toml::Table =
        toml::from_str(&content).map_err(|_| McpAttachmentRefusal::Unavailable)?;
    let Some(servers) = document
        .get("tools")
        .and_then(|tools| tools.get("mcp_servers"))
    else {
        return Ok(Vec::new());
    };
    servers
        .clone()
        .try_into()
        .map_err(|_| McpAttachmentRefusal::Unavailable)
}

impl ConfiguredMcpAttachmentAuthority for RealmInstalledMcp {
    fn authorize_configured(&self, server: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        self.check(server)
    }
}

/// The config document's current generation; an absent sidecar is
/// generation 0, as for the config runtime. `None` when it cannot be read.
fn config_generation(state_path: &Path) -> Option<u64> {
    #[derive(serde::Deserialize)]
    struct ConfigState {
        generation: u64,
    }
    match std::fs::read_to_string(state_path) {
        Ok(raw) => serde_json::from_str::<ConfigState>(&raw)
            .ok()
            .map(|state| state.generation),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(0),
        Err(_) => None,
    }
}

fn restore_failure(error: &GrantDurabilityError) -> &'static str {
    match error {
        GrantDurabilityError::Missing => "missing",
        GrantDurabilityError::RestartRequired => "corrupt or incompatible",
        GrantDurabilityError::StoreUnavailable => "unreadable or locked",
        _ => "unavailable",
    }
}

/// Open a realm through `provider` with the posture the trusted host selected.
/// Storage and admission failures are the ordinary [`PersistenceError`]s; an
/// unavailable governed owner is not one (see the module docs).
pub async fn open_governed_realm_persistence_with_provider(
    provider: &dyn crate::storage_provider::RealmStorageProvider,
    realms_root: &std::path::Path,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
    layout: Option<meerkat_core::StorageLayout>,
    governance: RealmGovernance,
) -> Result<(meerkat_store::RealmManifestPin, PersistenceBundle), PersistenceError> {
    let stock = match &governance {
        RealmGovernance::Stock(stock) => Some(Arc::clone(stock)),
        _ => None,
    };
    // The realm owner's review rules, from the realm's effective
    // configuration as every host reads it, configured on the stock
    // composition before it is built. This open's own read alone decides its
    // composition, carried to it through this open's machine build and
    // nowhere another open could change it: rules that cannot be read, or
    // that differ from the rules this composition already serves, make it
    // unavailable, so its governed work is refused rather than served under
    // another document's, another open's or an earlier open's rules.
    let review_read = match &stock {
        Some(stock) => {
            let resolved = match &layout {
                Some(layout) => layout.clone(),
                None => crate::persistence::layout_for_explicit_state_root(realms_root, realm_id)?,
            };
            Some(configure_stock_review(stock, realms_root, realm_id, &resolved).await)
        }
        None => None,
    };
    let ordinary = matches!(governance, RealmGovernance::Ungoverned);
    let (pin, mut bundle) = crate::persistence::open_realm_persistence_with_provider_and_machine(
        provider,
        realms_root,
        realm_id,
        backend_hint,
        origin_hint,
        layout,
        |runtime_store, blob_store, paths| {
            governed_machine(governance, review_read, runtime_store, blob_store, paths)
        },
    )
    .await?;
    if ordinary {
        // An ordinary machine never runs a governed realm (see
        // `refuse_governed_posture`); the bundle drops with the error.
        crate::persistence::refuse_governed_posture(realms_root, realm_id).await?;
    }
    bundle.set_stock_governance(stock);
    Ok((pin, bundle))
}

/// Configure `stock` with the review rules of its realm's effective
/// configuration, under the realms root of `paths` and that root's explicit
/// layout.
async fn configure_stock_review_at(
    stock: &stock::StockGovernance,
    paths: &RealmPaths,
) -> Result<(), stock::StockGovernanceError> {
    let realms_root = paths.root.parent().ok_or_else(|| {
        stock::StockGovernanceError::Review("the realm has no realms root".to_string())
    })?;
    let realm_id = stock.realm().as_str().to_string();
    let layout = crate::persistence::layout_for_explicit_state_root(realms_root, &realm_id)
        .map_err(|error| stock::StockGovernanceError::Review(error.to_string()))?;
    configure_stock_review(stock, realms_root, &realm_id, &layout).await
}

/// Configure `stock` with the review rules of `realm_id`'s effective
/// configuration.
pub(crate) async fn configure_stock_review(
    stock: &stock::StockGovernance,
    realms_root: &std::path::Path,
    realm_id: &str,
    layout: &meerkat_core::StorageLayout,
) -> Result<(), stock::StockGovernanceError> {
    let realm = meerkat_core::connection::RealmId::parse(realm_id).map_err(|_| {
        stock::StockGovernanceError::Review(format!("'{realm_id}' is not a realm id"))
    })?;
    let rules = stock::review::read_realm_review_rules(realms_root, &realm, layout)
        .await
        .map_err(|error| stock::StockGovernanceError::Review(error.to_string()))?;
    stock.configure_review(rules)
}

/// Open a realm as stock rkat does: by its durable posture. A governed realm
/// opens with the stock composition for this process's local operator (or,
/// when that operator cannot be determined, governed without owners); any
/// other realm opens ordinarily. The ordinary open re-reads the posture
/// while it holds the realm's execution custody and refuses a realm governed
/// in the meantime ([`PersistenceError::RealmGoverned`]); governing takes
/// exclusive custody before publishing the posture, so the posture an open
/// proceeds with holds for as long as its bundle lives. An unreadable
/// posture is an error, never ordinary. Without `layout` the realm opens
/// under the single-candidate layout of an explicit state root.
pub async fn open_stock_realm_persistence(
    provider: &dyn crate::storage_provider::RealmStorageProvider,
    realms_root: &std::path::Path,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
    layout: Option<meerkat_core::StorageLayout>,
) -> Result<(meerkat_store::RealmManifestPin, PersistenceBundle), PersistenceError> {
    open_stock_realm_persistence_as(
        provider,
        realms_root,
        realm_id,
        backend_hint,
        origin_hint,
        layout,
        stock::issuance::DEFAULT_STOCK_AGENT,
    )
    .await
}

/// [`open_stock_realm_persistence`] acting as the named agent: a governed
/// realm's stock composition takes that agent's executor and the grants the
/// operator selected for it. An ordinary realm ignores the agent. An agent
/// name that is not a valid principal opens governed without owners, so its
/// governed work is refused.
pub async fn open_stock_realm_persistence_as(
    provider: &dyn crate::storage_provider::RealmStorageProvider,
    realms_root: &std::path::Path,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
    layout: Option<meerkat_core::StorageLayout>,
    agent: &str,
) -> Result<(meerkat_store::RealmManifestPin, PersistenceBundle), PersistenceError> {
    open_stock_realm_persistence_in(
        provider,
        realms_root,
        realm_id,
        backend_hint,
        origin_hint,
        layout,
        agent,
        None,
    )
    .await
}

/// [`open_stock_realm_persistence_as`] for a trusted host that authenticates
/// its own callers: a governed realm opens with the host-authenticated stock
/// composition ([`stock::StockGovernance::host_authenticated_for_agent`]),
/// whose requesters are the callers the host binds
/// ([`stock::StockGovernance::bind_caller_prompt`]), each admitted by
/// `access` for the exact session it invokes, through the host's own
/// `ingress_actor`. This process's local operator stays the canonical owner
/// of every session (and the root of the realm's grants) and is no
/// requester. The bundle carries the composition
/// ([`PersistenceBundle::stock_governance`]); the host joins its access
/// facts to its publication before exposing any writer of them. An ordinary
/// realm opens ordinarily, exactly as [`open_stock_realm_persistence_as`]
/// opens it; an operator, agent or ingress actor that is not a valid
/// principal opens governed without owners.
#[allow(clippy::too_many_arguments)]
pub async fn open_stock_realm_persistence_for_host(
    provider: &dyn crate::storage_provider::RealmStorageProvider,
    realms_root: &std::path::Path,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
    layout: Option<meerkat_core::StorageLayout>,
    agent: &str,
    ingress_actor: meerkat_core::PrincipalRef,
    access: Arc<dyn stock::caller::StockCallerAccess>,
) -> Result<(meerkat_store::RealmManifestPin, PersistenceBundle), PersistenceError> {
    open_stock_realm_persistence_in(
        provider,
        realms_root,
        realm_id,
        backend_hint,
        origin_hint,
        layout,
        agent,
        Some((ingress_actor, access)),
    )
    .await
}

/// The posture-aware stock open, for the local operator (`host` absent) or
/// for a trusted host's ingress actor and caller access.
#[allow(clippy::too_many_arguments)]
async fn open_stock_realm_persistence_in(
    provider: &dyn crate::storage_provider::RealmStorageProvider,
    realms_root: &std::path::Path,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
    layout: Option<meerkat_core::StorageLayout>,
    agent: &str,
    host: Option<(
        meerkat_core::PrincipalRef,
        Arc<dyn stock::caller::StockCallerAccess>,
    )>,
) -> Result<(meerkat_store::RealmManifestPin, PersistenceBundle), PersistenceError> {
    let layout = match layout {
        Some(layout) => layout,
        None => crate::persistence::layout_for_explicit_state_root(realms_root, realm_id)?,
    };
    let posture = meerkat_store::read_realm_posture_in(realms_root, realm_id).await?;
    if posture != meerkat_store::RealmPosture::Governed {
        match crate::persistence::open_realm_persistence_with_provider(
            provider,
            realms_root,
            realm_id,
            backend_hint,
            origin_hint,
            Some(layout.clone()),
        )
        .await
        {
            // Governed between the read and the open's custody: the ordinary
            // bundle was dropped; open it governed instead.
            Err(PersistenceError::RealmGoverned { .. }) => {}
            opened => return opened,
        }
    }
    let realm = meerkat_core::connection::RealmId::parse(realm_id).map_err(|_| {
        PersistenceError::RealmGoverned {
            realm_id: realm_id.to_string(),
        }
    })?;
    let governance = match stock::StockPrincipals::for_local_operator(&realm).and_then(
        |mut principals| match host {
            None => stock::StockGovernance::new_for_agent(realm, principals, agent),
            Some((ingress_actor, access)) => {
                ingress_actor.validate_qualified().map_err(|_| {
                    stock::StockGovernanceError::Principal(
                        "the host's ingress actor is not a qualified principal".to_string(),
                    )
                })?;
                principals.ingress_actor = ingress_actor;
                stock::StockGovernance::host_authenticated_for_agent(
                    realm, principals, agent, access,
                )
            }
        },
    ) {
        Ok(stock) => RealmGovernance::Stock(stock),
        Err(error) => {
            tracing::warn!(
                error = %error,
                "governed realm's local operator or agent is unavailable: governed work is refused"
            );
            RealmGovernance::GovernedWithoutOwners
        }
    };
    open_governed_realm_persistence_with_provider(
        provider,
        realms_root,
        realm_id,
        backend_hint,
        origin_hint,
        Some(layout),
        governance,
    )
    .await
}

#[cfg(test)]
mod read_custody_tests;

#[cfg(test)]
mod review_load_tests;

#[cfg(all(test, feature = "memory-store-session"))]
mod memory_grant_tests;
