//! Realm, config, factory and session-service construction.
//!
//! The TUI hosts its sessions on Meerkat's runtime-backed path, the one the
//! product surfaces (CLI, REST, RPC) use: a [`PersistentSessionService`] over
//! the realm's stores, with a [`MeerkatMachine`] owning input admission
//! (Queue/Steer), interruption, terminal publication and resume. Both halves
//! come from [`build_runtime_backed_service_with_default_reconfigure_host`],
//! which also installs the session-LLM reconfigure host (the model hot swap
//! of a later phase relies on it).
//!
//! 1. [`prepare`] resolves the [`RuntimeScope`] and loads the realm [`Config`]
//!    (for `--isolated` this mints a fresh realm id, so call it once).
//! 2. [`bootstrap_with`] opens the realm persistence and composes the service.
//!
//! [`bootstrap`] runs both back to back.
//!
//! [`bootstrap_with`] also builds the approval engine ([`approval::build`]):
//! every session the TUI opens installs its gate as the build's
//! `ToolDispatchAdmission` (see [`crate::session`]).
//!
//! It also lowers `--confinement` into the shell's OS confinement
//! ([`confinement::plan`], installed with `AgentFactory::with_shell_confinement`),
//! discovers the LLM endpoints ([`crate::endpoints`]), picks the start-up
//! model ([`plan_model`]) and loads the price table.
//!
//! The factory reads the realm's own `config.toml` through its owner
//! ([`Runtime::realm_config`], see [`realm_config`]) on every build. The
//! configured endpoints are projected into that document as realm
//! connections ([`crate::endpoints::project_endpoints`]), and models only an
//! endpoint knows get a `[models.<id>]` profile there
//! ([`Runtime::ensure_profile`]), so a session's auth binding and model
//! resolve for `rkat`, REST and RPC on the same realm too.
//!
//! With the `mob` feature it hosts helpers ([`crate::mobs`]): the factory's
//! mob tools are on, the builder's default mob-tools slot carries the mob
//! host's tool factory, and every helper spawn passes the TUI's customizer
//! (approval gate, model binding, file tools).
//!
//! Context ([`instructions`], [`memory_probe`], [`mcp`]): a user-global
//! `SYSTEM.md` becomes the config's `system_prompt_file` when the realm sets
//! no prompt; the factory runs with native semantic memory (`memory_search`,
//! compaction-time indexing) when the realm's index opens; the effective
//! MCP set (realm `[[tools.mcp_servers]]`, project and user `mcp.toml`) is
//! loaded once and given to every session build as its declarative servers.

pub mod approval;
pub mod confinement;
pub mod credentials;
pub mod dirs;
pub mod instructions;
pub mod mcp;
pub mod persona;
pub mod realm_config;
pub mod scope;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use meerkat::surface::build_runtime_backed_service_with_default_reconfigure_host;
use meerkat::{
    AgentFactory, FactoryAgentBuilder, LlmClient, PersistentSessionService, SessionStore,
};
use meerkat_core::connection::RealmId;
use meerkat_core::{AuthBindingRef, Config, ConfigStore, McpServerConfig, Provider};
use meerkat_runtime::MeerkatMachine;
use meerkat_store::{RealmBackend, realm_paths_in};
use meerkat_tools::builtin::shell::ShellConfinement;

use crate::cli::Args;
use crate::endpoints::{
    EndpointError, EndpointName, EndpointRef, EndpointRegistry, ModelRefError, StartupPlan,
    StartupRequest, ensure_model_profile, needs_lookup, plan_startup, project_endpoints,
};
use crate::error::TuiError;
use crate::mobs::Helpers;
use crate::models::{ModelCache, ModelEntry, ModelLister, live_lister};
use crate::pricing::PricingSetup;
#[cfg(feature = "mob")]
use crate::tools::FileToolDispatcher;
#[cfg(feature = "mob")]
use meerkat_core::AgentToolDispatcher;

pub use approval::{ApprovalOverrides, ApprovalParts};
pub use confinement::{ConfinementPlan, HostPaths};
pub use dirs::GlobalDirs;
pub use instructions::InstructionSources;
pub use persona::{CODING_PERSONA, HELPER_PERSONA};
pub use realm_config::RealmConfigStore;
pub use scope::{RuntimeScope, load_config, resolve_scope};

/// Live sessions one TUI process may hold (the primary session plus a few
/// retiring ones while `/new` and `/resume` switch).
const MAX_SESSIONS: usize = 8;

/// File in the realm directory where the session-LLM reconfigure host keeps
/// its state (the same name the REST surface uses).
const CONFIG_STATE_FILE: &str = "config_state.json";

/// The session service the TUI drives.
pub type TuiService = PersistentSessionService<FactoryAgentBuilder>;

/// The model a session runs on, and where it comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    /// Endpoint name (`anthropic`, `litellm`, …).
    pub endpoint: EndpointName,
    /// Model id as the endpoint knows it.
    pub model: String,
    /// The provider whose client speaks the endpoint's dialect.
    pub provider: Provider,
    /// The explicit auth binding of a configured endpoint (`None`: default
    /// resolution, the built-in endpoints).
    pub auth_binding: Option<AuthBindingRef>,
}

impl ModelChoice {
    /// The choice as an `endpoint:model` reference.
    #[must_use]
    pub fn reference(&self) -> EndpointRef {
        EndpointRef::new(self.endpoint.as_str(), self.model.clone())
    }
}

/// Everything a running TUI needs from the engine.
pub struct Runtime {
    /// Project, conventions roots and realm.
    pub scope: RuntimeScope,
    /// Realm config with env overrides applied, as loaded at start-up.
    pub config: Config,
    /// The realm document the factory reads on every build (see
    /// [`realm_config`]); endpoints and profiles are written into it.
    pub realm_config: Arc<RealmConfigStore>,
    /// The effective config as of the last profile change: the model
    /// registry the context gauge and the compaction policy read windows
    /// from.
    pub effective: Arc<parking_lot::RwLock<Config>>,
    /// Bumped whenever [`Runtime::ensure_profile`] adds a profile. An agent
    /// captures its model registry when it is built, so a session built at
    /// an older generation is rebuilt before it can switch to a new model.
    pub profile_generation: Arc<AtomicU64>,
    /// Model of new sessions.
    pub choice: ModelChoice,
    /// The LLM endpoints of this process.
    pub endpoints: EndpointRegistry,
    /// Model lists fetched during start-up, if any.
    pub models: Option<ModelCache>,
    /// Fetches the endpoints' model lists.
    pub lister: ModelLister,
    /// The price table and where to refresh it.
    pub pricing: PricingSetup,
    /// The shell's OS confinement.
    pub confinement: ConfinementPlan,
    /// Session lifecycle substrate.
    pub service: Arc<TuiService>,
    /// Runtime control plane: input admission, interrupt, registrations.
    pub machine: Arc<MeerkatMachine>,
    /// Realm session store (for listing persisted sessions).
    pub store: Arc<dyn SessionStore>,
    /// Backend pinned in the realm manifest.
    pub backend: RealmBackend,
    /// The approval engine and the channels the runner serves.
    pub approval: ApprovalParts,
    /// The helper (mob member) host; `None` without the `mob` feature.
    pub helpers: Option<Arc<Helpers>>,
    /// Where the instruction files of every session build come from.
    pub instructions: InstructionSources,
    /// The declarative MCP servers every session build carries (reloaded
    /// by `/mcp trust`).
    pub mcp_servers: Vec<McpServerConfig>,
    /// Whether sessions get native semantic memory (`memory_search`).
    pub memory: bool,
    /// Start-up notices (warnings) for the transcript.
    pub notices: Vec<String>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("scope", &self.scope)
            .field("choice", &self.choice)
            .field("endpoints", &self.endpoints.names())
            .field("confinement", &self.confinement.mode)
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

impl Runtime {
    /// The choice for `reference` on this process's endpoints.
    ///
    /// # Errors
    ///
    /// [`TuiError::Endpoint`] when the endpoint is unknown.
    pub fn choice_for(&self, reference: &EndpointRef) -> Result<ModelChoice, TuiError> {
        choice_for(&self.endpoints, reference)
    }

    /// Make sure the factory knows a capability profile for `model` on
    /// `provider`: a `[models.<id>]` entry in the realm document when only
    /// the endpoint knows it, refreshed from `listed` (see
    /// [`ensure_model_profile`]).
    ///
    /// # Errors
    ///
    /// [`TuiError::Endpoint`] when the profile cannot be written (or the
    /// id is already profiled for another dialect).
    pub async fn ensure_profile(
        &self,
        provider: Provider,
        model: &str,
        listed: Option<&ModelEntry>,
    ) -> Result<(), TuiError> {
        if ensure_model_profile(self.realm_config.document(), provider, model, listed).await? {
            let effective = self
                .realm_config
                .get()
                .await
                .map_err(EndpointError::ConfigRead)?;
            *self.effective.write() = effective;
            self.profile_generation.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }

    /// The current [`Runtime::profile_generation`].
    #[must_use]
    pub fn profiles(&self) -> u64 {
        self.profile_generation.load(Ordering::SeqCst)
    }
}

/// The choice for `reference` on `endpoints`.
fn choice_for(
    endpoints: &EndpointRegistry,
    reference: &EndpointRef,
) -> Result<ModelChoice, TuiError> {
    let endpoint =
        endpoints
            .get(&reference.endpoint)
            .ok_or_else(|| {
                EndpointError::from(ModelRefError::UnknownEndpoint {
                    name: reference.endpoint.clone(),
                    known: endpoints.names(),
                })
            })?;
    Ok(ModelChoice {
        endpoint: EndpointName::new(endpoint.name.clone()),
        model: reference.model.clone(),
        provider: endpoint.provider(),
        auth_binding: endpoint.binding(),
    })
}

/// Engine-level overrides for [`bootstrap_with`].
#[derive(Default, Clone)]
pub struct BootstrapOptions {
    /// Serve every session with this client instead of resolving provider
    /// credentials (tests, or a host that already built a client). With an
    /// override the user's provider token store is not opened, the
    /// classifier runs on the same client, and every built-in endpoint
    /// exists (keyless, so its list is not fetched).
    pub llm_override: Option<Arc<dyn LlmClient>>,
    /// Approval-engine overrides (classifier, escalation timeout).
    pub approval: ApprovalOverrides,
    /// Fetch model lists with this instead of the live endpoints.
    pub lister: Option<ModelLister>,
    /// The directories outside the workspace a confined shell may use
    /// (default: [`HostPaths::detect`]).
    pub host_paths: Option<HostPaths>,
}

/// The shell's extra grants: the command line's `--confinement-read`/
/// `--confinement-write`, then the `[confinement]` grants of the user's own
/// `tui.toml` layers (which yield to the flags, see
/// [`confinement::grants_with`]).
fn extra_grants(args: &Args, layered: &crate::approval::rules::Layered) -> confinement::ExtraGrants {
    let mut extra = confinement::ExtraGrants::from_args(args);
    extra.extend_from(&layered.confinement);
    extra
}

/// Resolve the runtime scope and load the realm config.
///
/// # Errors
///
/// See [`resolve_scope`] and [`load_config`].
pub async fn prepare(args: &Args) -> Result<(RuntimeScope, Config), TuiError> {
    let scope = resolve_scope(args)?;
    let config = load_config(&scope).await?;
    Ok((scope, config))
}

/// [`prepare`] followed by [`bootstrap_with`].
///
/// # Errors
///
/// See [`prepare`] and [`bootstrap_with`].
pub async fn bootstrap(args: &Args, options: BootstrapOptions) -> Result<Runtime, TuiError> {
    let (scope, config) = prepare(args).await?;
    bootstrap_with(scope, config, args, options).await
}

/// Open the realm persistence and compose the runtime-backed service.
///
/// # Errors
///
/// [`TuiError::Endpoint`] when no start-up model can be chosen,
/// [`TuiError::Confinement`] when `--confinement` cannot be enforced,
/// [`TuiError::Persistence`] when the realm cannot be opened,
/// [`TuiError::Overlay`] / [`TuiError::Approval`] when the approval rules or
/// ledger cannot be loaded, and [`TuiError::Composition`] when the runtime
/// refuses the composition.
pub async fn bootstrap_with(
    scope: RuntimeScope,
    config: Config,
    args: &Args,
    options: BootstrapOptions,
) -> Result<Runtime, TuiError> {
    let (mut layered, mut approval_notices) = approval::load_layers(&scope, args)?;
    let realm = RealmId::parse(scope.realm_id.clone()).ok();
    let (mut endpoints, endpoint_notices) =
        EndpointRegistry::discover(&config, realm.as_ref(), &layered.endpoints)?;
    if options.llm_override.is_some() {
        endpoints.add_keyless_builtins();
    }
    let lister = options.lister.clone().unwrap_or_else(live_lister);
    let (choice, models) = plan_model(args, &config, &endpoints, &lister).await?;

    let realm_config = Arc::new(RealmConfigStore::new(
        &scope.config_path(),
        scope.global.clone(),
    ));
    project_endpoints(realm_config.document(), endpoints.as_slice()).await?;
    let listed = models
        .as_ref()
        .and_then(|cache| cache.entry(choice.endpoint.as_str(), &choice.model));
    ensure_model_profile(
        realm_config.document(),
        choice.provider,
        &choice.model,
        listed,
    )
    .await?;
    let effective = realm_config
        .get()
        .await
        .map_err(EndpointError::ConfigRead)?;

    let host_paths = options.host_paths.clone().unwrap_or_else(HostPaths::detect);
    let confinement = confinement::plan(
        args.effective_confinement(),
        scope.project_root.as_path(),
        &config,
        &host_paths,
        &extra_grants(args, &layered),
    )?;

    let realm_root = realm_paths_in(&scope.state_root, &scope.realm_id).root;
    let (manifest, persistence) = meerkat::open_realm_persistence_in(
        &scope.state_root,
        &scope.realm_id,
        scope.backend_hint,
        Some(scope.origin),
    )
    .await
    .map_err(|source| TuiError::PersistenceOpen {
        realm_root: realm_root.clone(),
        state_root: scope.state_root.clone(),
        realm_id: scope.realm_id.clone(),
        source: Box::new(source),
    })?;
    let store = persistence.session_store();
    let store_path = persistence
        .store_path()
        .map_or_else(|| realm_root.clone(), PathBuf::from);

    let mut notices: Vec<String> = scope
        .global
        .as_ref()
        .and_then(|global| {
            dirs::legacy_config_notice(
                global,
                args.config_dir.as_deref(),
                scope.user_config_root.as_deref(),
            )
        })
        .into_iter()
        .collect();
    let memory = memory_probe(&store_path, manifest.backend, &mut notices);
    let factory = build_factory(
        &scope,
        store_path,
        FactoryToggles {
            token_store: options.llm_override.is_none(),
            memory,
        },
        &confinement.shell,
    );
    let pricing = PricingSetup::load(
        std::mem::take(&mut layered.pricing),
        scope.global.as_ref().map(GlobalDirs::pricing_cache),
        !args.no_pricing_fetch,
    );
    approval_notices.extend(endpoint_notices.iter().map(ToString::to_string));
    let approval = approval::build(
        &scope,
        layered,
        approval_notices,
        approval::ClassifierSource {
            factory: &factory,
            config: &effective,
            provider: choice.provider,
            auth_binding: choice.auth_binding.clone(),
            llm_override: options.llm_override.clone(),
        },
        &options.approval,
        persistence.approval_issuer(),
    )
    .await?;
    let project_mcp =
        mcp::ProjectMcpTrust::of(&scope, &approval.trust.lock(), args.allow_project_mcp);
    let mcp = mcp::load(&scope, &config, project_mcp).await?;
    notices.extend(mcp.notices);
    let mut builder = FactoryAgentBuilder::new_with_config_store(
        factory,
        effective.clone(),
        Arc::clone(&realm_config) as Arc<dyn ConfigStore>,
    );
    builder.default_llm_client.clone_from(&options.llm_override);
    builder.wait_for_mcp = args.wait_for_mcp;
    let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
    let workgraph = meerkat::WorkGraphService::with_scope(
        persistence.workgraph_store(),
        scope.realm_id.clone(),
        meerkat::WorkNamespace::default(),
    );
    let (service, machine) = build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        MAX_SESSIONS,
        persistence,
        realm_root.join(CONFIG_STATE_FILE),
    )?;
    // Every gate judges a call by where the session's tool of that name
    // comes from (its live tool definitions), never by the name alone.
    approval
        .engine
        .set_tool_origins(Arc::new(crate::approval::ServiceToolOrigins::new(&service)));
    let effective = Arc::new(parking_lot::RwLock::new(effective));
    let helpers = start_helpers(
        &effective,
        &service,
        &machine,
        &scope,
        &approval.engine,
        options.llm_override,
        &mob_tools_slot,
        workgraph,
        confinement.shell_env.clone(),
    )?;

    let scope_workspace = scope.context_root.clone();
    let scope_global = scope.global.clone();
    Ok(Runtime {
        scope,
        config,
        realm_config,
        effective,
        profile_generation: Arc::new(AtomicU64::new(0)),
        choice,
        endpoints,
        models,
        lister,
        pricing,
        confinement,
        service,
        machine,
        store,
        backend: manifest.backend,
        approval,
        helpers,
        instructions: InstructionSources {
            workspace: scope_workspace,
            global: scope_global,
            home: dirs_home(),
        },
        mcp_servers: mcp.servers,
        memory,
        notices,
    })
}

/// The canonical home directory, if any.
fn dirs_home() -> Option<PathBuf> {
    ::dirs::home_dir().map(|home| std::fs::canonicalize(&home).unwrap_or(home))
}

/// Directory of the realm's semantic-memory index (where the factory opens
/// its `HnswMemoryStore`).
#[must_use]
pub fn memory_dir(store_path: &std::path::Path) -> PathBuf {
    store_path.join("memory")
}

/// Whether sessions get native semantic memory: the realm keeps files (not
/// an in-memory backend) and its index opens. The index is opened once
/// here so a failure (an unwritable or damaged index) degrades
/// to a start-up notice instead of failing every session build.
fn memory_probe(
    store_path: &std::path::Path,
    backend: RealmBackend,
    notices: &mut Vec<String>,
) -> bool {
    if backend == RealmBackend::Memory {
        return false;
    }
    let dir = memory_dir(store_path);
    match meerkat_memory::HnswMemoryStore::open(&dir) {
        Ok(_) => true,
        Err(err) => {
            notices.push(format!(
                "semantic memory is off for this run (memory_search unavailable, compaction does not index): cannot open {}: {err}",
                dir.display()
            ));
            false
        }
    }
}

/// The slot every session build takes its mob tools from.
type MobToolsSlot = Arc<std::sync::RwLock<Option<Arc<dyn meerkat_core::service::MobToolsFactory>>>>;

/// Host helpers on the composed service and offer their tools to every
/// session build (see [`crate::mobs`]).
#[cfg(feature = "mob")]
#[allow(clippy::too_many_arguments)]
fn start_helpers(
    effective: &Arc<parking_lot::RwLock<Config>>,
    service: &Arc<TuiService>,
    machine: &Arc<MeerkatMachine>,
    scope: &RuntimeScope,
    engine: &Arc<crate::approval::ApprovalEngine>,
    llm_override: Option<Arc<dyn LlmClient>>,
    slot: &MobToolsSlot,
    workgraph: meerkat::WorkGraphService,
    shell_env: Option<HashMap<String, String>>,
) -> Result<Option<Arc<Helpers>>, TuiError> {
    let root = scope.project_root.clone();
    let tools: crate::mobs::customizer::HelperTools = Arc::new(move || {
        Arc::new(FileToolDispatcher::new(root.clone(), None)) as Arc<dyn AgentToolDispatcher>
    });
    let (helpers, factory) = Helpers::start(crate::mobs::HelperHostParts {
        service: Arc::clone(service),
        machine: Arc::clone(machine),
        realm_root: scope.realm_root(),
        engine: Arc::clone(engine),
        tools,
        llm_override,
        workgraph,
        shell_env,
        profiles: model_profiles(effective),
    })?;
    *slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(factory);
    Ok(Some(helpers))
}

/// The providers of the realm's `[models.<id>]` profiles as of the last
/// profile change ([`Runtime::ensure_profile`] refreshes `effective`).
#[cfg(feature = "mob")]
fn model_profiles(effective: &Arc<parking_lot::RwLock<Config>>) -> crate::mobs::ModelProfiles {
    let effective = Arc::clone(effective);
    Arc::new(move |model: &str| {
        effective
            .read()
            .models
            .custom
            .get(model)
            .map(|profile| profile.provider)
    })
}

/// Without the `mob` feature no helpers are hosted.
#[cfg(not(feature = "mob"))]
#[allow(clippy::unnecessary_wraps, clippy::too_many_arguments)]
fn start_helpers(
    _effective: &Arc<parking_lot::RwLock<Config>>,
    _service: &Arc<TuiService>,
    _machine: &Arc<MeerkatMachine>,
    _scope: &RuntimeScope,
    _engine: &Arc<crate::approval::ApprovalEngine>,
    _llm_override: Option<Arc<dyn LlmClient>>,
    _slot: &MobToolsSlot,
    _workgraph: meerkat::WorkGraphService,
    _shell_env: Option<HashMap<String, String>>,
) -> Result<Option<Arc<Helpers>>, TuiError> {
    Ok(None)
}

/// The agent factory every session is built with.
///
/// `project_root` and `context_root` are the workspace, so the factory's
/// native `AGENTS.md`, skills and hooks discovery see the project. Builtin
/// tools and the shell are on: every call passes the approval gate each
/// session installs, and the shell runs under the OS confinement the caller
/// installs ([`confinement::plan`]). Mob tools are on with the `mob` feature
/// (helpers, see [`crate::mobs`]); semantic memory when `toggles.memory`
/// (the index lives under `store_path`, see [`memory_dir`]). The file
/// and note tools come in per session as external tools (see
/// [`crate::session`]). The factory's `runtime_root` is the realm, so every
/// shell call runs in durable process custody (`<realm>/process_custody`),
/// which the runtime-backed composition settles on every attachment.
/// `with_token_store` attaches the user's provider token store, so OAuth and
/// stored keys resolve like they do for `rkat`.
/// Optional factory capabilities.
#[derive(Debug, Clone, Copy)]
struct FactoryToggles {
    /// Attach the user's provider token store.
    token_store: bool,
    /// Native semantic memory (`memory_search`, compaction indexing).
    memory: bool,
}

fn build_factory(
    scope: &RuntimeScope,
    store_path: PathBuf,
    toggles: FactoryToggles,
    shell_confinement: &ShellConfinement,
) -> AgentFactory {
    let mut factory = AgentFactory::new(store_path)
        .with_shell_confinement(shell_confinement.clone())
        .runtime_root(scope.realm_root())
        .project_root(scope.project_root.as_path())
        .context_root(scope.context_root.clone())
        .builtins(true)
        .shell(true)
        .memory(toggles.memory)
        .mob(cfg!(feature = "mob"));
    if let Some(user_root) = &scope.user_config_root {
        factory = factory.user_config_root(user_root.clone());
    }
    if toggles.token_store {
        factory = match factory.clone().with_default_token_store() {
            Ok(with_store) => with_store,
            Err(err) => {
                tracing::warn!(error = %err, "provider token store unavailable; using environment keys only");
                factory
            }
        };
    }
    factory
}

/// The model of new sessions, and the lists fetched to choose it.
///
/// `--model` (`endpoint:id` or a bare id), `--endpoint`/`--provider` and the
/// realm config's `agent.model` are planned with [`plan_startup`]; the
/// endpoints' lists are fetched first when a bare `--model` id could belong
/// to several endpoints, or when the plan asks for an endpoint's first listed
/// model.
///
/// # Errors
///
/// [`TuiError::Endpoint`] when no endpoint is configured, the reference does
/// not resolve, or the endpoint to take the first model of lists none.
pub async fn plan_model(
    args: &Args,
    config: &Config,
    endpoints: &EndpointRegistry,
    lister: &ModelLister,
) -> Result<(ModelChoice, Option<ModelCache>), TuiError> {
    let request = StartupRequest {
        model: args.model.as_deref(),
        endpoint: args.endpoint.as_deref().or(args.provider.as_deref()),
        config_model: &config.agent.model,
    };
    let fetch = || async {
        let listable = endpoints.iter().filter(|e| e.has_key()).cloned().collect();
        ModelCache::from_results(lister(listable).await).0
    };
    let mut cache = None;
    if request.endpoint.is_none()
        && request
            .model
            .is_some_and(|model| needs_lookup(model, endpoints.as_slice()))
    {
        cache = Some(fetch().await);
    }
    let index = cache.as_ref().map(ModelCache::index).unwrap_or_default();
    let reference = match plan_startup(request, endpoints.as_slice(), &index)
        .map_err(EndpointError::from)?
    {
        StartupPlan::Resolved(reference) => reference,
        StartupPlan::FirstListed(name) => {
            if cache.is_none() {
                cache = Some(fetch().await);
            }
            let first = cache
                .as_ref()
                .and_then(|cache| cache.first(&name))
                .ok_or_else(|| {
                    let why = cache
                        .as_ref()
                        .and_then(|cache| cache.get(&name))
                        .and_then(|list| list.as_ref().err().map(ToString::to_string))
                        .unwrap_or_else(|| "it lists no models".to_string());
                    TuiError::from(EndpointError::NothingListed {
                        endpoint: name.clone(),
                        why,
                    })
                })?;
            EndpointRef::new(name, first.id.clone())
        }
    };
    Ok((choice_for(endpoints, &reference)?, cache))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::endpoints::{WireDialect, Endpoint, SecretString};
    use crate::models::{EndpointListing, ModelSource};
    use clap::Parser;

    fn args(extra: &[&str]) -> Args {
        let mut argv = vec!["rkat-tui"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv).unwrap()
    }

    fn ep(name: &str, dialect: WireDialect, builtin: bool) -> Endpoint {
        Endpoint {
            name: name.to_string(),
            dialect,
            base_url: (!builtin).then(|| "http://127.0.0.1:4000".to_string()),
            api_key: SecretString::new("k"),
            key_env: None,
            builtin,
            priority: 0,
        }
    }

    fn entry(id: &str) -> ModelEntry {
        ModelEntry {
            id: id.to_string(),
            display_name: None,
            source: ModelSource::Provider,
            context_window: Some(64_000),
            max_output_tokens: None,
        }
    }

    /// Lists `litellm: [llama3:8b, gpt-5.5]`, `openai: [gpt-5.5]`.
    fn lister() -> ModelLister {
        Arc::new(|endpoints: Vec<Endpoint>| {
            Box::pin(async move {
                endpoints
                    .into_iter()
                    .map(|e| {
                        let models = match e.name.as_str() {
                            "litellm" => vec![entry("llama3:8b"), entry("gpt-5.5")],
                            "openai" => vec![entry("gpt-5.5")],
                            _ => Vec::new(),
                        };
                        (
                            e.name,
                            Ok(EndpointListing {
                                models,
                                prices: Vec::new(),
                            }),
                        )
                    })
                    .collect()
            })
        })
    }

    fn registry() -> EndpointRegistry {
        EndpointRegistry::from_endpoints(vec![
            ep("anthropic", WireDialect::Anthropic, true),
            ep("openai", WireDialect::OpenAi, true),
            ep("litellm", WireDialect::OpenAi, false),
        ])
    }

    async fn plan(extra: &[&str], config_model: &str) -> Result<ModelChoice, TuiError> {
        let mut config = Config::default();
        config.agent.model = config_model.to_string();
        plan_model(&args(extra), &config, &registry(), &lister())
            .await
            .map(|(choice, _)| choice)
    }

    #[tokio::test]
    async fn startup_model_planning() {
        let choice = plan(&["--model", "claude-sonnet-4-6"], "").await.unwrap();
        assert_eq!(choice.endpoint.as_str(), "anthropic");
        assert_eq!(choice.provider, Provider::Anthropic);
        assert!(choice.auth_binding.is_none());
        // A bare id only one endpoint lists resolves by lookup.
        let choice = plan(&["--model", "llama3:8b"], "").await.unwrap();
        assert_eq!(
            (choice.endpoint.as_str(), choice.model.as_str()),
            ("litellm", "llama3:8b")
        );
        let binding = choice.auth_binding.unwrap();
        assert_eq!(binding.realm.as_str(), crate::endpoints::ENDPOINT_REALM);
        assert_eq!(binding.binding.as_str(), "litellm");
        // Listed by both: the built-in of its dialect wins.
        let choice = plan(&["--model", "gpt-5.5"], "").await.unwrap();
        assert_eq!(choice.endpoint.as_str(), "openai");
        // Explicit endpoint.
        let choice = plan(&["--model", "litellm:gpt-5.5"], "").await.unwrap();
        assert_eq!(choice.endpoint.as_str(), "litellm");
        assert_eq!(choice.provider, Provider::OpenAI);
        // `--provider` / `--endpoint` with a bare id.
        let choice = plan(&["--endpoint", "litellm", "--model", "x"], "")
            .await
            .unwrap();
        assert_eq!(choice.reference().to_string(), "litellm:x");
        // `--endpoint` alone on a configured endpoint: its first listed model.
        let choice = plan(&["--endpoint", "litellm"], "").await.unwrap();
        assert_eq!(choice.reference().to_string(), "litellm:llama3:8b");
        // The config model.
        let choice = plan(&[], "gpt-5.5").await.unwrap();
        assert_eq!(choice.reference().to_string(), "openai:gpt-5.5");
        let err = plan(&["--provider", "nope"], "").await.unwrap_err();
        assert!(err.to_string().contains("unknown endpoint `nope`"), "{err}");
    }

    #[test]
    fn tui_toml_confinement_grants_reach_the_plan() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = root.join("ws");
        let cfg = root.join("cfg");
        let (global_dir, overlay_dir, flag_dir) =
            (root.join("global"), root.join("overlay"), root.join("flag"));
        for d in [&ws, &cfg, &global_dir, &overlay_dir, &flag_dir] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(
            cfg.join("tui.toml"),
            format!(
                "[confinement]\nextra_read = [{:?}]\nextra_write = [{:?}]\n",
                global_dir.to_str().unwrap(),
                flag_dir.to_str().unwrap()
            ),
        )
        .unwrap();
        let set = format!("confinement.extra_read=[{:?}]", overlay_dir.to_str().unwrap());
        let args = args(&[
            "-w",
            ws.to_str().unwrap(),
            "--config-dir",
            cfg.to_str().unwrap(),
            "--set",
            &set,
            "--confinement-read",
            flag_dir.to_str().unwrap(),
        ]);
        let scope = resolve_scope(&args).unwrap();
        let (layered, _) = approval::load_layers(&scope, &args).unwrap();
        let extra = extra_grants(&args, &layered);
        assert_eq!(extra.read, vec![flag_dir.clone()]);
        assert_eq!(extra.file.extra_read, vec![global_dir.clone(), overlay_dir.clone()]);
        assert_eq!(extra.file.extra_write, vec![flag_dir.clone()]);
        // The flag wins over the file's write grant of the same directory.
        let (write, read, _) =
            confinement::grants_with(&ws, &HostPaths::default(), &extra).unwrap();
        assert!(read.contains(&flag_dir) && !write.contains(&flag_dir), "{write:?}");
        assert!(read.contains(&global_dir) && read.contains(&overlay_dir), "{read:?}");
        let requirement = confinement::lower_with(
            args.effective_confinement(),
            scope.project_root.as_path(),
            &HostPaths::default(),
            &extra,
        )
        .unwrap()
        .unwrap();
        let shown = format!("{requirement:?}");
        assert!(shown.contains(global_dir.to_str().unwrap()), "{shown}");
    }

    #[test]
    fn each_confinement_mode_reaches_the_factory_shell() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap();
        let ws_arg = ws.to_str().unwrap();
        for mode in ["workspace", "strict", "off"] {
            let args = args(&["-w", ws_arg, "--confinement", mode]);
            let scope = resolve_scope(&args).unwrap();
            let shell = match confinement::lower(
                args.effective_confinement(),
                scope.project_root.as_path(),
                &HostPaths::default(),
            )
            .unwrap()
            {
                Some(requirement) => ShellConfinement::Required { requirement },
                None => ShellConfinement::TrustedHost,
            };
            let factory = build_factory(
                &scope,
                ws.join("store"),
                FactoryToggles {
                    token_store: false,
                    memory: false,
                },
                &shell,
            );
            assert_eq!(factory.shell_confinement(), &shell, "{mode}");
            assert_eq!(
                mode == "off",
                shell == ShellConfinement::TrustedHost,
                "{mode}"
            );
        }
    }
}
