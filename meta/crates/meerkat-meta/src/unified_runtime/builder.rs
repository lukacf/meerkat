//! Builder for constructing a configured UnifiedRuntime instance.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use meerkat_client::LlmClient;
use meerkat_mob::{MobDefinition, MobStorage, SpawnMemberSpec};

use crate::console_aggregator::{ConsoleLogStore, InMemoryConsoleLogStore, SqliteConsoleLogStore};
use crate::contact_directory::ContactDirectory;
pub use crate::identity_first::IdentityBootstrapMode;
use crate::identity_first::{
    AgentCustomizer, AgentMemoryConfig, AgentMemoryCustomizer, AgentMemoryProvider,
    AgentMemoryRuntimeInjector, AgentRuntimeServices, DurabilityPolicy,
    IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore,
    LocalLeaseProvider, RosterContext, RosterProvider, TopologyProvider,
};
use crate::mob_composition_manifest::MobStorageProvenance;
use crate::mob_handle_runtime::{
    CapabilityFlags, MobBootstrapOptions, MobBootstrapSpec, SessionHook,
};
use crate::runtime::{
    InMemoryMetadataStore, PersistentMetadataStore, RuntimeOptions, SqliteMetadataStore,
};
use crate::storage_layout::MetaStorageLayout;
use crate::types::{EventEnvelope, MetaConfig, UnifiedEvent};

use super::edge_types::{Discovery, EdgeDiscovery, PreSpawnHook};
use super::types::{
    UnifiedRuntimeBootstrapError, UnifiedRuntimeBuilderError, UnifiedRuntimeBuilderField,
};
use super::{
    DEFAULT_DRAIN_TIMEOUT, ErrorHook, EventLogConfig, PostReconcileHook, PostSpawnHook,
    UnifiedRuntime, discovery_spec_to_spawn_spec,
};

/// How the mob definition is supplied to the builder.
pub(crate) enum DefinitionSource {
    Inline(Box<MobDefinition>),
    TomlPath(PathBuf),
}

/// A build over a realm kept in a host store (see [`crate::host_store`]).
struct HostStoreComposition {
    realm: crate::host_store::HostStoreRealm,
    /// The meerkat-level realm id the realm's stores are keyed by.
    realm_id: meerkat_core::RealmId,
    /// The per-host cache the composition is built over: `scratch_dir()`
    /// when set, else `cache_dir`. Disposable: it holds no authority.
    cache_root: PathBuf,
    /// The cache directory the runtime owns when the host named none.
    cache_dir: Option<Arc<tempfile::TempDir>>,
}

/// Default max concurrent sessions for builder-created session services.
const DEFAULT_MAX_SESSIONS: usize = 64;

/// Default builder timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct UnifiedRuntimeBuilder {
    // --- Legacy path (mob_spec directly) ---
    mob_spec: Option<MobBootstrapSpec>,

    // --- New convenience path ---
    definition_source: Option<DefinitionSource>,
    persistent_state_path: Option<PathBuf>,
    mob_storage: Option<(MobStorage, MobStorageProvenance)>,
    /// Host-declared `rkat` scope for `SessionRepairRequired` commands.
    session_repair_scope: Option<crate::identity_first::SessionRepairScope>,
    /// Root `continuity_from_state_dir` opened — pinned in CANONICAL form at
    /// open time — retained so `build()` can refuse a silent authority fork:
    /// session authority in one directory's continuity.sqlite3 with
    /// runtime/blob/workgraph stores in another is exactly the split-storage
    /// class the layout's twin detection refuses elsewhere. Canonical because
    /// the gate compares physical identity, not spelling: a relative path
    /// re-resolved under a later working directory, or a symlink retargeted
    /// between open and build, must not slip past as a lexical match.
    continuity_state_dir: Option<PathBuf>,
    /// The state directory's claim for this runtime, taken before anything
    /// opens a store in it (see [`crate::state_dir_claim`]) and handed to the
    /// runtime through its bootstrap spec.
    state_dir_claim: Option<Arc<crate::state_dir_claim::StateDirClaim>>,
    session_hook: Option<Arc<dyn SessionHook>>,
    meerkat_config: Option<meerkat::Config>,
    mcp_call_context_provider: Option<Arc<dyn meerkat_mcp::McpCallContextProvider>>,
    /// The host's operation refusal observer, installed on every
    /// builder-owned agent factory.
    operation_refusal_observer: Option<Arc<dyn meerkat_core::OperationRefusalObserver>>,
    /// The host's fork instruction policy, applied to every builder-owned
    /// mob spec. `None` keeps Meerkat's default.
    fork_instruction_policy: Option<meerkat_core::ForkInstructionPolicy>,
    /// Host-level compaction policy composed over `meerkat_config`'s
    /// compaction slot at spec-resolve time. Separate from `meerkat_config`
    /// so tuning compaction does not require an embedder to author a whole
    /// `meerkat::Config`, and so the one knob that actually bounds transcript
    /// growth is reachable by name.
    compaction_policy: Option<meerkat_core::config::CompactionRuntimeConfig>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
    max_sessions: Option<usize>,
    capability_flags: CapabilityFlags,

    /// Refuse to build when the session store would persist whole session
    /// documents per turn (see
    /// [`require_incremental_session_persistence`](Self::require_incremental_session_persistence)).
    require_incremental_session_persistence: bool,
    /// The continuity mirror to install (see
    /// [`continuity_mirror`](Self::continuity_mirror)).
    continuity_mirror: Option<(
        Arc<dyn super::continuity_mirror::ContinuityMirrorSink>,
        super::continuity_mirror::ContinuityMirrorTimings,
    )>,

    // --- Identity-first external path ---
    continuity_store: Option<Arc<dyn crate::identity_first::contracts::ContinuityStore>>,
    lease_provider: Option<Arc<dyn crate::identity_first::contracts::LeaseProvider>>,
    roster_provider: Option<Arc<dyn RosterProvider>>,
    topology_provider: Option<Arc<dyn TopologyProvider>>,
    agent_customizer: Option<Arc<dyn AgentCustomizer>>,
    agent_memory_provider: Option<Arc<dyn AgentMemoryProvider>>,
    agent_memory_config: Option<AgentMemoryConfig>,
    agent_memory_profile_policy: BTreeMap<meerkat_mob::ProfileName, bool>,
    /// Named Rust tool bundles added to the resolved or supplied mob spec,
    /// each with its availability to child mobs.
    tool_bundles: BTreeMap<
        String,
        (
            Arc<dyn meerkat_core::AgentToolDispatcher>,
            meerkat_mob_mcp::ChildToolBundleAvailability,
        ),
    >,
    /// Child mob application tool policy added to the mob spec.
    child_application_tool_policy: Option<meerkat_core::ApplicationToolPolicyBinding>,
    /// Native member registration hook added to the mob spec.
    member_runtime_registration_hook: Option<Arc<dyn meerkat_mob::MemberRuntimeRegistrationHook>>,
    /// The host's application consequence-policy registry added to the mob
    /// spec.
    tool_consequence_policy_registry: Option<Arc<meerkat_core::ToolConsequencePolicyRegistry>>,
    /// The host's actual grant owner and paired invocation policy (D50 S1),
    /// installed on the runtime machine this builder constructs. Taken once,
    /// by the one spec construction that builds that machine.
    grant_configuration:
        std::sync::Mutex<Option<meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration>>,
    #[cfg(feature = "extension-state")]
    tool_bundle_factories: BTreeMap<
        String,
        (
            crate::extension_state::ToolBundleRequirements,
            Arc<dyn crate::extension_state::ToolBundleFactory>,
        ),
    >,
    #[cfg(feature = "extension-state")]
    mcp_document_bindings: Vec<Arc<crate::extension_state::HostMcpDocumentBinding>>,
    #[cfg(feature = "extension-state")]
    extension_store: Option<crate::storage_provider::OpenedExtensionState>,
    agent_memory_engines: Option<crate::memory_wiring::MemoryEnginesConfig>,
    identity_bootstrap_mode: IdentityBootstrapMode,
    identity_bootstrap_mode_configured: bool,
    identity_runtime_instance_id: Option<String>,
    scratch_dir: Option<PathBuf>,
    /// The root of the members' session comms identities (see
    /// [`session_identity_config_root`](Self::session_identity_config_root)).
    session_identity_config_root: Option<PathBuf>,
    /// The host's provider credential store (see
    /// [`provider_auth_persistence`](Self::provider_auth_persistence)).
    provider_auth_persistence: Option<meerkat::ProviderAuthPersistence>,
    blob_store: Option<Arc<dyn meerkat_core::BlobStore>>,
    binary_blob_store: Option<Arc<dyn crate::blob_store::BinaryBlobStore>>,
    ephemeral_blobs: bool,
    ephemeral_runtime_store: bool,
    /// Whether the caller set `continuity_store()` or `lease_provider()`,
    /// both ignored since 0.9.4 (a warning at build).
    injected_continuity_seams: bool,
    schedule_store: Option<Arc<dyn meerkat::ScheduleStore>>,
    workgraph_store: Option<Arc<dyn meerkat::WorkGraphStore>>,
    storage_provider: Option<Arc<dyn crate::storage_provider::MetaStorageProvider>>,
    meerkat_realm_declaration: Option<crate::storage_provider::MeerkatLevelRealmDeclaration>,
    // Materialized from `storage_provider` (M4b): the meerkat-level bundle
    // opened through `meerkat_provider()` for non-disk backends, and the
    // provider's per-slot durability declarations retained for the census.
    provider_meerkat_stores: Option<crate::storage_provider::ProviderMeerkatStores>,
    provider_slot_census: Vec<crate::storage_health::StorageSlotSummary>,
    /// Materialized from a `storage_provider` that keeps the realm in a host
    /// store: the realm and the per-host cache the build composes over.
    host_store: Option<HostStoreComposition>,
    console_log_store: Option<Arc<dyn ConsoleLogStore>>,

    // --- Common fields ---
    module_config: Option<MetaConfig>,
    module_agent_events: Vec<EventEnvelope<UnifiedEvent>>,
    timeout: Option<Duration>,
    options: RuntimeOptions,
    post_spawn_hook: Option<PostSpawnHook>,
    post_reconcile_hook: Option<PostReconcileHook>,
    error_hook: Option<ErrorHook>,
    event_log_config: Option<EventLogConfig>,
    drain_timeout: Option<Duration>,
    discovery: Option<Box<dyn Discovery>>,
    /// Behind a mutex only so the builder is `Sync`: the public
    /// [`PreSpawnHook`] is `Send` but not `Sync`, and `build` holds `&self`
    /// across awaits, so a plain field made `build()`'s future `!Send`. The
    /// hook is only ever moved in and out by value; the lock is never
    /// contended.
    pre_spawn_hook: std::sync::Mutex<Option<PreSpawnHook>>,
    edge_discovery: Option<Box<dyn EdgeDiscovery>>,
    contact_directory: Option<ContactDirectory>,
    control_listen: Option<String>,
    control_grants: Option<crate::runtime::cross_mob_control::ControlGrantTable>,
    keyed_inject: Option<crate::mob_announcements::KeyedInjectPlane>,
    mob_announcements: Option<crate::mob_announcements::AnnouncementConfig>,
    persistent_metadata: Option<Arc<dyn PersistentMetadataStore>>,
    access_controller: Option<crate::access::AccessController>,
    flow_editor_host_deploy: bool,
    topology_control_policy: crate::topology_control::TopologyControlPolicy,
    live: Option<super::live_compose::LiveOptions>,
    #[cfg(feature = "openai-live")]
    console_voice: Option<crate::public_live_config::PublicLiveRegistration>,
}

impl UnifiedRuntimeBuilder {
    /// Enable standard MCP Apps capabilities for a host with an isolated UI
    /// renderer. The stock Console also needs `mcp_apps_sandbox_url` configured.
    pub fn mcp_apps(mut self, enabled: bool) -> Self {
        self.capability_flags.mcp_apps = enabled;
        self
    }

    // -----------------------------------------------------------------------
    // New convenience API
    // -----------------------------------------------------------------------

    /// Set the mob definition from an inline `MobDefinition`.
    pub fn definition(mut self, def: MobDefinition) -> Self {
        self.definition_source = Some(DefinitionSource::Inline(Box::new(def)));
        self
    }

    /// Set the mob definition from a TOML file path.
    pub fn definition_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.definition_source = Some(DefinitionSource::TomlPath(path.into()));
        self
    }

    /// Enable persistent state at the given path. When set, the builder
    /// composes Core's session and runtime stores on `runtime.sqlite`, and
    /// the metadata store, console log store, binary blob store and mob
    /// storage (`event_log.sqlite3`, unless supplied through
    /// [`mob_storage`](Self::mob_storage)) under this directory. Before any
    /// store opens, `build()` refuses a directory that another process still
    /// hosts runtimes on, one that needs `meta_gateway
    /// storage-migrate-native`, and one whose identity bindings lost their
    /// runtime store. When not set, the builder uses an ephemeral session
    /// service with an auto-created temp directory.
    pub fn persistent_state(mut self, path: impl Into<PathBuf>) -> Self {
        self.persistent_state_path = Some(path.into());
        self
    }

    /// Supply the complete per-mob storage and its composition provenance for
    /// builder-created session services. Use the pair returned by
    /// [`crate::mob_composition_manifest::persistent_mob_storage`] to retain
    /// the mob journal, identity state, and fork records across restarts.
    ///
    /// This slot is separate from the realm stores selected by
    /// [`storage_provider`](Self::storage_provider) and does not change session
    /// persistence. Without an override, a
    /// [`persistent_state`](Self::persistent_state) build keeps mob storage
    /// durable in `event_log.sqlite3` under the state directory, and any other
    /// build keeps it in memory. A persistent build with a
    /// [`roster_provider`](Self::roster_provider) refuses storage without a
    /// persistent path: the roster's identity bindings live in it. A pre-built
    /// [`mob_spec`](Self::mob_spec) already owns this slot and cannot be
    /// combined with this override.
    pub fn mob_storage(mut self, storage: MobStorage, provenance: MobStorageProvenance) -> Self {
        self.mob_storage = Some((storage, provenance));
        self
    }

    /// Override where the `rkat session repair-wholeblob` commands in a
    /// `SessionRepairRequired` hold point. By default the builder derives the
    /// scope from the runtime store it opened (`persistent_state` opens
    /// `<state_dir>/runtime.sqlite`, rendered as `--runtime-store`; a
    /// composite storage provider's meerkat-level realm renders as
    /// `--state-root <state_dir> --realm meta`); a build with no persistent
    /// runtime store declares no scope and the commands carry explicit
    /// `<state-root>` / `<realm>` placeholders. Meta never guesses a path.
    pub fn session_repair_scope(
        mut self,
        scope: crate::identity_first::SessionRepairScope,
    ) -> Self {
        self.session_repair_scope = Some(scope);
        self
    }

    /// Set a session lifecycle hook.
    pub fn session_hook(mut self, hook: Arc<dyn SessionHook>) -> Self {
        self.session_hook = Some(hook);
        self
    }

    /// Set the Meerkat agent factory configuration used by builder-created
    /// session services.
    ///
    /// Applications with long-lived durable coordinator agents can use this to
    /// tune session compaction and other factory-level Meerkat behavior without
    /// constructing a full `MobBootstrapSpec` by hand.
    pub fn meerkat_config(mut self, config: meerkat::Config) -> Self {
        self.meerkat_config = Some(config);
        self
    }

    /// Declare the host-level session-compaction policy for every session
    /// this runtime builds.
    ///
    /// Meerkat always installs a compactor on the built session path; what
    /// this declares is *when it fires*. Without a declaration the threshold
    /// is meerkat's model-aware default — `context_window * 4 / 5`, i.e.
    /// `840_000` tokens on a million-token model, which in practice means
    /// "never". Declaring `auto_compact_threshold` pins the number instead
    /// (see [`crate::compaction_policy`] for the full precedence rules; a mob
    /// profile's own `auto_compact_threshold` still wins per member).
    ///
    /// Composes over [`meerkat_config`](Self::meerkat_config): this
    /// declaration replaces that config's compaction slot and leaves every
    /// other field alone. Invalid declarations (a zero threshold) surface as
    /// a build-time `ConflictingConfiguration` error, never as a silently
    /// ignored knob.
    pub fn compaction(mut self, policy: meerkat_core::config::CompactionRuntimeConfig) -> Self {
        self.compaction_policy = Some(policy);
        self
    }

    /// Set the default LLM client (used for test stubs).
    pub fn default_llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.default_llm_client = Some(client);
        self
    }

    /// Set the maximum number of active sessions for builder-created session
    /// services.
    ///
    /// This only applies to the definition-based path. A legacy `.mob_spec()`
    /// supplies its own already-built session service and capacity.
    pub fn max_sessions(mut self, max_sessions: usize) -> Self {
        self.max_sessions = Some(max_sessions);
        self
    }

    /// Set an external `ContinuityStore` for the identity-first path.
    ///
    /// Ignored since 0.9.4: identity continuity and leases live in the state
    /// directory, and the runtime never reads or writes this store. Migrate
    /// 0.9.3 identities with the continuity export (see the continuity
    /// export guide). To keep an external copy, use
    /// [`continuity_mirror`](Self::continuity_mirror). The external path still requires the pair with
    /// [`lease_provider`](Self::lease_provider) until both methods are
    /// removed, and `build()` logs a warning when either is set.
    pub fn continuity_store(
        mut self,
        store: Arc<dyn crate::identity_first::contracts::ContinuityStore>,
    ) -> Self {
        self.continuity_store = Some(store);
        self.injected_continuity_seams = true;
        self
    }

    /// Opt in to strict session persistence: `build()` refuses with
    /// [`UnifiedRuntimeBuilderError::SessionStoreNotIncremental`] when the
    /// session store does not advertise meerkat's incremental persistence,
    /// instead of starting on the whole-blob fallback (one warning, then the
    /// whole session document written at every turn boundary).
    ///
    /// The SQLite session store `persistent_state()` composes is
    /// incremental; a [`storage_provider`](Self::storage_provider)'s realm
    /// session store may not be. Off by default.
    pub fn require_incremental_session_persistence(mut self, required: bool) -> Self {
        self.require_incremental_session_persistence = required;
        self
    }

    /// Keep a live copy of each durable identity's current record and
    /// committed session in `sink`, refreshed every `interval`: a restore
    /// source for a lost state directory that is never read at boot (see
    /// [`continuity_mirror`](super::continuity_mirror)). The runtime owns the
    /// mirror: shutdown writes a last copy, and dropping the runtime stops it.
    /// `build()` refuses it on a runtime that is not identity-first, which has
    /// no durable identity to copy.
    pub fn continuity_mirror(
        mut self,
        sink: Arc<dyn super::continuity_mirror::ContinuityMirrorSink>,
        interval: std::time::Duration,
    ) -> Self {
        let timings = super::continuity_mirror::ContinuityMirrorTimings {
            interval,
            ..Default::default()
        };
        self.continuity_mirror = Some((sink, timings));
        self
    }

    /// [`continuity_mirror`](Self::continuity_mirror) with every timing set:
    /// the interval, how long one sink put may take, and how long the last
    /// round at shutdown may take.
    pub fn continuity_mirror_with_timings(
        mut self,
        sink: Arc<dyn super::continuity_mirror::ContinuityMirrorSink>,
        timings: super::continuity_mirror::ContinuityMirrorTimings,
    ) -> Self {
        self.continuity_mirror = Some((sink, timings));
        self
    }

    /// Set an external `LeaseProvider` for the identity-first path.
    ///
    /// Ignored since 0.9.4, like
    /// [`continuity_store`](Self::continuity_store): identity continuity and
    /// leases live in the state directory, and the runtime never calls this
    /// provider. Migrate 0.9.3 identities with the continuity export.
    pub fn lease_provider(
        mut self,
        provider: Arc<dyn crate::identity_first::contracts::LeaseProvider>,
    ) -> Self {
        self.lease_provider = Some(provider);
        self.injected_continuity_seams = true;
        self
    }

    /// Open the state directory's identity substrate (the continuity store
    /// and its lease provider) and install both halves, through the same
    /// [`crate::gateway_wiring`] seam the gateways use, including the fencing
    /// floor, which must resume above the persisted high-water or restore
    /// aborts every restart with a stale token.
    ///
    /// The continuity store holds identity records only: sessions live in
    /// Core's session and runtime stores on `runtime.sqlite`, which
    /// [`persistent_state`](Self::persistent_state) composes. Pair it with
    /// `persistent_state` pointing at the SAME directory and with
    /// [`roster_provider`](Self::roster_provider).
    ///
    /// # Errors
    ///
    /// Fails when the state directory cannot be created or claimed, when it
    /// is refused before any store opens (see
    /// [`persistent_state`](Self::persistent_state)), when the continuity slot
    /// cannot be resolved (file-name twins), or when the substrate cannot be
    /// opened; it never degrades to a zero fencing floor.
    pub async fn continuity_from_state_dir(
        mut self,
        state_dir: impl AsRef<std::path::Path>,
    ) -> Result<Self, UnifiedRuntimeBuilderError> {
        let state_dir = state_dir.as_ref();
        std::fs::create_dir_all(state_dir).map_err(|e| {
            UnifiedRuntimeBuilderError::Io(format!(
                "failed to create state directory at {}: {e}",
                state_dir.display()
            ))
        })?;
        // Pin the CANONICAL root at open time: the substrate is opened
        // against this physical directory NOW, and the same-root gate in
        // `build()` compares against what was actually opened. A relative
        // spelling re-resolved under a later working directory, or a
        // retargeted symlink, would otherwise fork session authority while
        // comparing equal lexically.
        let state_dir = std::fs::canonicalize(state_dir).map_err(|e| {
            UnifiedRuntimeBuilderError::Io(format!(
                "failed to canonicalize state directory at {}: {e}",
                state_dir.display()
            ))
        })?;
        self.claim_state_dir(
            &state_dir,
            crate::native_startup::RuntimeStoreLocation::StateDir,
        )
        .await?;
        let continuity_db = MetaStorageLayout::with_injected_roots(state_dir.clone(), None)
            .continuity_db()?
            .path;
        let substrate = crate::gateway_wiring::open_identity_substrate(&continuity_db)
            .await
            .map_err(UnifiedRuntimeBuilderError::Io)?;
        self.continuity_store = Some(substrate.continuity_store);
        self.lease_provider = Some(substrate.lease_provider);
        self.continuity_state_dir = Some(state_dir);
        Ok(self)
    }

    /// Claim `state_dir` for this runtime, once (see
    /// [`crate::state_dir_claim`]), and run the startup checks before any
    /// store opens in it (see [`crate::native_startup`]). The claim this
    /// builder or its supplied `mob_spec` already holds on the same
    /// directory is reused: the OS lock belongs to its open file
    /// description, so a second acquisition would refuse this very runtime,
    /// and its holder ran the checks when it took it. `runtime_store` is
    /// where this composition's runtime store lives: a storage provider's
    /// backend leaves no `runtime.sqlite` here for the bindings check.
    async fn claim_state_dir(
        &mut self,
        state_dir: &std::path::Path,
        runtime_store: crate::native_startup::RuntimeStoreLocation,
    ) -> Result<(), UnifiedRuntimeBuilderError> {
        let held = self
            .state_dir_claim
            .iter()
            .chain(
                self.mob_spec
                    .iter()
                    .flat_map(|spec| spec.state_dir_claims.iter()),
            )
            .find(|claim| same_directory(claim.state_dir(), state_dir))
            .cloned();
        let claim = match held {
            Some(claim) => claim,
            None => {
                let claim = Arc::new(crate::state_dir_claim::StateDirClaim::acquire(state_dir)?);
                let mob_store = self.durable_mob_store();
                crate::native_startup::refuse_unready_state_dir_for(
                    state_dir,
                    mob_store.as_deref(),
                    runtime_store,
                )
                .await?;
                claim
            }
        };
        self.state_dir_claim = Some(claim);
        Ok(())
    }

    /// The durable mob store this build composes: the supplied storage's
    /// persistent path, or, under `persistent_state`, the default
    /// `event_log.sqlite3` beside the sessions.
    fn durable_mob_store(&self) -> Option<std::path::PathBuf> {
        match (&self.mob_storage, &self.persistent_state_path) {
            (Some((_, provenance)), _) => provenance
                .persistent_path()
                .map(std::path::Path::to_path_buf),
            (None, Some(state_path)) => Some(
                MetaStorageLayout::with_injected_roots(state_path.clone(), None).event_log_db(),
            ),
            (None, None) => None,
        }
    }

    /// Set the desired identity roster provider for identity-first bootstrap.
    pub fn roster_provider(mut self, provider: Arc<dyn RosterProvider>) -> Self {
        self.roster_provider = Some(provider);
        self
    }

    /// Set the managed topology provider for identity-first bootstrap and refresh.
    pub fn topology_provider(mut self, provider: Arc<dyn TopologyProvider>) -> Self {
        self.topology_provider = Some(provider);
        self
    }

    /// Register a named host-only Rust tool bundle for profiles'
    /// `tools.rust_bundles`. It is added to the mob spec this builder resolves
    /// or is given; see [`MobBootstrapSpec::register_tool_bundle`]. A name that
    /// the supplied spec already registers is refused at build.
    #[must_use]
    pub fn register_tool_bundle(
        self,
        name: impl Into<String>,
        dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher>,
    ) -> Self {
        self.register_tool_bundle_with_availability(
            name,
            dispatcher,
            meerkat_mob_mcp::ChildToolBundleAvailability::HostOnly,
        )
    }

    /// Register a named Rust tool bundle with its availability to child
    /// mobs; see [`MobBootstrapSpec::register_tool_bundle_with_availability`].
    /// A name that the supplied spec already registers is refused at build.
    #[must_use]
    pub fn register_tool_bundle_with_availability(
        mut self,
        name: impl Into<String>,
        dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher>,
        availability: meerkat_mob_mcp::ChildToolBundleAvailability,
    ) -> Self {
        self.tool_bundles
            .insert(name.into(), (dispatcher, availability));
        self
    }

    /// The application tool policy child mob members are built with; see
    /// [`MobBootstrapSpec::with_child_application_tool_policy`]. A supplied
    /// spec that already sets one is refused at build.
    #[must_use]
    pub fn child_application_tool_policy(
        mut self,
        binding: meerkat_core::ApplicationToolPolicyBinding,
    ) -> Self {
        self.child_application_tool_policy = Some(binding);
        self
    }

    /// The host's native member registration hook; see
    /// [`MobBootstrapSpec::with_member_runtime_registration_hook`]. A supplied
    /// spec that already sets one is refused at build.
    #[must_use]
    pub fn member_runtime_registration_hook(
        mut self,
        hook: Arc<dyn meerkat_mob::MemberRuntimeRegistrationHook>,
    ) -> Self {
        self.member_runtime_registration_hook = Some(hook);
        self
    }

    /// The host's application consequence-policy registry, which member
    /// builds bind their selected policy against; see
    /// [`MobBootstrapSpec::with_tool_consequence_policy_registry`]. A member
    /// a registration hook governs is finished under the policy its host
    /// selects from this registry. A supplied spec that already sets one is
    /// refused at build.
    #[must_use]
    pub fn tool_consequence_policy_registry(
        mut self,
        registry: Arc<meerkat_core::ToolConsequencePolicyRegistry>,
    ) -> Self {
        self.tool_consequence_policy_registry = Some(registry);
        self
    }

    /// Install the host's actual native grant owner and its paired invocation
    /// policy (D50 S1; build it with `ScopedGrantComposition::into_configuration`)
    /// on the runtime machine this builder constructs. Meta never issues
    /// grants: it installs exactly the supplied owner. Refused with a
    /// supplied `mob_spec`, whose machine is already built.
    #[must_use]
    pub fn native_grant_work_configuration(
        self,
        configuration: meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration,
    ) -> Self {
        *self
            .grant_configuration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(configuration);
        self
    }

    /// Build an optional tool bundle from the configured provider's durable
    /// document capability. Profiles select its name through rust_bundles.
    #[cfg(feature = "extension-state")]
    pub fn register_tool_bundle_factory(
        mut self,
        name: impl Into<String>,
        requirements: crate::extension_state::ToolBundleRequirements,
        factory: Arc<dyn crate::extension_state::ToolBundleFactory>,
    ) -> Self {
        self.tool_bundle_factories
            .insert(name.into(), (requirements, factory));
        self
    }

    /// Install a process-local context provider on every builder-owned agent
    /// factory. A supplied, already-erased mob_spec cannot accept this override.
    pub fn mcp_call_context_provider(
        mut self,
        provider: Arc<dyn meerkat_mcp::McpCallContextProvider>,
    ) -> Self {
        self.mcp_call_context_provider = Some(provider);
        self
    }

    /// Install the host's operation refusal observer on every builder-owned
    /// agent factory: it receives each refusal disposition the agents of
    /// those factories decide for a governed operation (see
    /// `meerkat::AgentFactory::with_operation_refusal_observer`). Nothing of
    /// it reaches the event stream, a wire type or the model. A supplied,
    /// already-erased mob_spec cannot accept this override.
    pub fn operation_refusal_observer(
        mut self,
        observer: Arc<dyn meerkat_core::OperationRefusalObserver>,
    ) -> Self {
        self.operation_refusal_observer = Some(observer);
        self
    }

    /// Set the host's fork instruction policy for member forks of the mob
    /// this builder composes (persistent, scratch and temp-dir launches, on
    /// create and on resume): what a fork of a member with activated host
    /// instructions does with them (see
    /// `MobBootstrapSpec::with_fork_instruction_policy`). Unset, Meerkat's
    /// default applies. A supplied, already-built mob_spec carries its own
    /// policy, so the builder refuses this override; set it on that spec.
    pub fn fork_instruction_policy(mut self, policy: meerkat_core::ForkInstructionPolicy) -> Self {
        self.fork_instruction_policy = Some(policy);
        self
    }

    /// Bind provider-owned documents to an external MCP host. Profiles select
    /// the binding's public descriptor; no native tool bundle is installed.
    #[cfg(feature = "extension-state")]
    pub fn register_mcp_tool_bundle_factory(
        mut self,
        binding: crate::extension_state::HostMcpDocumentBinding,
    ) -> Self {
        self.mcp_document_bindings.push(Arc::new(binding));
        self
    }

    fn effective_mcp_call_context_provider(
        &self,
    ) -> Option<Arc<dyn meerkat_mcp::McpCallContextProvider>> {
        #[cfg(feature = "extension-state")]
        if !self.mcp_document_bindings.is_empty() {
            return Some(Arc::new(
                crate::extension_state::mcp_binding::DocumentMcpContextProvider {
                    bindings: self.mcp_document_bindings.clone(),
                    fallback: self.mcp_call_context_provider.clone(),
                },
            ));
        }
        self.mcp_call_context_provider.clone()
    }

    /// Set the identity-first build customizer.
    pub fn agent_customizer(mut self, customizer: Arc<dyn AgentCustomizer>) -> Self {
        self.agent_customizer = Some(customizer);
        self
    }

    /// Enable identity-first agent memory injection using the provided memory provider.
    pub fn agent_memory(
        mut self,
        provider: Arc<dyn AgentMemoryProvider>,
        config: AgentMemoryConfig,
    ) -> Self {
        self.agent_memory_provider = Some(provider);
        self.agent_memory_config = Some(config);
        self
    }

    /// Enable the FULL agent-memory stack (bundled SQLite store + the taint
    /// firewall + the enabled judgment-plane engines) — the same stack the
    /// rpc gateway assembles, reachable from the Rust builder (the operator
    /// deployment shape). Requires `persistent_state()`; the store lives
    /// under the layout's agent-memory root (canonical
    /// `<persistent_state>/agent-memory`, with a legacy
    /// `agent-memory-sqlite/` corpus honored where it lies).
    ///
    /// v1 boundaries (documented in `memory_wiring`): the Hygienist stays
    /// gateway-only; engines are driven by the member-event observe stream
    /// (their primary trigger path — the gateway's additional injector-side
    /// rotation hooks are not wired here yet); the steward dream runs on the
    /// in-process loop (no schedule host in library mode).
    pub fn persistent_agent_memory_stack(
        mut self,
        config: AgentMemoryConfig,
        engines: crate::memory_wiring::MemoryEnginesConfig,
    ) -> Self {
        self.agent_memory_config = Some(config);
        self.agent_memory_engines = Some(engines);
        self
    }

    /// Override identity-first memory tooling for a specific durable profile.
    ///
    /// Inline profiles inherit `profiles.<name>.tools.memory` automatically.
    /// Realm-referenced profiles are unresolved at the customizer boundary and
    /// therefore fail closed unless explicitly enabled with this method.
    pub fn agent_memory_for_profile(
        mut self,
        profile: impl Into<meerkat_mob::ProfileName>,
        enabled: bool,
    ) -> Self {
        self.agent_memory_profile_policy
            .insert(profile.into(), enabled);
        self
    }

    fn composed_agent_customizer(
        &self,
        memory_provider: Option<Arc<dyn AgentMemoryProvider>>,
    ) -> Option<Arc<dyn AgentCustomizer>> {
        match memory_provider {
            Some(provider) => Some(Arc::new(
                AgentMemoryCustomizer::wrap(
                    self.agent_customizer.clone(),
                    provider,
                    self.agent_memory_config.clone().unwrap_or_default(),
                )
                .with_profile_memory_policy(self.agent_memory_profile_policy.clone()),
            )),
            None => self.agent_customizer.clone(),
        }
    }

    /// Set how identity-first durable agents are materialized during build.
    pub fn identity_bootstrap_mode(mut self, mode: IdentityBootstrapMode) -> Self {
        self.identity_bootstrap_mode = mode;
        self.identity_bootstrap_mode_configured = true;
        self
    }

    /// Set the identity runtime instance id used when acquiring leases.
    pub fn identity_runtime_instance_id(mut self, id: impl Into<String>) -> Self {
        self.identity_runtime_instance_id = Some(id.into());
        self
    }

    /// Set a scratch directory for the external-authoritative path.
    ///
    /// Required when using `continuity_store()` + `lease_provider()`. Over a
    /// realm kept in a host store it is the per-host cache the runtime is
    /// built over (without it the runtime owns a temporary one): disposable,
    /// it holds no authority.
    pub fn scratch_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.scratch_dir = Some(path.into());
        self
    }

    /// Set the root of the members' session comms identities, as the
    /// gateway's `runtime_options.session_identity_config_root` sets it.
    /// With in-process comms (the default) each member session's key pair
    /// then lives under `<root>/.rkat/session_comms_identity/<session id>`,
    /// and no ambient location is consulted. Unset, the per-host default
    /// applies: `$XDG_STATE_HOME/meerkat/session_comms_identity`, else
    /// `~/.local/state/meerkat/session_comms_identity`, on Linux, and
    /// `~/Library/Application Support/meerkat/session_comms_identity` on
    /// macOS.
    ///
    /// A restored member's comms endpoint must match the one its mob
    /// recorded, so a start that cannot find a member's key pair restores
    /// that member broken. A realm kept in a host store holds the members but
    /// not their keys: point this at storage that survives a restart and
    /// moves with the realm. Turning this on, or changing it, for members
    /// whose key pairs already live elsewhere marks every one of them broken
    /// at the next start: copy their per-session key directories into
    /// `<root>/.rkat/session_comms_identity` first, or set the root from the
    /// first start after an import, as for a cutover.
    ///
    /// The root is also the agent factory's user-global convention root,
    /// which Core consults when it resolves skill repositories, as with the
    /// gateway's setting. An empty root is refused, as the gateway refuses
    /// it, and so is a supplied, already-built mob_spec, which cannot accept
    /// this override.
    pub fn session_identity_config_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.session_identity_config_root = Some(root.into());
        self
    }

    /// Keep the provider credentials of every builder-owned agent factory in
    /// the host's credential store instead of the process default,
    /// `<config dir>/meerkat/credentials`, which every runtime and process
    /// of the OS user shares. Open the store once, with
    /// `meerkat::TokenStoreBackend::File { root }.open_with_refresh_authority()`,
    /// and pass the pair here: every factory of the runtime (members,
    /// helpers, forks, child mobs and councils, live sessions, and the agent
    /// memory engines) holds a clone of it, so they share one refresh
    /// authority. Provider logins, token refreshes and logouts then read and
    /// write under that root only.
    ///
    /// Two runtimes whose members use the same realm and binding ids share
    /// credentials exactly when they share a root. A host serving several
    /// users gives each user's runtime its own root, and passes the same
    /// root again whenever it reopens that runtime's state. Unset, the
    /// process default applies, as before. A supplied, already-erased
    /// mob_spec cannot accept this override, so the build refuses it there:
    /// such a host attaches the pair to the factory its session service
    /// builds on (`meerkat::AgentFactory::with_provider_auth_persistence`),
    /// and records it on the built runtime with
    /// [`UnifiedRuntime::set_explicit_provider_credentials`], so the runtime
    /// refuses a host deploy whose `rkat` process could not carry it.
    pub fn provider_auth_persistence(
        mut self,
        persistence: meerkat::ProviderAuthPersistence,
    ) -> Self {
        self.provider_auth_persistence = Some(persistence);
        self
    }

    /// Set an optional blob store for custom blob persistence.
    ///
    /// The same store is used for runtime image blobs and for Meta's
    /// `/blobs/{id}` and `meta/blob/*` serving/upload paths.
    pub fn blob_store(mut self, store: Arc<dyn meerkat_core::BlobStore>) -> Self {
        self.blob_store = Some(store);
        self
    }

    /// Set an optional blob store in its raw-bytes [`crate::blob_store::BinaryBlobStore`] form
    /// (M4). Like [`blob_store`](Self::blob_store) but without the base64
    /// adapter round-trip on the HTTP `/blobs/{id}` and `meta/blob/*`
    /// byte-serving paths; the meerkat `BlobStore` face is adapted from it.
    /// Mutually exclusive with `blob_store()` — inject exactly one form.
    pub fn binary_blob_store(mut self, store: Arc<dyn crate::blob_store::BinaryBlobStore>) -> Self {
        self.binary_blob_store = Some(store);
        self
    }

    /// Declare that blobs are intentionally ephemeral (in-memory).
    ///
    /// Persistent-state builds fail closed when the blob slot resolves to a
    /// store that does not survive restart — a failed open of the local blob
    /// directory or an injected [`blob_store`](Self::blob_store) reporting
    /// `!is_persistent()`. This declaration makes in-memory blobs a
    /// configuration instead of an error (tests, demos), and is reported as
    /// `declared_ephemeral` on the health surfaces. Ephemeral launch modes
    /// (scratch/temp-dir) declare it implicitly.
    pub fn ephemeral_blobs(mut self, enabled: bool) -> Self {
        self.ephemeral_blobs = enabled;
        self
    }

    /// Declare that the runtime store is intentionally ephemeral
    /// (in-memory), for a [`storage_provider`](Self::storage_provider) realm
    /// opened on a [`scratch_dir`](Self::scratch_dir), or for an
    /// identity-first build on the external path.
    ///
    /// An identity-first build without
    /// [`persistent_state`](Self::persistent_state), a non-disk
    /// [`storage_provider`](Self::storage_provider) or a
    /// [`mob_spec`](Self::mob_spec) keeps its sessions in memory, so every
    /// session ends with the process and a restart starts every identity
    /// with an empty transcript. `build()` refuses that configuration with
    /// [`UnifiedRuntimeBuilderError::ConflictingConfiguration`] unless this
    /// declaration says so on purpose.
    ///
    /// A [`persistent_state`](Self::persistent_state) build refuses it:
    /// sessions live in the runtime store under the state directory, so a
    /// persistent build keeps that store there. Ephemeral launch modes
    /// declare it implicitly.
    pub fn ephemeral_runtime_store(mut self, enabled: bool) -> Self {
        self.ephemeral_runtime_store = enabled;
        self
    }

    /// Set an external [`ScheduleStore`](meerkat::ScheduleStore) (M4): the
    /// agent-facing schedule tools attach over the caller's store instead of
    /// the bundled SQLite file. Library mode wires no firing host — authored
    /// schedules become durable rows the embedder's own driver (or a gateway
    /// pointed at the same store) fires. Injection is a *foundation*: any
    /// shadow-scheduler deletion downstream requires a feature-parity audit
    /// first (multi-replica claims, timezone cron, jitter, delivery-policy
    /// handoffs).
    pub fn schedule_store(mut self, store: Arc<dyn meerkat::ScheduleStore>) -> Self {
        self.schedule_store = Some(store);
        self
    }

    /// Set an external [`WorkGraphStore`](meerkat::WorkGraphStore) (item 5):
    /// the agent-facing workgraph tools attach over the caller's store
    /// instead of the SQLite file beside `runtime.sqlite`. Injectable
    /// INDEPENDENTLY of continuity, schedule, lease, console and blob - that
    /// independence is the point of the item, replacing the previous
    /// all-or-nothing composition where a durable workgraph was reachable
    /// only by adopting a whole composite provider.
    ///
    /// Durability rides with the injector. Meta does NOT gate on
    /// [`WorkGraphStore::kind()`](meerkat::WorkGraphStore::kind), which is a
    /// backend-SHAPE tag and not a durability oracle: the path behind
    /// `Sqlite` is caller-supplied and says nothing about whether it
    /// survives a restart, and `Custom` says nothing either way.
    ///
    /// Two things an injected store does NOT get, both deliberate:
    /// - It is a MEERKAT-level slot, so it is absent from
    ///   `storage_provider::REQUIRED_META_DURABILITY_DOMAINS` and from the
    ///   meta `RealmStoreSet`. On the provider path the workgraph rides
    ///   `provider_meerkat_stores` instead, and per-slot injection takes
    ///   precedence over it.
    /// - The cross-process admission sidecar is keyed on the STATE DIR, not
    ///   on the store. An injected backend can live anywhere, so the sidecar
    ///   serializes co-processes sharing a state dir rather than co-processes
    ///   sharing this store.
    pub fn workgraph_store(mut self, store: Arc<dyn meerkat::WorkGraphStore>) -> Self {
        self.workgraph_store = Some(store);
        self
    }

    /// Install a composite [`MetaStorageProvider`]: the one-remote-bundle
    /// seam (M4). The provider's realm store set supplies the console
    /// timeline, metadata, blobs, schedule store, and (when configured) the
    /// event log and agent memory, replacing the per-slot builder seams;
    /// supplying both the provider and an individual seam is a conflict. Its
    /// continuity store and lease authority are ignored since 0.9.4: an
    /// identity's continuity is Core's mob identity authority in the mob
    /// storage. For a non-disk backend the composition also opens the
    /// provider's meerkat-level bundle through
    /// [`meerkat_provider`](crate::storage_provider::MetaStorageProvider::meerkat_provider)
    /// (M4b), so runtime and workgraph authority land in the same backend
    /// instead of local files; the built-in disk backend keeps the flat
    /// local composition.
    ///
    /// Requires an identity-first configuration (`roster_provider()`). The
    /// sessions live in the provider's runtime store, which the build checks
    /// against the identities' bindings before any identity wakes. The realm
    /// root comes from `persistent_state()` or `scratch_dir()`. Over
    /// `scratch_dir()` the mob storage stays in memory unless
    /// [`mob_storage`](Self::mob_storage) supplies one, so a restart keeps a
    /// non-disk backend's sessions but starts every identity fresh; the
    /// storage census warns about it.
    ///
    /// To keep the whole realm in your own database, install a
    /// [`HostStoreMetaStorageProvider`](crate::host_store::HostStoreMetaStorageProvider):
    /// the realm's sessions, mob storage (and with it every identity's
    /// binding), fence and Meta's own records live in the host store, and the
    /// local directory (`scratch_dir()`, or one the runtime owns) is a
    /// disposable per-host cache. Such a build refuses `persistent_state()`,
    /// `mob_storage()`, `continuity_from_state_dir()`, a `topology_control()`
    /// mode other than Disabled, and `persistent_agent_memory_stack()`
    /// without a provider memory. Its `mob_announcements()` keep their tables
    /// in the realm too.
    ///
    /// [`MetaStorageProvider`]: crate::storage_provider::MetaStorageProvider
    pub fn storage_provider(
        mut self,
        provider: Arc<dyn crate::storage_provider::MetaStorageProvider>,
    ) -> Self {
        self.storage_provider = Some(provider);
        self
    }

    /// Declare the meerkat-level realm a non-disk storage provider's
    /// backend is keyed by. Required when the local state directory carries
    /// no realm evidence, for example an ephemeral `scratch_dir()`:
    /// `Fresh` for a new backend, `AdoptedPredecessor` for a backend the
    /// predecessor (pre-0.9) wrote. A declaration that contradicts the realm
    /// the state directory records refuses.
    pub fn meerkat_level_realm(
        mut self,
        declaration: crate::storage_provider::MeerkatLevelRealmDeclaration,
    ) -> Self {
        self.meerkat_realm_declaration = Some(declaration);
        self
    }

    /// Set a custom console log store.
    ///
    /// This lets applications pair a durable console timeline (for fast
    /// cursor-based history replay) with otherwise ephemeral mob state.
    pub fn with_console_log_store(mut self, store: Arc<dyn ConsoleLogStore>) -> Self {
        self.console_log_store = Some(store);
        self
    }

    /// Register the external live channel (`meta/live/*` over the live
    /// WebSocket router), the builder-path equivalent of the gateway's
    /// `runtime_options.live`.
    ///
    /// Requires persistent sessions ([`Self::persistent_state`], or a
    /// [`Self::storage_provider`] realm); `build()` fails closed with
    /// [`UnifiedRuntimeBuilderError::LiveCompose`] otherwise. The doors are
    /// composed by [`UnifiedRuntime::compose_live`] after the runtime is
    /// shared; [`UnifiedRuntime::build_reference_app_router`] does that and
    /// mounts the WebSocket router, so an embedder serving that router on
    /// its own listener gets `/live/ws` beside the console.
    pub fn live(mut self, options: super::live_compose::LiveOptions) -> Self {
        self.live = Some(options);
        self
    }

    /// Register console voice (gpt-live-1 through the authenticated console),
    /// the builder-path equivalent of the gateway's
    /// `runtime_options.console_voice`.
    ///
    /// Same persistent-store requirement as [`Self::live`]. The composed
    /// controller serves only authenticated console principals, so the
    /// decision state handed to [`UnifiedRuntime::build_reference_app_router`]
    /// must have `console.require_app_auth = true`; the router refuses to
    /// build otherwise. With [`Self::live`] registered as well, the two doors
    /// share one live context and take turns on its voice-path arbiter
    /// ("latest engaged wins"), exactly as in the gateway binaries.
    #[cfg(feature = "openai-live")]
    pub fn console_voice(
        mut self,
        registration: crate::public_live_config::PublicLiveRegistration,
    ) -> Self {
        self.console_voice = Some(registration);
        self
    }

    /// Enable or disable builtin tools (default: true).
    ///
    /// NOT a containment control for mob members. This sets the agent
    /// factory's runtime DEFAULT, and a default only applies where the
    /// per-session intent is `ToolCategoryOverride::Inherit`. A member built
    /// from an inline mob profile never inherits: `build_agent_config` maps
    /// every `profile.tools.*` boolean through
    /// `ToolCategoryOverride::from_effective`, which returns only
    /// `Enable`/`Disable` and never `Inherit`
    /// (`meerkat-core/src/session.rs:6656-6658`), and `resolve()` ignores the
    /// runtime default for those two (`:6626-6632`). So a profile declaring
    /// `tools.builtins = true` still gets builtins after `builtins(false)`.
    /// This flag governs sessions built through the same factory that are not
    /// profile-driven. For per-member containment set `profiles.<name>.tools`
    /// (or a `tool_access_policy`), not this.
    pub fn builtins(mut self, enabled: bool) -> Self {
        self.capability_flags.builtins = enabled;
        self
    }

    /// Enable or disable shell tool (default: true).
    ///
    /// A profile's `tools.shell` wins on the definition path; this is a
    /// factory default, not member containment. See [`Self::builtins`].
    pub fn shell(mut self, enabled: bool) -> Self {
        self.capability_flags.shell = enabled;
        self
    }

    /// Enable or disable mob tools (default: true).
    ///
    /// A profile's `tools.mob` wins on the definition path; see
    /// [`Self::builtins`]. Narrower still: the factory never reads its own
    /// mob default when composing, because `effective_mob` is derived from
    /// the build's mob operator authority context
    /// (`meerkat/src/factory.rs:5462-5467`).
    pub fn mob(mut self, enabled: bool) -> Self {
        self.capability_flags.mob = enabled;
        self
    }

    /// Enable or disable comms (default: true).
    ///
    /// A profile's `tools.comms` wins on the definition path; see
    /// [`Self::builtins`]. Note also that `tools.comms = false` is refused
    /// outright for mob members (`meerkat-mob/src/build.rs:170-173`), and the
    /// factory's own comms default is consulted only when the build carries
    /// no comms name, which the mob path always sets
    /// (`meerkat/src/factory.rs:4977-4981`, `meerkat-mob/src/build.rs:212`).
    pub fn comms(mut self, enabled: bool) -> Self {
        self.capability_flags.comms = enabled;
        self
    }

    /// Enable or disable memory tools (default: true).
    ///
    /// A profile's `tools.memory` wins on the definition path; this is a
    /// factory default, not member containment. See [`Self::builtins`].
    pub fn memory(mut self, enabled: bool) -> Self {
        self.capability_flags.memory = enabled;
        self
    }

    /// Force image-generation runtime substrate wiring.
    ///
    /// Definition-based builders also infer this from
    /// `profiles.<name>.tools.image_generation`; Meerkat owns the per-profile
    /// visibility decision.
    pub fn image_generation(mut self, enabled: bool) -> Self {
        self.capability_flags.image_generation = enabled;
        self
    }

    // -----------------------------------------------------------------------
    // Legacy API (preserved for backward compat)
    // -----------------------------------------------------------------------

    pub fn mob_spec(mut self, spec: MobBootstrapSpec) -> Self {
        self.mob_spec = Some(spec);
        self
    }

    pub fn module_config(mut self, config: MetaConfig) -> Self {
        self.module_config = Some(config);
        self
    }

    pub fn module_agent_events(mut self, events: Vec<EventEnvelope<UnifiedEvent>>) -> Self {
        self.module_agent_events = events;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn runtime_options(mut self, options: RuntimeOptions) -> Self {
        self.options = options;
        self
    }

    pub fn post_spawn_hook(mut self, hook: PostSpawnHook) -> Self {
        self.post_spawn_hook = Some(hook);
        self
    }

    pub fn post_reconcile_hook(mut self, hook: PostReconcileHook) -> Self {
        self.post_reconcile_hook = Some(hook);
        self
    }

    /// Register the operational error hook.
    ///
    /// Failures the runtime detects after bootstrap (a spawn that failed, a
    /// checkpoint that did not persist, a stalled actor loop) have no caller
    /// to return to; they are emitted as typed [`crate::ErrorEvent`]s through
    /// this hook. The hook is fire-and-forget: it runs on a detached task, and
    /// a slow or failing hook cannot break the runtime. An attached
    /// identity-first runtime receives the same hook.
    ///
    /// Every event is also logged through `tracing` (ERROR, or INFO for
    /// `ErrorEvent::ActorLoopRecovered`) whether or not a hook is registered,
    /// and when this method was never called [`Self::build`] emits one WARN
    /// saying that nobody is listening. Both are `tracing` lines, so a library
    /// host must install a `tracing` subscriber before `build`; without one it
    /// sees neither the failures nor the notice. The gateway binaries install
    /// a stderr subscriber themselves; an embedded host does that itself.
    ///
    /// Hosts that construct the runtime through [`UnifiedRuntime::bootstrap`]
    /// instead of the builder register the hook with
    /// [`UnifiedRuntime::set_error_hook`].
    pub fn on_error(mut self, hook: ErrorHook) -> Self {
        self.error_hook = Some(hook);
        self
    }

    /// Ingest the runtime's events into `config`'s store, with its filter and
    /// batching. Over a realm kept in a host store it stays available when
    /// the host supplies no event log store with the realm
    /// (`HostStoreMetaStores::event_log_store` is `None`); a build given both
    /// is refused.
    pub fn event_log(mut self, config: EventLogConfig) -> Self {
        self.event_log_config = Some(config);
        self
    }

    pub fn drain_timeout(mut self, timeout: Duration) -> Self {
        self.drain_timeout = Some(timeout);
        self
    }

    pub fn discovery(mut self, discovery: impl Discovery + 'static) -> Self {
        self.discovery = Some(Box::new(discovery));
        self
    }

    pub fn pre_spawn_hook(mut self, hook: PreSpawnHook) -> Self {
        *self
            .pre_spawn_hook
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
        self
    }

    pub fn edge_discovery(mut self, edge_discovery: impl EdgeDiscovery + 'static) -> Self {
        self.edge_discovery = Some(Box::new(edge_discovery));
        self
    }

    /// Set the contact directory for cross-mob address resolution.
    pub fn contact_directory(mut self, directory: ContactDirectory) -> Self {
        self.contact_directory = Some(directory);
        self
    }

    /// Bind a cross-mob control listener on startup so remote gateways can
    /// wire/unwire/inject/lookup members of this runtime.
    ///
    /// Accepts the contact-directory address spelling: `tcp://host:port`
    /// (port 0 binds an ephemeral port) or `uds:///path`. The concrete
    /// bound address is queryable after build via
    /// [`UnifiedRuntime::control_listener_advertised_address`].
    pub fn control_listen(mut self, addr: impl Into<String>) -> Self {
        self.control_listen = Some(addr.into());
        self
    }

    /// Authorize callers of the configured cross-mob control listener.
    ///
    /// Grants are bound to this runtime's mob id when the listener starts,
    /// so a signed request captured from one gateway cannot be replayed at
    /// another. Omitting this method is fail-closed: a configured listener
    /// still binds, but its empty grant table refuses every request.
    pub fn control_grants(
        mut self,
        grants: crate::runtime::cross_mob_control::ControlGrantTable,
    ) -> Self {
        self.control_grants = Some(grants);
        self
    }

    /// How the configured cross-mob control listener serves peers' keyed
    /// injects. Without it, an identity-first runtime serves them with the
    /// default freshness window, and any other runtime answers `not_served`.
    pub fn keyed_inject_plane(mut self, plane: crate::mob_announcements::KeyedInjectPlane) -> Self {
        self.keyed_inject = Some(plane);
        self
    }

    /// Enable mob announcements on this runtime, composed the way
    /// `rpc_gateway` composes `runtime_options.mob_announcements`
    /// ([`crate::mob_announcements::AnnouncementComposition`]): members
    /// announce to their mob and receive what they subscribe to in batches,
    /// and the host's own profiles that name
    /// [`crate::mob_announcements::tools::TOOL_BUNDLE`] in
    /// `tools.rust_bundles` get the member tools. With
    /// `member_tools_in_child_mobs` the bundle is offered to child mobs, and
    /// a child mob profile mounts it by naming it in its `tools.bundles`.
    ///
    /// Needs [`Self::persistent_state`], which holds the announcement
    /// database, or a host store realm ([`Self::storage_provider`] with a
    /// [`HostStoreMetaStorageProvider`](crate::host_store::HostStoreMetaStorageProvider)),
    /// which keeps the announcement tables in the realm, and an
    /// identity-first runtime ([`Self::roster_provider`]); `build()` refuses
    /// it otherwise, and refuses a configuration
    /// [`crate::mob_announcements::AnnouncementConfig::validate`] refuses.
    /// On a host store realm the process works on an in-memory copy of the
    /// tables that every changing call commits to the realm before it
    /// answers, and no source runner is scheduled here: the host's own
    /// scheduler fires each source's collector and calls
    /// [`advance_source`](crate::mob_announcements::AnnouncementService::advance_source).
    /// While [`Self::keyed_inject_plane`] serves (by default with
    /// [`crate::runtime::keyed_inject::DEFAULT_KEYED_INJECT_WINDOW_MS`]),
    /// its window is also the one this runtime assumes its peers apply to its
    /// own requests, in place of the configuration's
    /// `peer_request_window_ms`. The runtime's shutdown stops the dispatcher.
    #[must_use]
    pub fn mob_announcements(
        mut self,
        config: crate::mob_announcements::AnnouncementConfig,
    ) -> Self {
        self.mob_announcements = Some(config);
        self
    }

    /// Install a persistent metadata store. Used for the structural-events
    /// subscription cursor — see `runtime::metadata::PersistentMetadataStore`.
    /// When unset, the builder defaults to an `InMemoryMetadataStore`, which
    /// is correct for in-memory mob deployments. Production gateways with a
    /// SQLite mob storage should pass `SqliteMetadataStore::open(path)`
    /// against the same database the mob uses, so the structural-events
    /// subscription can resume from the last-projected cursor on restart
    /// rather than jumping forward to "latest" and dropping events emitted
    /// between processes.
    pub fn persistent_metadata(mut self, store: Arc<dyn PersistentMetadataStore>) -> Self {
        self.persistent_metadata = Some(store);
        self
    }

    /// Install a pre-built access controller (ABAC enforcement for the
    /// console and SSE surfaces). Absent — the default — access control is
    /// off and every surface behaves exactly as before.
    pub fn access_controller(mut self, controller: crate::access::AccessController) -> Self {
        self.access_controller = Some(controller);
        self
    }

    /// Let the Flow Editor on the reference app router run `rkat` on this
    /// host for callers that no enforced access controller governs. Off by
    /// default; see [`UnifiedRuntime::allow_flow_editor_host_deploy`].
    pub fn allow_flow_editor_host_deploy(mut self, allowed: bool) -> Self {
        self.flow_editor_host_deploy = allowed;
        self
    }

    /// Enable access control backed by an existing TOML file
    /// (conventionally `config/access.toml`); admin edits from the console
    /// persist back to the same path. A missing path, a directory, or an
    /// unreadable or invalid file is an error, never a disabled default.
    pub fn access_control_file(
        mut self,
        path: impl Into<PathBuf>,
    ) -> Result<Self, crate::access::AccessConfigError> {
        self.access_controller = Some(crate::access::AccessController::load(path)?);
        Ok(self)
    }

    /// Configure the optional topology control plane. The default policy is
    /// disabled, single-operation, local-authority only.
    ///
    /// The plane keeps its state in `topology-control.json` under
    /// [`persistent_state`](Self::persistent_state), so a realm kept in a host
    /// store ([`crate::host_store`]) refuses every mode but
    /// [`TopologyControlMode::Disabled`](crate::topology_control::TopologyControlMode::Disabled)
    /// at build. A [`TopologyProvider`]'s desired topology needs no such
    /// state: it is reconciled against the mob's own wiring, which the realm
    /// keeps.
    pub fn topology_control(
        mut self,
        policy: crate::topology_control::TopologyControlPolicy,
    ) -> Result<Self, crate::topology_control::TopologyControlError> {
        policy.validate()?;
        self.topology_control_policy = policy;
        Ok(self)
    }

    // -----------------------------------------------------------------------
    // Build
    // -----------------------------------------------------------------------

    pub async fn build(mut self) -> Result<UnifiedRuntime, UnifiedRuntimeBuilderError> {
        // --- Identity-first builder validation ---

        if self.mob_spec.is_some() && self.mob_storage.is_some() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "mob_storage() and mob_spec() are mutually exclusive: the supplied spec already owns mob storage".into(),
            ));
        }

        if self.mob_spec.is_some() && self.effective_mcp_call_context_provider().is_some() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "MCP context provider and document bindings require builder-owned session services; mob_spec() is already erased".into(),
            ));
        }

        if self.mob_spec.is_some() && self.operation_refusal_observer.is_some() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "an operation refusal observer requires builder-owned session services; mob_spec() is already erased".into(),
            ));
        }

        if self.mob_spec.is_some() && self.provider_auth_persistence.is_some() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "a provider credential store (provider_auth_persistence()) requires builder-owned session services; mob_spec() is already erased".into(),
            ));
        }

        if self.mob_spec.is_some() && self.fork_instruction_policy.is_some() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "fork_instruction_policy() and mob_spec() are mutually exclusive: the supplied spec carries its own policy (MobBootstrapSpec::with_fork_instruction_policy)".into(),
            ));
        }

        if self.mob_spec.is_some() && self.session_identity_config_root.is_some() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "session_identity_config_root() requires builder-owned session services; mob_spec() is already erased".into(),
            ));
        }

        // As the gateway refuses an empty runtime option: an empty root would
        // resolve the comms identity directory against the working directory.
        if self
            .session_identity_config_root
            .as_deref()
            .is_some_and(|root| root.as_os_str().to_string_lossy().trim().is_empty())
        {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "session_identity_config_root() must not be empty: an empty root would resolve the members' comms identity directory against the working directory".into(),
            ));
        }

        self.identity_bootstrap_mode
            .validate()
            .map_err(UnifiedRuntimeBuilderError::ConflictingConfiguration)?;

        // Validate the control-listen address up front so a typo fails the
        // build before any expensive bootstrap; the actual bind happens
        // after the runtime exists (near the end of build()).
        let control_listen = self
            .control_listen
            .as_deref()
            .map(crate::runtime::cross_mob_control::ControlListenAddr::parse)
            .transpose()
            .map_err(|error| {
                UnifiedRuntimeBuilderError::ConflictingConfiguration(format!(
                    "control_listen(): {error}"
                ))
            })?;

        let has_persistent_state = self.persistent_state_path.is_some();
        // Same-root pairing is a documented requirement of
        // `continuity_from_state_dir`; enforce it instead of letting two
        // different directories silently fork session authority from the
        // runtime/blob/workgraph stores.
        if let (Some(continuity_dir), Some(state_path)) = (
            self.continuity_state_dir.as_ref(),
            self.persistent_state_path.as_ref(),
        ) {
            verify_shared_state_root(continuity_dir, state_path)?;
        }
        // One path root per realm: the state directory and a scratch root
        // are competing path authorities.
        if has_persistent_state && self.scratch_dir.is_some() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "persistent_state() and scratch_dir() are mutually exclusive — one path root \
                 per realm"
                    .to_string(),
            ));
        }
        if self.blob_store.is_some() && self.binary_blob_store.is_some() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "blob_store() and binary_blob_store() are mutually exclusive — inject the one \
                 typed form you have"
                    .to_string(),
            ));
        }
        // Sessions live in the runtime store under the state directory: a
        // persistent build cannot declare that store in memory.
        if has_persistent_state && self.ephemeral_runtime_store {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "ephemeral_runtime_store(true) declares an in-memory runtime store, but \
                 persistent_state() is set: sessions live in the runtime store under the \
                 state directory, so a persistent build keeps it there. Remove the \
                 declaration, or build without persistent_state()"
                    .to_string(),
            ));
        }
        // The roster's identity bindings live in the mob storage: a persistent
        // identity-first build keeps it durable.
        if has_persistent_state
            && self.roster_provider.is_some()
            && self
                .mob_storage
                .as_ref()
                .is_some_and(|(_, provenance)| provenance.persistent_path().is_none())
        {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "mob_storage() supplies storage without a persistent path while \
                 persistent_state() and roster_provider() are set: the roster's identity \
                 bindings live in the mob storage, so they would not survive a restart while \
                 the sessions do. Omit mob_storage() to keep mob storage durable under the \
                 state directory, or supply persistent_mob_storage(path)"
                    .to_string(),
            ));
        }
        // 0.9.4 keeps identity continuity and leases in the state directory:
        // an injected pair is validated and handed on, but never read or
        // written (the native migration reads the 0.9.3 files itself). Keyed
        // on the caller's own calls: a storage provider and the state
        // directory's own substrate fill the same fields.
        if self.injected_continuity_seams {
            tracing::warn!(
                "continuity_store() and lease_provider() are ignored since 0.9.4: identity \
                 continuity and leases live in the state directory. Migrate 0.9.3 identities \
                 with the continuity export; to keep an external copy, use continuity_mirror()"
            );
        }
        // The topology control plane keeps its state in a local file, which a
        // host store realm does not treat as authority.
        if self.topology_control_policy.mode
            != crate::topology_control::TopologyControlMode::Disabled
            && self
                .storage_provider
                .as_ref()
                .is_some_and(|provider| provider.host_store_realm().is_some())
        {
            let mode = self.topology_control_policy.mode;
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                format!(
                    "topology_control() with mode {mode:?} is not available on a host store \
                 realm: the topology control plane keeps its state in a local file, which \
                 such a realm does not treat as authority. Keep the mode Disabled; a \
                 TopologyProvider's desired topology is reconciled against the mob's wiring, \
                 which the realm keeps"
                ),
            ));
        }
        self.materialize_storage_provider().await?;

        let has_continuity_store = self.continuity_store.is_some();
        let has_lease_provider = self.lease_provider.is_some();
        let has_roster_provider = self.roster_provider.is_some();
        let has_topology_provider = self.topology_provider.is_some();
        // A2 decouple: agent memory is keyed by AgentIdentity, which every
        // mob member already has, so enabling it must NOT pull in the
        // identity-first orchestration layer (roster/continuity/leases).
        // Without a roster the BASIC memory surface (recorder tool +
        // build-time injection + panel store) rides the classic path via a
        // `MemorySpawnCustomizer`; with a roster, memory composes into the
        // IdentityRuntime customizer exactly as before (advanced lifecycle
        // features included).
        let has_agent_customizer = self.agent_customizer.is_some();
        let has_identity_runtime_instance_id = self.identity_runtime_instance_id.is_some();
        let has_scratch_dir = self.scratch_dir.is_some();
        let has_external_identity_storage =
            has_continuity_store || has_lease_provider || has_scratch_dir;
        let wants_identity_first = has_external_identity_storage
            || has_roster_provider
            || has_topology_provider
            || has_agent_customizer
            || has_identity_runtime_instance_id
            || self.identity_bootstrap_mode_configured;
        // REQ-23, lifted (M4): an external continuity/lease pair may coexist
        // with persistent_state(). Since 0.9.4 the pair is ignored (see the
        // warning above): the state directory holds identities and sessions.
        // Half a pair keeps its typed error until both methods are removed.
        if has_persistent_state && (has_continuity_store != has_lease_provider) {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "continuity_store() and lease_provider() must be supplied together — the lease \
                 authority's fencing floor is coupled to its continuity store"
                    .to_string(),
            ));
        }

        if has_persistent_state && wants_identity_first && !has_roster_provider {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "persistent_state() identity-first path requires roster_provider()".to_string(),
            ));
        }

        // REQ-24: external path requires all three storage inputs plus a roster.
        // A host store realm is that external path's authority on its own.
        if !has_persistent_state
            && wants_identity_first
            && self.host_store.is_none()
            && !(has_continuity_store
                && has_lease_provider
                && has_roster_provider
                && has_scratch_dir)
        {
            let mut missing = Vec::new();
            if !has_continuity_store {
                missing.push("continuity_store");
            }
            if !has_lease_provider {
                missing.push("lease_provider");
            }
            if !has_roster_provider {
                missing.push("roster_provider");
            }
            if !has_scratch_dir {
                missing.push("scratch_dir");
            }
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                format!(
                    "identity-first path requires continuity_store() + lease_provider() + \
                     roster_provider() + scratch_dir(); missing: {}",
                    missing.join(", ")
                ),
            ));
        }
        // Without a state directory, a non-disk provider's backend or a
        // caller's mob spec, the sessions of an identity-first build live in
        // an in-memory runtime store: a restart would start every bound
        // identity with an empty transcript. That is a declaration, never a
        // default.
        if !has_persistent_state
            && wants_identity_first
            && self.provider_meerkat_stores.is_none()
            && self.mob_spec.is_none()
            && !self.ephemeral_runtime_store
        {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "this identity-first build keeps its sessions in an in-memory runtime store \
                 (no persistent_state(), no non-disk storage_provider(), no mob_spec()), so a \
                 restart would start every identity with an empty transcript. Build with \
                 persistent_state(), or with a storage_provider() whose backend keeps the \
                 sessions, or declare ephemeral_runtime_store(true) if sessions may end with \
                 the process"
                    .to_string(),
            ));
        }

        // Mob announcements compose through the same two steps as on every
        // gateway: refuse what cannot compose before anything is built, and
        // put the member tool bundle on the spec with the builder's own.
        let announcements = match self.mob_announcements.take() {
            Some(config) => {
                // A host store realm keeps the announcement tables itself. An
                // identity-first runtime on any other external substrate has
                // no state directory to hold the announcement database.
                if self.host_store.is_none() && !has_persistent_state {
                    return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                        "mob_announcements() requires persistent_state(), which holds the \
                         announcement database, or a host store realm (storage_provider()), \
                         which keeps its tables"
                            .to_string(),
                    ));
                }
                if !wants_identity_first {
                    return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                        "mob_announcements() requires an identity-first runtime \
                         (roster_provider())"
                            .to_string(),
                    ));
                }
                let mut composition = crate::mob_announcements::AnnouncementComposition::new(
                    config,
                )
                .map_err(|error| {
                    UnifiedRuntimeBuilderError::ConflictingConfiguration(format!(
                        "mob_announcements(): {error}"
                    ))
                })?;
                if let crate::mob_announcements::KeyedInjectPlane::Serve { window_ms } =
                    self.keyed_inject.unwrap_or_default()
                {
                    composition = composition.with_peer_request_window(window_ms);
                }
                let (name, dispatcher, availability) = composition.member_tool_bundle();
                if self.tool_bundles.contains_key(name) {
                    return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                        format!(
                            "tool bundle '{name}' is the mob announcement member tools, which \
                         mob_announcements() registers"
                        ),
                    ));
                }
                self.tool_bundles
                    .insert(name.to_string(), (dispatcher, availability));
                Some(composition)
            }
            None => None,
        };
        // One Meta runtime per state directory: claim it once the
        // configuration is known to be coherent (a refused one leaves the
        // directory untouched) and before the identity substrate or any local
        // store opens in it. A storage provider claimed it already.
        if let Some(state_path) = self.persistent_state_path.clone() {
            let runtime_store = if self.provider_meerkat_stores.is_some() {
                crate::native_startup::RuntimeStoreLocation::StorageProvider
            } else {
                crate::native_startup::RuntimeStoreLocation::StateDir
            };
            self.claim_state_dir(&state_path, runtime_store).await?;
        }

        #[cfg(feature = "extension-state")]
        if (!self.tool_bundle_factories.is_empty() || !self.mcp_document_bindings.is_empty())
            && wants_identity_first
            && self.continuity_store.is_none()
        {
            let path = self.persistent_state_path.clone().ok_or_else(|| {
                UnifiedRuntimeBuilderError::ConflictingConfiguration(
                    "extension documents require a configured persistent identity authority".into(),
                )
            })?;
            self = self.continuity_from_state_dir(path).await?;
        }

        // Strict mode: the SQLite session store `persistent_state()` composes
        // is incremental, so only a storage provider's realm session store
        // can fail this.
        if self.require_incremental_session_persistence
            && let Some(stores) = self.provider_meerkat_stores.as_ref()
            && Arc::clone(&stores.session_store).as_incremental().is_none()
        {
            return Err(UnifiedRuntimeBuilderError::SessionStoreNotIncremental {
                store_kind: format!("{} session store", stores.provider_name),
            });
        }

        // Legacy mob_spec path takes precedence — must be consumed before
        // resolve_mob_spec (which borrows &self for the definition path).
        let mut mob_spec = match self.mob_spec.take() {
            Some(spec) => {
                // Legacy path: require module_config and timeout as before.
                if self.module_config.is_none() {
                    return Err(UnifiedRuntimeBuilderError::MissingRequiredField(
                        UnifiedRuntimeBuilderField::ModuleConfig,
                    ));
                }
                if self.timeout.is_none() {
                    return Err(UnifiedRuntimeBuilderError::MissingRequiredField(
                        UnifiedRuntimeBuilderField::Timeout,
                    ));
                }
                // A legacy spec arrives with its session service (and thus
                // its compactor) already built from the composer's own
                // `meerkat::Config`. Accepting a compaction declaration here
                // would produce exactly the dead knob this seam exists to
                // remove, so it refuses instead of silently doing nothing.
                if self
                    .grant_configuration
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_some()
                {
                    return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                        "native_grant_work_configuration() and mob_spec() are mutually \
                         exclusive: a pre-built MobBootstrapSpec already constructed its \
                         runtime machine, so the grant owner could not be installed on it"
                            .to_string(),
                    ));
                }
                if self.compaction_policy.is_some() {
                    return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                        "compaction() and mob_spec() are mutually exclusive — a pre-built \
                         MobBootstrapSpec already carries its session service's compaction \
                         policy; declare it on the spec's own meerkat::Config"
                            .to_string(),
                    ));
                }
                spec
            }
            None => self.resolve_mob_spec().await?,
        };
        // The runtime keeps the state directory's claim until its shutdown
        // has run to its end; a resolved spec already carries it, a supplied
        // one may not.
        if let Some(claim) = self.state_dir_claim.take()
            && !mob_spec
                .state_dir_claims
                .iter()
                .any(|held| Arc::ptr_eq(held, &claim))
        {
            mob_spec.state_dir_claims.push(claim);
        }
        // The session store the stock constructors composed, for the agent
        // memory engines below.
        let composed_session_store = mob_spec.durable_session_store.clone();
        // Never skip a supplied grant owner: every spec path that builds a
        // runtime machine consumes it, and an unconsumed one is refused.
        if self.take_grant_configuration().is_some() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "native_grant_work_configuration() was not installed: this composition \
                 builds no runtime machine to install the grant owner on"
                    .to_string(),
            ));
        }

        #[cfg(feature = "extension-state")]
        let extension_authority = self.materialize_extension_factories(&mut mob_spec).await?;

        // A host store realm keeps the members, not their comms identities:
        // the census names the per-host directory that holds them, and a
        // restored member whose key pair is missing there restores broken,
        // which the census and the log name.
        if self.host_store.is_some() {
            let slot = crate::host_store::comms_identity_census(
                self.session_identity_config_root.as_deref(),
                mob_spec.member_model_config.as_deref(),
                &mob_spec.storage,
            )
            .await;
            if slot.degraded {
                tracing::error!(
                    detail = slot.detail.as_deref().unwrap_or_default(),
                    "restored members of the host store realm have no comms identity on this host"
                );
                if let Some(host) = self.host_store.as_ref() {
                    host.realm.record_comms_identity_degraded();
                }
            }
            self.provider_slot_census.push(slot);
        }

        // Provider-declared durability flows to the census verbatim (M4b):
        // replace the locally-labeled slots for the domains the provider
        // declared and add the slots the local composition has no
        // equivalent record for (continuity, event_log, console, metadata,
        // agent_memory).
        if !self.provider_slot_census.is_empty()
            && let Some(summary) = mob_spec.resolved_storage.as_mut()
        {
            let provider_census = std::mem::take(&mut self.provider_slot_census);
            summary.slots.retain(|slot| {
                !provider_census.iter().any(|provider_slot| {
                    provider_slot.durability.domain() == slot.durability.domain()
                })
            });
            summary.slots.extend(provider_census);
        }

        for (name, (dispatcher, availability)) in std::mem::take(&mut self.tool_bundles) {
            if mob_spec.tool_bundles.contains_key(&name) {
                return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                    format!(
                        "tool bundle '{name}' is registered on both the builder and the supplied \
                     MobBootstrapSpec"
                    ),
                ));
            }
            mob_spec.tool_bundles.insert(name.clone(), dispatcher);
            mob_spec
                .child_tool_bundle_availability
                .insert(name, availability);
        }
        if let Some(binding) = self.child_application_tool_policy.take() {
            if mob_spec.child_application_tool_policy.is_some() {
                return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                    "a child application tool policy is set on both the builder and the \
                     supplied MobBootstrapSpec"
                        .to_string(),
                ));
            }
            mob_spec.child_application_tool_policy = Some(binding);
        }
        if let Some(hook) = self.member_runtime_registration_hook.take() {
            if mob_spec.member_runtime_registration_hook.is_some() {
                return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                    "a member registration hook is set on both the builder and the supplied \
                     MobBootstrapSpec"
                        .to_string(),
                ));
            }
            mob_spec.member_runtime_registration_hook = Some(hook);
        }
        if let Some(registry) = self.tool_consequence_policy_registry.take() {
            if mob_spec.tool_consequence_policy_registry.is_some() {
                return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                    "a tool consequence-policy registry is set on both the builder and the \
                     supplied MobBootstrapSpec"
                        .to_string(),
                ));
            }
            mob_spec.tool_consequence_policy_registry = Some(registry);
        }

        let module_config = self.module_config.take().unwrap_or_else(|| MetaConfig {
            modules: Vec::new(),
            discovery: crate::types::DiscoverySpec {
                namespace: String::new(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        });
        let timeout = self.timeout.unwrap_or(DEFAULT_TIMEOUT);
        if let Some(state_path) = self.persistent_state_path.as_ref() {
            std::fs::create_dir_all(state_path).map_err(|e| {
                UnifiedRuntimeBuilderError::Io(format!(
                    "failed to create state directory at {}: {e}",
                    state_path.display()
                ))
            })?;
        }
        // The path authority for every state-dir file name below:
        // canonical-name-first probing keeps legacy spellings working where
        // they lie; twin spellings refuse loudly instead of forking history.
        let storage_layout = self
            .persistent_state_path
            .as_ref()
            .map(|path| MetaStorageLayout::with_injected_roots(path.clone(), None));
        // Full-stack path: the stack provider exists pre-runtime so it can
        // serve as the provider (recorder + recall) from the first spawn;
        // the firewall + engines attach post-construction, when the memory
        // event sink and mob handle exist. An explicit `agent_memory()`
        // provider is used as-is — `attach_memory_engines` capability-checks
        // it; otherwise the bundled SQLite store opens under the layout root.
        let stack_provider: Option<Arc<dyn AgentMemoryProvider>> =
            if self.agent_memory_engines.is_some() {
                if let Some(provider) = self.agent_memory_provider.clone() {
                    Some(provider)
                } else {
                    // A host store realm keeps no local store as authority:
                    // the stack runs over the provider's memory there.
                    if self.host_store.is_some() {
                        return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                            "persistent_agent_memory_stack() needs a provider memory on a host \
                             store realm: its bundled store is a local SQLite file, which such \
                             a realm does not treat as authority. Supply an agent memory \
                             provider (HostStoreMetaStores::agent_memory_provider, or \
                             agent_memory())"
                                .to_string(),
                        ));
                    }
                    let Some(layout) = storage_layout.as_ref() else {
                        return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                            "persistent_agent_memory_stack() requires persistent_state()"
                                .to_string(),
                        ));
                    };
                    // One slot for both store kinds: a legacy
                    // `agent-memory-sqlite/` corpus keeps being used where it
                    // lies; fresh deployments get the canonical
                    // `agent-memory/` root.
                    let memory_path = layout.agent_memory_root()?.path;
                    Some(Arc::new(
                        crate::memory::sqlite_store::SqliteAgentMemoryStore::open(&memory_path)
                            .map_err(|e| {
                                UnifiedRuntimeBuilderError::Io(format!(
                                    "failed to open agent memory store at {}: {e}",
                                    memory_path.display()
                                ))
                            })?,
                    ))
                }
            } else {
                None
            };
        let agent_memory_provider = self
            .agent_memory_provider
            .clone()
            .or_else(|| stack_provider.clone());
        let agent_memory_injector = agent_memory_provider.as_ref().map(|provider| {
            AgentMemoryRuntimeInjector::new(
                provider.clone(),
                self.agent_memory_config.clone().unwrap_or_default(),
            )
        });
        let agent_customizer = self.composed_agent_customizer(agent_memory_provider.clone());

        // Classic (roster-less) agent memory: register the per-spawn memory
        // customizer on the mob runtime itself, so every member spawn —
        // consumer, agent-tool, policy, respawn, resume — gets the recorder
        // tool and the build-time injection keyed on its AgentIdentity. The
        // identity-first path keeps its AgentCustomizer instead (composing
        // both would double-inject on identity-first materializations).
        let classic_agent_memory = if wants_identity_first {
            None
        } else {
            agent_memory_provider.clone()
        };
        if let Some(provider) = classic_agent_memory.as_ref() {
            // Composed after a spawn customizer the spec already carries,
            // never replacing it: the host's runs first, and memory keys on
            // the identity it leaves.
            mob_spec.spawn_member_customizer =
                Some(crate::identity_first::ComposedSpawnMemberCustomizer::over(
                    mob_spec.spawn_member_customizer.take(),
                    Arc::new(crate::memory::MemorySpawnCustomizer::new(
                        provider.clone(),
                        self.agent_memory_config.clone().unwrap_or_default(),
                    )),
                ));
        }
        // Identity-first `customize_build` tools follow each member across
        // meerkat-side rebuilds through one stable dispatcher per identity
        // (#563). Composed into the single customizer slot, never replacing.
        let customizer_tool_registry = (wants_identity_first && agent_customizer.is_some())
            .then(crate::identity_first::CustomizerToolRegistry::new);
        if let Some(registry) = customizer_tool_registry.as_ref() {
            mob_spec.spawn_member_customizer =
                Some(crate::identity_first::ComposedSpawnMemberCustomizer::over(
                    mob_spec.spawn_member_customizer.take(),
                    Arc::new(crate::identity_first::CustomizerToolsSpawnCustomizer::new(
                        registry.clone(),
                    )),
                ));
        }

        // The structural-events subscription cursor lives in the
        // persistent metadata adapter. For ephemeral builds this can be
        // in-memory (the ledger itself isn't durable, so there's nothing
        // to resume from). For persistent_state builds we MUST default
        // to SQLite — otherwise after a gateway restart the subscriber
        // resumes from `latest_cursor` and silently skips every event
        // that was written to the durable mob ledger while Meta was
        // down. Callers can still override via `.persistent_metadata()`.
        let persistent_metadata: Arc<dyn PersistentMetadataStore> =
            if let Some(store) = self.persistent_metadata.clone() {
                store
            } else if let Some(layout) = storage_layout.as_ref() {
                let metadata_path = layout.metadata_db()?.path;
                Arc::new(SqliteMetadataStore::open(&metadata_path).map_err(|e| {
                    UnifiedRuntimeBuilderError::Io(format!(
                        "failed to open the meta metadata store at {}: {e}",
                        metadata_path.display()
                    ))
                })?)
            } else {
                Arc::new(InMemoryMetadataStore::new())
            };
        let console_log_store: Arc<dyn ConsoleLogStore> =
            if let Some(store) = self.console_log_store.clone() {
                store
            } else if let Some(layout) = storage_layout.as_ref() {
                let console_log_path = layout.console_db()?.path;
                Arc::new(SqliteConsoleLogStore::open(&console_log_path).map_err(|e| {
                    UnifiedRuntimeBuilderError::Io(format!(
                        "failed to open the meta console store at {}: {e}",
                        console_log_path.display()
                    ))
                })?)
            } else {
                Arc::new(InMemoryConsoleLogStore::new())
            };
        // §10.1 dispatch-time taint join: keep the spec's late-bound slot so
        // the full-stack path below can bind the stack's tracker into every
        // member build (including bootstrap members built before the stack
        // attaches - their decorators read the slot per call).
        let dispatch_taint_slot = mob_spec.dispatch_taint_slot();
        let live_plan = self.live_plan(&mut mob_spec)?;
        // Resolve one roster for this boot before preparing the mob. A dirty
        // Running log can restore members inside prepare, before the identity
        // runtime itself is constructed: the mob build's early customizer
        // publication, the run-start holds it stages for those members and
        // the identity bootstrap all use this one roster, so a hold never
        // names an identity the bootstrap's roster lacks.
        //
        // The roster is resolved from the definition the mob runs: on a
        // resume the stored one, judged against the supplied composition
        // first, so a resume preparation would refuse is refused before the
        // roster provider sees its definition. Preparation then resumes
        // exactly what was judged here, even when the supplied definition
        // differs (a candidate launch that boots the stored one knowingly).
        let identity_roster_specs = if wants_identity_first {
            let provider = self.roster_provider.as_ref().ok_or_else(|| {
                UnifiedRuntimeBuilderError::Bootstrap(UnifiedRuntimeBootstrapError::IdentityFirst(
                    "identity-first validation requires roster_provider".to_string(),
                ))
            })?;
            let launch_definition = mob_spec.read_launch_definition().await.map_err(|error| {
                UnifiedRuntimeBuilderError::Bootstrap(UnifiedRuntimeBootstrapError::Mob(error))
            })?;
            Some(
                provider
                    .roster(&RosterContext {
                        mob_definition: Some(launch_definition),
                        previous_identities: Vec::new(),
                    })
                    .await
                    .map_err(|error| {
                        UnifiedRuntimeBuilderError::Bootstrap(
                            UnifiedRuntimeBootstrapError::IdentityFirst(format!(
                                "roster provider failed before mob preparation: {error}"
                            )),
                        )
                    })?,
            )
        } else {
            None
        };
        // A persistent mob can be restored while `bootstrap` builds it (a log
        // that was left Running revives every member inside meerkat's own
        // resume), before any identity activation step. Publish each roster
        // identity's customizer tools first, so those builds carry them (#563).
        // The mob is durable under a state directory or in a host store realm.
        let durable_mob = storage_layout.is_some() || self.host_store.is_some();
        let mut early_customizer_tools_pending = match (
            customizer_tool_registry.as_ref(),
            identity_roster_specs.as_ref(),
            agent_customizer.as_ref(),
            durable_mob,
        ) {
            (Some(registry), Some(roster), Some(customizer), true) => {
                crate::identity_first::customizer_tools::prepublish_before_mob_build(
                    registry,
                    roster,
                    customizer.as_ref(),
                    AgentRuntimeServices::empty(),
                )
                .await
            }
            _ => BTreeMap::new(),
        };
        // That resume revives a member whose identity this roster no longer
        // lists as well, and no publication covers it: it is held until the
        // roster pass retires it. A host-adopted identity's member, which the
        // roster pass never retires, is not held.
        if let (Some(_), Some(roster), true) = (
            customizer_tool_registry.as_ref(),
            identity_roster_specs.as_ref(),
            durable_mob,
        ) {
            let stored = mob_spec
                .stored_command_driven_identity_members()
                .await
                .map_err(|error| {
                    UnifiedRuntimeBuilderError::Bootstrap(UnifiedRuntimeBootstrapError::Mob(error))
                })?;
            early_customizer_tools_pending.extend(
                crate::identity_first::customizer_tools::unpublished_before_mob_build(
                    crate::identity_first::customizer_tools::unlisted_stored_identities(
                        roster, &stored,
                    ),
                ),
            );
        }
        // A restored member without its tools must not start a run before its
        // materialization publishes them: meerkat holds its run starts from
        // registration until the identity runtime releases the hold.
        mob_spec = mob_spec.with_restored_members_awaiting_tools(&early_customizer_tools_pending);
        let module_agent_events = self.module_agent_events;
        let options = self.options;
        #[allow(unused_mut)]
        let mut runtime = crate::boxed_future::boxed(|| {
            UnifiedRuntime::bootstrap_with_options(
                mob_spec,
                module_config,
                module_agent_events,
                timeout,
                options,
                persistent_metadata,
            )
        })
        .await
        .map_err(|error| {
            UnifiedRuntimeBuilderError::Bootstrap(if wants_identity_first {
                name_a_definition_change_after_the_roster(error)
            } else {
                error
            })
        })?;

        #[cfg(feature = "extension-state")]
        {
            runtime.extension_authority = extension_authority;
        }

        // Construct session bridge from the mob handle for identity-first wiring.
        // Available for BOTH persistent_state and external-authoritative paths —
        // the bridge connects the identity-first control plane to real sessions.
        let session_bridge: Option<Arc<dyn crate::identity_first::bridge::SessionBridge>> = {
            // The composition's own bridge (kept in its state and inside its
            // identity runtime) is a back-edge, so it is non-owning; exports
            // carry the public owner separately (`PublicIdentityRuntime`,
            // `PublicSessionBridge`).
            let handle = runtime.mob_runtime.handle();
            let session_service = runtime.mob_runtime.session_service().cloned();
            let mut bridge: crate::identity_first::bridge::MobSessionBridge =
                if let Some(service) = session_service {
                    crate::identity_first::bridge::MobSessionBridge::with_session_service(
                        handle, service,
                    )
                } else {
                    crate::identity_first::bridge::MobSessionBridge::new(handle)
                };
            // Heal seam (2026-07-29 incident): without the recoverer, the
            // continuity repair supervisor can only reset entries — a
            // cosmetic heal that materialization re-Breaks when the durable
            // head is an intra-turn projection.
            if let Some(recoverer) = runtime.mob_runtime.committed_boundary_recoverer() {
                bridge = bridge.with_committed_boundary_recoverer(recoverer);
            }
            // Image deliveries are judged with the member builder's host
            // config, so a self-hosted model's `vision` flag counts.
            if let Some(config) = runtime.mob_runtime.member_model_config() {
                bridge = bridge.with_member_model_config(config);
            }
            Some(Arc::new(bridge) as Arc<dyn crate::identity_first::bridge::SessionBridge>)
        };

        // Construct the durable control-plane authority before identity
        // restore. TopologyProvider output is only a declaration; the
        // IdentityRuntime composes this controller's additions/suppressions
        // under the same admission lock before it touches peer wiring.
        let topology_controller = if let Some(state_path) = self.persistent_state_path.as_ref() {
            crate::topology_control::TopologyController::load_or_default(
                self.topology_control_policy.clone(),
                state_path.join("topology-control.json"),
            )
            .map_err(|error| UnifiedRuntimeBuilderError::Io(error.to_string()))?
        } else {
            crate::topology_control::TopologyController::new(self.topology_control_policy.clone())
                .map_err(|error| UnifiedRuntimeBuilderError::Io(error.to_string()))?
        };
        topology_controller
            .bind_authority(runtime.mob_id())
            .await
            .map_err(|error| UnifiedRuntimeBuilderError::Io(error.to_string()))?;

        let (identity_first_context, identity_roster_specs) = if wants_identity_first {
            let (continuity_store, lease_provider): (
                Arc<dyn crate::identity_first::contracts::ContinuityStore>,
                Arc<dyn crate::identity_first::contracts::LeaseProvider>,
            ) = if let (Some(continuity_store), Some(lease_provider)) =
                (self.continuity_store.clone(), self.lease_provider.clone())
            {
                // An explicit external pair (or a storage provider's realm
                // set) is the identity authority even alongside
                // persistent_state() — the REQ-23 lift (M4).
                (continuity_store, lease_provider)
            } else if let Some(layout) = storage_layout.as_ref() {
                let continuity_path = layout.continuity_db()?.path;
                let (local_store, high_water) =
                    LocalContinuityStore::open_with_fencing_floor(continuity_path.clone())
                        .await
                        .map_err(|e| {
                            UnifiedRuntimeBuilderError::Io(format!(
                                "failed to open the identity continuity store and read its fencing high-water at {}: {e}",
                                continuity_path.display()
                            ))
                        })?;
                (
                    Arc::new(local_store),
                    Arc::new(LocalLeaseProvider::with_floor(high_water)),
                )
            } else {
                let Some(continuity_store) = self.continuity_store.clone() else {
                    return Err(UnifiedRuntimeBuilderError::Bootstrap(
                        UnifiedRuntimeBootstrapError::IdentityFirst(
                            "identity-first validation requires continuity_store".to_string(),
                        ),
                    ));
                };
                let Some(lease_provider) = self.lease_provider.clone() else {
                    return Err(UnifiedRuntimeBuilderError::Bootstrap(
                        UnifiedRuntimeBootstrapError::IdentityFirst(
                            "identity-first validation requires lease_provider".to_string(),
                        ),
                    ));
                };
                (continuity_store, lease_provider)
            };
            let Some(roster_provider) = self.roster_provider.clone() else {
                return Err(UnifiedRuntimeBuilderError::Bootstrap(
                    UnifiedRuntimeBootstrapError::IdentityFirst(
                        "identity-first validation requires roster_provider".to_string(),
                    ),
                ));
            };
            let bridge = session_bridge.clone();
            let identity_runtime = Arc::new(
                IdentityRuntime::new(IdentityRuntimeConfig {
                    continuity_store,
                    lease_provider,
                    runtime_instance_id: self
                        .identity_runtime_instance_id
                        .clone()
                        .unwrap_or_else(|| format!("meta-{}", std::process::id())),
                    has_runtime_store: true,
                    durability_policy: DurabilityPolicy::SyncWriteThrough,
                    bridge,
                    default_timeout: None,
                })
                .with_runtime_services(AgentRuntimeServices::new(runtime.mob_runtime.handle()))
                .with_reset_roster_provider_context(
                    roster_provider.clone(),
                    Some(runtime.mob_runtime.handle().definition().clone()),
                ),
            );
            identity_runtime
                .set_agent_customizer(agent_customizer.clone())
                .await;
            identity_runtime
                .set_customizer_tool_registry(customizer_tool_registry.clone())
                .await;
            identity_runtime.record_customizer_tools_pending(early_customizer_tools_pending);
            // A candidate launch's mob build held every stored member's run
            // starts, and it releases none of them.
            if !runtime.mob_runtime.speaks_for_composition() {
                identity_runtime.mark_candidate_launch();
            }
            identity_runtime
                .set_agent_memory(agent_memory_injector.clone())
                .await;
            identity_runtime.set_error_hook(self.error_hook.clone());
            // The repair commands in a session repair hold name the runtime
            // store this build opened (recorded by the composition that
            // opened it); an explicit declaration wins, and a build without
            // a persistent runtime store declares no scope.
            identity_runtime.set_session_repair_scope(self.session_repair_scope.clone().or_else(
                || {
                    runtime
                        .mob_runtime
                        .resolved_storage()
                        .as_ref()
                        .and_then(crate::identity_first::SessionRepairScope::from_resolved_storage)
                },
            ));
            identity_runtime.set_topology_controller(topology_controller.clone());
            if let Some(host) = self.host_store.as_ref() {
                // A host store realm keeps both as its own documents: the
                // first-boot rows an import leaves, and the held-completion
                // marks, read on whichever host the runtime starts.
                let documents = host.realm.documents().clone();
                identity_runtime.set_first_boot_location(Some(
                    crate::identity_first::first_boot_manifest::FirstBootLocation::HostStore(
                        documents.clone(),
                    ),
                ));
                identity_runtime.set_held_completion_host_store(documents);
            } else {
                // `storage-migrate-native` leaves the first-boot manifest of a
                // migrated state directory there; the first roster
                // compile carries it out.
                identity_runtime.set_first_boot_state_dir(self.persistent_state_path.clone());
                // A held bootstrap's held-completion marks live in the same
                // directory, so a restart keeps them.
                identity_runtime.set_held_completion_state_dir(self.persistent_state_path.clone());
            }
            // Extension documents refuse a principal to an unmarked member
            // of an identity this runtime registers.
            #[cfg(feature = "extension-state")]
            if let Some(registry) = runtime.extension_authority.as_ref() {
                registry.bind_identity_runtime(&identity_runtime);
            }

            let roster_specs = identity_roster_specs.ok_or_else(|| {
                UnifiedRuntimeBuilderError::Bootstrap(UnifiedRuntimeBootstrapError::IdentityFirst(
                    "identity roster was not resolved before mob preparation".to_string(),
                ))
            })?;

            let identity_context = Arc::new(IdentityFirstRuntimeContext::new_with_bootstrap_mode(
                identity_runtime,
                roster_provider,
                self.topology_provider.clone(),
                agent_customizer.clone(),
                Some(runtime.mob_runtime.handle().definition().clone()),
                self.identity_bootstrap_mode.clone(),
            ));
            (Some(identity_context), Some(roster_specs))
        } else {
            (None, None)
        };
        // Set immutable outer fields by rebuilding the struct
        let (core, public_owner) = runtime.into_parts();
        let mut runtime = UnifiedRuntime::from_parts_unchecked(
            super::UnifiedRuntimeCore {
                access_controller: self.access_controller,
                flow_editor_host_deploy: self.flow_editor_host_deploy,
                // A host deploy cannot carry the builder's credential store.
                explicit_provider_credentials: self.provider_auth_persistence.is_some(),
                topology_controller,
                post_spawn_hook: self.post_spawn_hook,
                post_reconcile_hook: self.post_reconcile_hook,
                drain_timeout: self.drain_timeout.unwrap_or(DEFAULT_DRAIN_TIMEOUT),
                discovery: self.discovery,
                // A custom embedder policy overrides the definition-derived
                // default `bootstrap_with_options` installed (a downstream app,
                // 2026-07-09); with none supplied the default is preserved.
                edge_discovery: self
                    .edge_discovery
                    .map(std::sync::Arc::<dyn EdgeDiscovery>::from)
                    .or(core.edge_discovery),
                contact_directory: self.contact_directory,
                session_bridge,
                identity_first_context,
                console_log_store,
                ..core
            },
            public_owner,
        );
        runtime.set_live_plan(live_plan);
        // The error hook lives in a late-bound shared slot created at
        // construction (runtime-owned tasks already hold it), so a
        // builder-supplied hook installs into the slot rather than
        // replacing it.
        if let Some(hook) = self.error_hook {
            runtime.set_error_hook(hook);
        } else {
            // Say so at startup rather than at the first failure: a host that
            // never calls `on_error` gets operational events in logs only, and
            // that should be a decision it made rather than one it discovers
            // months later while reading a console table by hand. Checked here
            // rather than inside `bootstrap` because the hook installs AFTER
            // construction, so a bootstrap-time check would fire for every
            // correctly-wired host too.
            super::emit_error_hook_absent_notice();
        }

        // Run the host's last fallible pre-bootstrap hook before identity
        // hydration can start any runtime-owned background work. In
        // particular, LazyWithBackgroundWarm spawns materialization from
        // bootstrap_roster; a later hook failure must not detach that task or
        // leave its leases alive after build() returns an error.
        //
        // The mob is already bootstrapped here, so its supervisor in-proc
        // route is live; under meerkat 0.8.23's fail-closed registration an
        // ordinary-drop return would leave the name occupied and block any
        // same-process rebuild of this mob id. Failure must follow the same
        // cooperative shutdown path as the later bootstrap errors.
        let pre_spawn_hook = self
            .pre_spawn_hook
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pre_spawn_context = if let Some(hook) = pre_spawn_hook {
            match hook().await {
                Ok(context) => context,
                Err(err) => {
                    let build_error = UnifiedRuntimeBuilderError::Bootstrap(
                        UnifiedRuntimeBootstrapError::PreSpawnHook(err.to_string()),
                    );
                    runtime.shutdown().await;
                    return Err(build_error);
                }
            }
        } else {
            serde_json::Value::Null
        };

        // Classic-path capability wiring (M4 de-weld): the §10.1 posture
        // write gate and the §9.3 timeline sink install on any provider
        // advertising the firewall controls (only when the embedder did not
        // install a taint-tracking gate already), and the console Memory
        // panel (§9.3) registers for any provider advertising the panel
        // read API. Recall-only providers keep injection + recorder
        // without a panel — by their capability flags, not by type.
        if let Some(provider) = classic_agent_memory.as_ref() {
            if let Some(taintable) = provider.as_taintable() {
                let llm_writes = self
                    .agent_memory_config
                    .as_ref()
                    .map(|config| config.llm_writes)
                    .unwrap_or_default();
                taintable.set_llm_write_gate_if_absent(Arc::new(
                    crate::memory::taint::TaintLlmWriteGate::new(None, llm_writes),
                ));
                taintable.set_event_sink_if_absent(runtime.memory_event_sink());
            }
            if let Some(panel) = provider.as_memory_panel_store() {
                runtime.set_memory_panel_store(panel);
            }
        }

        // Full-stack path (persistent_agent_memory_stack): firewall + engines
        // + observer over the pre-opened stack provider.
        if let (Some(provider), Some(engines)) =
            (stack_provider, self.agent_memory_engines.as_ref())
        {
            let config = self.agent_memory_config.clone().unwrap_or_default();
            // Engine factory state and the HNSW discard source live under
            // the realm's path root — the state directory, or the scratch
            // root on the remote-authoritative (scratch_dir) shape, or a host
            // store realm's per-host cache.
            let persistent_state = self
                .persistent_state_path
                .clone()
                .or_else(|| self.scratch_dir.clone())
                .or_else(|| self.host_store.as_ref().map(|host| host.cache_root.clone()));
            let transcript_store: Option<Arc<dyn meerkat::SessionStore>> =
                if engines.distiller.enabled || engines.steward.enabled {
                    // The session store the runtime persists through: the
                    // engines must read the transcripts the runtime actually
                    // keeps, never a parallel copy.
                    Some(composed_session_store.clone().ok_or_else(|| {
                        UnifiedRuntimeBuilderError::ConflictingConfiguration(
                            "agent memory engines require a durable session store: \
                             persistent_state() or a storage_provider() realm"
                                .to_string(),
                        )
                    })?)
                } else {
                    None
                };
            // The engines resolve provider credentials in the runtime's
            // store, as its agents do.
            let stack = crate::memory_wiring::attach_memory_engines_with_provider_auth(
                provider,
                &config,
                engines,
                crate::memory_wiring::MemoryStackSeams {
                    persistent_state,
                    transcript_store,
                    event_sink: Some(runtime.memory_event_sink()),
                    ..Default::default()
                },
                self.provider_auth_persistence.clone(),
            )
            .map_err(UnifiedRuntimeBuilderError::Io)?;
            if let Some(panel) = stack.panel.clone() {
                runtime.set_memory_panel_store(panel);
            }
            // §10.1 dispatch-time taint join: bind the stack's tracker into
            // the member pre-build seam so every member's LLM client marks
            // untrusted ingestion synchronously - ahead of the async
            // observer spawned below, closing the first-ingestion race.
            dispatch_taint_slot.fill(stack.taint.clone());
            // Runtime ownership makes both infinite supervisors visible to
            // normal shutdown and to the identity-bootstrap failure cleanup
            // path below. Leaking either handle here would leave ghost memory
            // work behind after build() returned Err.
            *runtime.agent_memory_observer_task.lock().await = Some(
                crate::spawn_member_event_observer(runtime.mob_runtime.handle(), stack.sinks),
            );
            if let Some(steward) = stack.steward.as_ref() {
                // Library mode has no schedule host; the guarded interval
                // loop drives dreams.
                *runtime.agent_memory_steward_task.lock().await = Some(steward.spawn_dream_loop());
            }
            tracing::info!(
                distiller = stack.distiller.is_some(),
                steward = stack.steward.is_some(),
                "agent memory stack installed (builder path)"
            );
        }

        // All fallible builder-only setup is complete. Install the circular
        // MobRuntime <-> IdentityRuntime authority only now: earlier
        // memory-stack errors can return by ordinary drop without retaining
        // the runtime and its persistent controller locks. (Ordinary drop
        // does NOT retire the live supervisor in-proc route under meerkat
        // 0.8.23; the pre-spawn hook path above already shuts down
        // cooperatively for that reason, and the memory-stack `?` returns
        // between runtime construction and this point share that latent
        // route leak for same-process same-id rebuilds.)
        if let (Some(context), Some(roster_specs)) = (
            runtime.identity_first_context.clone(),
            identity_roster_specs.as_ref(),
        ) {
            // The shared installer registers continuity owners before lifting
            // a persistent stopped mob and owns bootstrap failure cleanup.
            if let Err(err) = runtime
                .install_and_bootstrap_identity_first_context(context, roster_specs)
                .await
            {
                let build_error = UnifiedRuntimeBuilderError::Bootstrap(
                    UnifiedRuntimeBootstrapError::IdentityFirst(format!(
                        "identity bootstrap failed: {err}"
                    )),
                );
                return Err(build_error);
            }
        } else {
            runtime
                .activate_without_identity_context()
                .await
                .map_err(|error| {
                    UnifiedRuntimeBuilderError::Bootstrap(UnifiedRuntimeBootstrapError::Mob(error))
                })?;
        }

        // Mob announcements attach once the identity runtime is installed,
        // before discovery and before the control listener seals its keyed
        // inject plane: the order every gateway composes them in. A failure
        // follows the cooperative shutdown path of the other late errors.
        if let Some(composition) = announcements {
            // A host store realm first: its layout is the disposable cache's,
            // which holds no authority.
            let composed = match (self.host_store.as_ref(), storage_layout.as_ref()) {
                (Some(host), _) => composition
                    .compose_over_host_store(&runtime, host.realm.realm())
                    .await
                    .map(drop),
                (None, Some(layout)) => composition.compose(&runtime, layout).map(drop),
                (None, None) => Err(
                    "mob announcements need persistent_state() or a host store realm".to_string(),
                ),
            };
            if let Err(error) = composed {
                let build_error =
                    UnifiedRuntimeBuilderError::Io(format!("mob_announcements(): {error}"));
                runtime.shutdown().await;
                return Err(build_error);
            }
        }

        if runtime.identity_first_context.is_none()
            && let Some(ref discovery) = runtime.discovery
        {
            let specs = discovery.discover(pre_spawn_context).await;
            let spawn_specs: Vec<SpawnMemberSpec> =
                specs.iter().map(discovery_spec_to_spawn_spec).collect();
            if let Err(err) = runtime.spawn_many(spawn_specs).await {
                let build_error =
                    UnifiedRuntimeBuilderError::Bootstrap(UnifiedRuntimeBootstrapError::Mob(err));
                // The full memory stack may already own infinite observer and
                // steward supervisors. Dropping their JoinHandles detaches
                // them, so failed classic discovery follows normal shutdown.
                runtime.shutdown().await;
                return Err(build_error);
            }
        }

        // Run initial edge reconciliation after spawn completes
        if runtime.edge_discovery.is_some()
            || runtime.topology_controller.revision().await > 0
            || runtime.topology_controller.has_pending().await
        {
            let report = runtime.reconcile_edges().await;
            *runtime.bootstrap_edges_report.write().await = Some(report);
        }

        // Bind the cross-mob control listener last among the fallible
        // steps: the fully assembled runtime owns the serve task, so a bind
        // failure follows the same cooperative shutdown path as the other
        // late bootstrap errors above.
        if let Some(addr) = control_listen.as_ref() {
            let authorizer = std::sync::Arc::new(
                crate::runtime::cross_mob_control::ControlAuthorizer::with_grants_for_audience(
                    self.control_grants.take().unwrap_or_default(),
                    runtime.mob_id(),
                ),
            );
            if let Err(error) = runtime
                .start_control_listener_with_authorizer(addr, authorizer)
                .await
            {
                let build_error =
                    UnifiedRuntimeBuilderError::Io(format!("control_listen(): {error}"));
                runtime.shutdown().await;
                return Err(build_error);
            }
            // The runtime is composed: seal what the listener's keyed inject
            // plane serves, through the composition every surface shares.
            if let Err(error) = crate::mob_announcements::compose_keyed_inject_plane(
                &runtime,
                self.keyed_inject.unwrap_or_default(),
            ) {
                let build_error =
                    UnifiedRuntimeBuilderError::Io(format!("control_listen(): {error}"));
                runtime.shutdown().await;
                return Err(build_error);
            }
        }

        // Start event log ingestion if configured
        let mut runtime = runtime;
        if let Some(event_log_config) = self.event_log_config {
            runtime.start_event_log(event_log_config);
        }

        if let Some((sink, timings)) = self.continuity_mirror
            && let Err(error) = runtime.install_continuity_mirror(sink, timings).await
        {
            let build_error = UnifiedRuntimeBuilderError::ConflictingConfiguration(format!(
                "continuity_mirror(): {error}"
            ));
            runtime.shutdown().await;
            return Err(build_error);
        }

        // A host store realm's readiness names the members Core restored
        // broken at this start.
        if let Some(host) = self.host_store.as_ref() {
            let broken = crate::mob_handle_runtime::broken_members_of(&runtime.mob_handle())
                .await
                .into_iter()
                .map(|(member, _reason)| {
                    crate::member_comms_id::runtime_alias_str(member.as_str()).into_owned()
                })
                .collect();
            host.realm.record_broken_members(broken);
        }

        Ok(runtime)
    }

    /// The `meerkat::Config` builder-created session services are built with:
    /// the caller's [`meerkat_config`](Self::meerkat_config) with the
    /// host-level [`compaction`](Self::compaction) declaration composed over
    /// its compaction slot.
    ///
    /// Returns `None` when neither is configured, so the downstream
    /// `MobBootstrapSpec` constructors keep their existing
    /// `agent_config.unwrap_or_default()` behavior for un-configured hosts.
    #[allow(
        clippy::result_large_err,
        reason = "UnifiedRuntimeBuilderError is the published builder error; boxing it would change that public type"
    )]
    fn effective_meerkat_config(
        &self,
    ) -> Result<Option<meerkat::Config>, UnifiedRuntimeBuilderError> {
        let Some(policy) = self.compaction_policy.as_ref() else {
            return Ok(self.meerkat_config.clone());
        };
        let mut config = self.meerkat_config.clone().unwrap_or_default();
        let applied = crate::compaction_policy::apply_compaction_policy(&mut config, policy);
        if let Err(error) = applied {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                format!("compaction(): {error}"),
            ));
        }
        Ok(Some(config))
    }

    /// Resolve the caller's blob injection into its typed form. `build()`
    /// rejects supplying both forms up front; this keeps direct
    /// `resolve_mob_spec` users honest too.
    #[allow(
        clippy::result_large_err,
        reason = "UnifiedRuntimeBuilderError is the published builder error; boxing it would change that public type"
    )]
    fn blob_injection(
        &self,
    ) -> Result<Option<crate::blob_store::BlobStoreInjection>, UnifiedRuntimeBuilderError> {
        match (self.blob_store.clone(), self.binary_blob_store.clone()) {
            (Some(_), Some(_)) => Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "blob_store() and binary_blob_store() are mutually exclusive — inject the one \
                 typed form you have"
                    .to_string(),
            )),
            (Some(core), None) => Ok(Some(crate::blob_store::BlobStoreInjection::Core(core))),
            (None, Some(binary)) => Ok(Some(crate::blob_store::BlobStoreInjection::Binary(binary))),
            (None, None) => Ok(None),
        }
    }

    /// Validate retained identity authority before opening optional document
    /// storage or constructing a dispatcher. With no factory, leave every
    /// extension capability inactive.
    #[cfg(feature = "extension-state")]
    async fn materialize_extension_factories(
        &mut self,
        spec: &mut MobBootstrapSpec,
    ) -> Result<
        Option<Arc<crate::extension_state::NativeAuthorityRegistry>>,
        UnifiedRuntimeBuilderError,
    > {
        use crate::extension_state::{
            NativeAuthorityRegistry, NativeCallerResolver, ToolBundleContext,
        };
        use crate::storage_provider::MetaStorageProvider;
        if self.tool_bundle_factories.is_empty() && self.mcp_document_bindings.is_empty() {
            return Ok(None);
        }
        let state_dir = self.persistent_state_path.clone().ok_or_else(|| {
            UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "durable extension documents require persistent_state()".into(),
            )
        })?;
        let realm = extension_realm(spec, &state_dir)?;
        let opened = match self.extension_store.take() {
            Some(opened) => opened,
            None => {
                crate::storage_provider::DiskMetaStorageProvider
                    .open_extension_state(
                        &crate::storage_provider::MetaRealmOpenContext::for_state_dir(state_dir),
                    )
                    .await?
            }
        };
        if opened.durability.domain != "extension-state"
            || opened.durability.class != meerkat_core::DurabilityClass::Durable
            || opened.durability.resolution != meerkat_core::DurabilityResolution::Persistent
        {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "extension provider must declare persistent extension-state storage".into(),
            ));
        }
        self.provider_slot_census
            .push(crate::storage_health::StorageSlotSummary {
                durability: crate::storage_health::StorageSlotDurability::Declared(
                    opened.durability,
                ),
                backend: "configured extension-state provider".into(),
                detail: None,
                degraded: false,
            });
        let registry = NativeAuthorityRegistry::new(
            realm.as_str().to_string(),
            spec.definition.id.to_string(),
            Arc::clone(&spec.session_service),
            // Identity-first compositions are exactly the ones with a
            // roster (build() refuses one without it).
            self.roster_provider.is_some(),
            self.access_controller.clone(),
            Arc::clone(&opened.store),
        );
        // Every document service writes through the registry's ordering of
        // mob id changes.
        let document_store = registry.document_store();
        let mut namespaces = std::collections::BTreeSet::new();
        let mut document_tools = std::collections::BTreeSet::new();
        for (name, (requirements, factory)) in std::mem::take(&mut self.tool_bundle_factories) {
            if spec.tool_bundles.contains_key(&name)
                || self.tool_bundles.contains_key(&name)
                || !namespaces.insert(requirements.namespace.clone())
                || !document_tools.insert(requirements.read_tool.clone())
                || !document_tools.insert(requirements.edit_tool.clone())
            {
                return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                    format!("extension bundle or namespace registered more than once: {name}"),
                ));
            }
            let documents = meta_extension_state::DocumentService::new(
                realm.as_str(),
                requirements.namespace.clone(),
                Arc::clone(&document_store),
            )
            .map_err(|error| {
                UnifiedRuntimeBuilderError::ConflictingConfiguration(error.to_string())
            })?;
            let resolver = Arc::new(NativeCallerResolver {
                registry: Arc::downgrade(&registry),
                requirements: requirements.clone(),
                child_availability: None,
            });
            let dispatcher = factory
                .build(ToolBundleContext {
                    documents,
                    caller_resolver: resolver,
                })
                .map_err(UnifiedRuntimeBuilderError::ConflictingConfiguration)?;
            crate::extension_state::mcp_binding::validate_document_dispatcher(
                dispatcher.as_ref(),
                &requirements,
            )
            .map_err(UnifiedRuntimeBuilderError::ConflictingConfiguration)?;
            spec.child_tool_bundle_availability.insert(
                name.clone(),
                meerkat_mob_mcp::ChildToolBundleAvailability::ChildAvailable,
            );
            spec.tool_bundles.insert(name, dispatcher);
        }
        let mut child_mcp_servers = meerkat_mob_mcp::ChildMcpServers::new();
        let mut server_names = std::collections::BTreeSet::new();
        for binding in &self.mcp_document_bindings {
            let requirements = &binding.requirements;
            if !server_names.insert(binding.public_config().name.clone())
                || !namespaces.insert(requirements.namespace.clone())
                || !document_tools.insert(requirements.read_tool.clone())
                || !document_tools.insert(requirements.edit_tool.clone())
                || spec
                    .tool_bundles
                    .values()
                    .chain(self.tool_bundles.values().map(|(dispatcher, _)| dispatcher))
                    .any(|dispatcher| {
                        dispatcher.tools().iter().any(|tool| {
                            tool.name == requirements.read_tool
                                || tool.name == requirements.edit_tool
                        })
                    })
            {
                return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                    "document MCP server, namespace or tool registered more than once".into(),
                ));
            }
            let documents = meta_extension_state::DocumentService::new(
                realm.as_str(),
                requirements.namespace.clone(),
                Arc::clone(&document_store),
            )
            .map_err(|error| {
                UnifiedRuntimeBuilderError::ConflictingConfiguration(error.to_string())
            })?;
            let resolver = Arc::new(NativeCallerResolver {
                registry: Arc::downgrade(&registry),
                requirements: requirements.clone(),
                child_availability: Some(binding.child_availability),
            });
            let dispatcher = binding
                .factory
                .build(ToolBundleContext {
                    documents,
                    caller_resolver: resolver,
                })
                .map_err(UnifiedRuntimeBuilderError::ConflictingConfiguration)?;
            binding
                .validate_dispatcher(dispatcher)
                .map_err(UnifiedRuntimeBuilderError::ConflictingConfiguration)?;
            child_mcp_servers = child_mcp_servers
                .register(binding.public_config().clone(), binding.child_availability)
                .map_err(|error| {
                    UnifiedRuntimeBuilderError::ConflictingConfiguration(error.to_string())
                })?;
        }
        if !self.mcp_document_bindings.is_empty() {
            spec.child_mcp_servers = Some(child_mcp_servers);
        }
        // Bootstrap installs these on the final agent mob-tool state, after
        // composing its child application policy and native tool bundles.
        spec.before_activation = Some(registry.before_activation());
        spec.after_destroy = Some(registry.after_destroy());
        spec.before_create = Some(registry.before_create());
        Ok(Some(registry))
    }

    /// Open the composite [`crate::storage_provider::MetaStorageProvider`]
    /// realm (when one is installed) and materialize its slots into the
    /// per-slot builder seams, so the rest of `build()` composes through one
    /// code path. Conflicts between the provider and explicit per-slot seams
    /// are typed errors; the provider's set was validated fail-closed at
    /// `open_realm`.
    async fn materialize_storage_provider(&mut self) -> Result<(), UnifiedRuntimeBuilderError> {
        let Some(provider) = self.storage_provider.take() else {
            return Ok(());
        };
        if let Some(realm) = provider.host_store_realm().cloned() {
            return self.materialize_host_store_realm(provider, realm).await;
        }
        let conflicting = self.subsumed_seams();
        if !conflicting.is_empty() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                format!(
                    "storage_provider() supplies the realm store set; the per-slot seams it \
                 subsumes cannot also be set: {}",
                    conflicting.join(", ")
                ),
            ));
        }
        if self.roster_provider.is_none() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "storage_provider() requires roster_provider(): a storage provider composes an \
                 identity-first runtime, whose identities' continuity is Core's mob identity \
                 authority in the mob storage"
                    .to_string(),
            ));
        }
        let (state_dir, layout) = if let Some(state_path) = self.persistent_state_path.clone() {
            (
                state_path.clone(),
                MetaStorageLayout::with_injected_roots(state_path, None),
            )
        } else if let Some(scratch) = self.scratch_dir.clone() {
            (
                scratch.clone(),
                MetaStorageLayout::declared_ephemeral(scratch),
            )
        } else {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "storage_provider() requires persistent_state() or scratch_dir() as the \
                 realm's path root"
                    .to_string(),
            ));
        };
        let mut declared_ephemeral_domains = Vec::new();
        if self.ephemeral_blobs {
            declared_ephemeral_domains.push("blobs".to_string());
        }
        if self.ephemeral_runtime_store {
            declared_ephemeral_domains.push("runtime".to_string());
        }
        // The provider opens its realm in the state directory: claim it first
        // (see `build`). A non-disk provider's meerkat-level bundle holds the
        // runtime store (see below), so no `runtime.sqlite` sits here for
        // the bindings check.
        if let Some(state_path) = self.persistent_state_path.clone() {
            let runtime_store = if provider.meerkat_provider().name() == "disk" {
                crate::native_startup::RuntimeStoreLocation::StateDir
            } else {
                crate::native_startup::RuntimeStoreLocation::StorageProvider
            };
            self.claim_state_dir(&state_path, runtime_store).await?;
        }
        let ctx = crate::storage_provider::MetaRealmOpenContext {
            layout,
            state_dir,
            declared_ephemeral_domains,
            meerkat_realm_declaration: self.meerkat_realm_declaration,
        };
        let set = provider.open_realm(&ctx).await?;
        // Providers are contracted to enforce this before returning; the
        // composition re-checks because the rule is the composition's.
        crate::storage_provider::enforce_fail_closed_store_set(&set, &ctx)?;
        #[cfg(feature = "extension-state")]
        if !self.tool_bundle_factories.is_empty() || !self.mcp_document_bindings.is_empty() {
            self.extension_store = Some(provider.open_extension_state(&ctx).await?);
        }

        // M4b: the single-bundle contract covers the meerkat-shared level
        // too. The built-in disk backend IS the local composition the spec
        // opens (M2 layout compatibility — runtime.sqlite/workgraph.sqlite3
        // stay flat under the state dir), so only a non-disk backend routes
        // the meerkat-level slots through the provider's realm bundle;
        // runtime and workgraph authority then land in the same backend as
        // continuity, fail-closed at the seam instead of a silent local
        // split.
        let meerkat_provider = provider.meerkat_provider();
        self.provider_meerkat_stores = if meerkat_provider.name() == "disk" {
            None
        } else {
            Some(
                crate::storage_provider::open_provider_meerkat_stores(meerkat_provider, &ctx)
                    .await?,
            )
        };
        // The bindings check the state directory claim could not run: the
        // runtime store lives in the provider's backend, so read it through
        // the store, before any identity wakes onto an empty transcript.
        if let Some(stores) = self.provider_meerkat_stores.as_ref() {
            let mob_store = self.durable_mob_store();
            crate::native_startup::refuse_lost_runtime_store(
                &ctx.state_dir,
                mob_store.as_deref(),
                stores.runtime_store.as_ref(),
                &stores.provider_name,
            )
            .await?;
        }
        // Retain the provider's per-slot durability declarations for the
        // census: the health surfaces must report the provider-declared
        // resolutions verbatim, not the local defaults those declarations
        // replaced (an explicitly-ephemeral schedule must not read as
        // persistent).
        self.provider_slot_census = set
            .durability
            .iter()
            .map(|declaration| crate::storage_health::StorageSlotSummary {
                durability: crate::storage_health::StorageSlotDurability::Declared(
                    declaration.clone(),
                ),
                backend: format!("storage provider '{}'", provider.name()),
                detail: Some(
                    "provider-declared durability (composite realm store set)".to_string(),
                ),
                degraded: false,
            })
            .collect();
        // A non-disk backend over a scratch directory keeps its sessions in
        // the backend, but its mob storage in memory unless mob_storage()
        // supplies one: after a restart the sessions remain and the
        // identities' bindings do not, so every identity starts fresh. Said
        // in the census and the log; a host store realm keeps both.
        if self.provider_meerkat_stores.is_some()
            && self.persistent_state_path.is_none()
            && self.mob_storage.is_none()
        {
            tracing::warn!(
                provider = %provider.name(),
                "the storage provider keeps the sessions, but this build keeps its mob storage \
                 in memory: after a restart every identity starts on a fresh session while its \
                 old one remains. Keep the realm in a host store (HostStoreMetaStorageProvider) \
                 or supply a durable mob_storage()"
            );
            self.provider_slot_census
                .push(crate::storage_health::StorageSlotSummary {
                    durability: crate::storage_health::StorageSlotDurability::Declared(
                        meerkat_core::DurabilityDeclaration::durable(
                            "mob_storage",
                            meerkat_core::DurabilityResolution::NonPersistent,
                        ),
                    ),
                    backend: "in-memory mob storage".to_string(),
                    detail: Some(
                        "warning: the identities' bindings do not survive a restart while the \
                         provider keeps their sessions; keep the realm in a host store \
                         (HostStoreMetaStorageProvider) or supply a durable mob_storage()"
                            .to_string(),
                    ),
                    degraded: true,
                });
        }

        self.install_realm_store_set(set);
        Ok(())
    }

    /// The per-slot seams a storage provider's realm store set subsumes,
    /// among the ones this builder was given.
    fn subsumed_seams(&self) -> Vec<&'static str> {
        [
            // A caller-supplied mob spec routes composition through the
            // legacy spec path, silently bypassing the provider's meerkat
            // bundle and the session-authority rewiring - both P1 classes
            // this seam exists to prevent.
            ("mob_spec()", self.mob_spec.is_some()),
            ("continuity_store()", self.continuity_store.is_some()),
            ("lease_provider()", self.lease_provider.is_some()),
            ("blob_store()", self.blob_store.is_some()),
            ("binary_blob_store()", self.binary_blob_store.is_some()),
            ("schedule_store()", self.schedule_store.is_some()),
            // The workgraph is a meerkat-level slot, so it is NOT part of the
            // meta `RealmStoreSet` this seam materializes - the provider's
            // workgraph rides `provider_meerkat_stores`. It still conflicts:
            // two independent channels would otherwise both claim the slot,
            // and per-slot injection silently winning is exactly the
            // ambiguity the typed error exists to prevent.
            ("workgraph_store()", self.workgraph_store.is_some()),
            ("with_console_log_store()", self.console_log_store.is_some()),
            ("persistent_metadata()", self.persistent_metadata.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, set)| set.then_some(name))
        .collect()
    }

    /// Install a provider's realm store set into the per-slot seams the rest
    /// of `build()` composes through.
    fn install_realm_store_set(&mut self, set: crate::storage_provider::MetaRealmStoreSet) {
        self.continuity_store = Some(set.continuity_store);
        self.lease_provider = Some(match set.lease_authority {
            crate::storage_provider::MetaLeaseAuthority::Provider(provider) => provider,
            crate::storage_provider::MetaLeaseAuthority::FencingFloor(floor) => {
                Arc::new(LocalLeaseProvider::with_floor(floor))
            }
        });
        self.binary_blob_store = Some(set.blob_store);
        self.schedule_store = Some(set.schedule_store);
        self.console_log_store = Some(set.console_log_store);
        self.persistent_metadata = Some(set.metadata_store);
        if self.event_log_config.is_none()
            && let Some(store) = set.event_log_store
        {
            self.event_log_config = Some(EventLogConfig {
                store,
                ..EventLogConfig::default()
            });
        }
        if self.agent_memory_provider.is_none()
            && (self.agent_memory_config.is_some() || self.agent_memory_engines.is_some())
            && let Some(memory) = set.agent_memory_provider
        {
            self.agent_memory_provider = Some(memory);
        }
    }

    /// [`Self::materialize_storage_provider`] for a provider that keeps the
    /// realm in a host store ([`crate::host_store`]). The realm is the
    /// authority, so nothing local is claimed, pinned or probed: one realm
    /// listing holds the sessions and the bindings together, and the local
    /// directory is a per-host cache (`scratch_dir()` when set, else one the
    /// runtime owns).
    async fn materialize_host_store_realm(
        &mut self,
        provider: Arc<dyn crate::storage_provider::MetaStorageProvider>,
        realm: crate::host_store::HostStoreRealm,
    ) -> Result<(), UnifiedRuntimeBuilderError> {
        let mut conflicting: Vec<&str> = [
            ("persistent_state()", self.persistent_state_path.is_some()),
            ("mob_storage()", self.mob_storage.is_some()),
            (
                "continuity_from_state_dir()",
                self.continuity_state_dir.is_some(),
            ),
        ]
        .into_iter()
        .filter_map(|(name, set)| set.then_some(name))
        .collect();
        // continuity_from_state_dir() fills the continuity pair itself.
        conflicting.extend(self.subsumed_seams().into_iter().filter(|seam| {
            self.continuity_state_dir.is_none()
                || !matches!(*seam, "continuity_store()" | "lease_provider()")
        }));
        if !conflicting.is_empty() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                format!(
                    "storage_provider() keeps this realm in a host store, which holds its \
                     sessions, mob storage, identities and Meta's records; these settings would \
                     put part of that elsewhere: {}",
                    conflicting.join(", ")
                ),
            ));
        }
        if self.roster_provider.is_none() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "storage_provider() requires roster_provider(): a host store realm keeps an \
                 identity-first runtime, whose identities' continuity is Core's mob identity \
                 authority in the realm"
                    .to_string(),
            ));
        }
        // One event log: the builder's, or the store the host supplied with
        // the realm, never one silently dropped for the other.
        if self.event_log_config.is_some() && realm.supplies_event_log() {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "event_log() and the host store realm's event log store \
                 (HostStoreMetaStores::event_log_store) both supply the event log: leave \
                 event_log_store None to keep event_log()'s config, store and settings, or \
                 drop event_log() to ingest into event_log_store with the default settings"
                    .to_string(),
            ));
        }
        let (cache_root, cache_dir) = match self.scratch_dir.clone() {
            Some(scratch) => {
                std::fs::create_dir_all(&scratch).map_err(|error| {
                    UnifiedRuntimeBuilderError::Io(format!(
                        "failed to create scratch directory at {}: {error}",
                        scratch.display()
                    ))
                })?;
                (scratch, None)
            }
            None => {
                let dir = tempfile::tempdir().map_err(|error| {
                    UnifiedRuntimeBuilderError::Io(format!(
                        "failed to create the host store realm's cache directory: {error}"
                    ))
                })?;
                (dir.path().to_path_buf(), Some(Arc::new(dir)))
            }
        };
        let mut declared_ephemeral_domains = Vec::new();
        if self.ephemeral_blobs {
            declared_ephemeral_domains.push("blobs".to_string());
        }
        let ctx = crate::storage_provider::MetaRealmOpenContext {
            layout: MetaStorageLayout::declared_ephemeral(cache_root.clone()),
            state_dir: cache_root.clone(),
            declared_ephemeral_domains,
            meerkat_realm_declaration: self.meerkat_realm_declaration,
        };
        let set = provider.open_realm(&ctx).await?;
        crate::storage_provider::enforce_fail_closed_store_set(&set, &ctx)?;
        #[cfg(feature = "extension-state")]
        if !self.tool_bundle_factories.is_empty() || !self.mcp_document_bindings.is_empty() {
            self.extension_store = Some(provider.open_extension_state(&ctx).await?);
        }
        let realm_id = realm
            .meerkat_level_realm(self.meerkat_realm_declaration)
            .await?;
        self.provider_meerkat_stores = Some(realm.provider_meerkat_stores(realm_id.clone()));
        self.provider_slot_census = realm.census_slots(&set.durability, &cache_root);
        if self.mob_announcements.is_some() {
            self.provider_slot_census
                .push(realm.mob_announcements_census_slot());
        }
        self.install_realm_store_set(set);
        self.host_store = Some(HostStoreComposition {
            realm,
            realm_id,
            cache_root,
            cache_dir,
        });
        Ok(())
    }

    /// Resolve the mob spec from the definition-based path.
    /// Called only when `mob_spec` is not set (legacy path handled in `build()`).
    #[allow(
        unknown_lints,
        clippy::unused_async_trait_impl,
        reason = "preserve the awaitable builder resolution seam used throughout the build pipeline"
    )]
    /// Turn the registered live doors into a plan, taking the typed inputs
    /// the spec retained. Fails closed when a door is registered but the
    /// session service is not persistent: live channels need the durable
    /// session owner, and silently composing nothing would leave the console
    /// reporting voice unavailable with no diagnostic.
    #[allow(
        clippy::result_large_err,
        reason = "UnifiedRuntimeBuilderError is the published builder error; boxing it would change that public type"
    )]
    fn live_plan(
        &self,
        mob_spec: &mut MobBootstrapSpec,
    ) -> Result<Option<super::live_compose::LivePlan>, UnifiedRuntimeBuilderError> {
        #[cfg(feature = "openai-live")]
        let requested = self.live.is_some() || self.console_voice.is_some();
        #[cfg(not(feature = "openai-live"))]
        let requested = self.live.is_some();
        if !requested {
            return Ok(None);
        }
        if self
            .live
            .as_ref()
            .is_some_and(|options| options.public_base_url.trim().is_empty())
        {
            return Err(UnifiedRuntimeBuilderError::LiveCompose(
                super::live_compose::LiveComposeError::LiveRequiresPublicBaseUrl,
            ));
        }
        let inputs =
            mob_spec
                .live_compose_inputs
                .take()
                .ok_or(UnifiedRuntimeBuilderError::LiveCompose(
                    super::live_compose::LiveComposeError::LiveRequiresPersistentSessions,
                ))?;
        Ok(Some(super::live_compose::LivePlan {
            inputs,
            live: self.live.clone(),
            #[cfg(feature = "openai-live")]
            console_voice: self.console_voice.clone(),
            #[cfg(all(test, feature = "openai-live"))]
            summary_override: None,
            #[cfg(test)]
            session_factory_override: None,
        }))
    }

    /// The supplied grant configuration, taken by the one spec construction
    /// that builds the runtime machine.
    fn take_grant_configuration(
        &self,
    ) -> Option<meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration> {
        self.grant_configuration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    async fn resolve_mob_spec(&self) -> Result<MobBootstrapSpec, UnifiedRuntimeBuilderError> {
        let mut caps = self.capability_flags;
        // Resolved once for all three storage arms: the agent config every
        // builder-created session service (and therefore every compactor)
        // is built from.
        let agent_config = self.effective_meerkat_config()?;
        let definition = match self.definition_source {
            Some(DefinitionSource::Inline(ref def)) => *def.clone(),
            Some(DefinitionSource::TomlPath(ref path)) => {
                let toml_content = std::fs::read_to_string(path).map_err(|e| {
                    UnifiedRuntimeBuilderError::Io(format!(
                        "failed to read definition TOML at {}: {e}",
                        path.display()
                    ))
                })?;
                MobDefinition::from_toml(&toml_content).map_err(|e| {
                    UnifiedRuntimeBuilderError::DefinitionLoad(format!(
                        "failed to parse definition TOML at {}: {}",
                        path.display(),
                        crate::gateway_composition::describe_mob_definition_error(&e)
                    ))
                })?
            }
            None => {
                return Err(UnifiedRuntimeBuilderError::MissingRequiredField(
                    UnifiedRuntimeBuilderField::MobSpec,
                ));
            }
        };
        caps.image_generation |=
            crate::mob_handle_runtime::mob_definition_may_use_image_generation(&definition);
        // Declared-versus-resolved capability invariant. This is the ONLY
        // sound place for it: on the definition path `self.schedule_store`
        let max_sessions = self.max_sessions.unwrap_or(DEFAULT_MAX_SESSIONS);
        if max_sessions == 0 {
            return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
                "max_sessions() must be greater than 0".to_string(),
            ));
        }

        let hook = self
            .session_hook
            .as_ref()
            .map(|h| -> crate::mob_handle_runtime::PreBuildHook {
                let hook = h.clone();
                Arc::new(
                    move |req: &mut meerkat_core::service::CreateSessionRequest| {
                        let hook = hook.clone();
                        Box::pin(async move { hook.before_create(req).await })
                    },
                )
            });

        let after_hook: Option<crate::mob_handle_runtime::AfterCreateHook> = self
            .session_hook
            .as_ref()
            .map(|h| -> crate::mob_handle_runtime::AfterCreateHook {
                let hook = h.clone();
                Arc::new(move |session_id, ctx| {
                    let hook = hook.clone();
                    Box::pin(async move {
                        hook.after_create(&session_id, &ctx).await;
                    })
                })
            });

        // The session service calls this after the user hook and before its
        // capability invariant, on every create/resume variant. Only the
        // process-local launch vector is resolved; profiles remain public.
        #[cfg(feature = "extension-state")]
        let hook = Some(crate::extension_state::mcp_binding::pre_build_hook(
            hook,
            self.mcp_document_bindings.clone(),
        ));

        let (mob_storage, mob_storage_provenance) = match (
            self.host_store.as_ref(),
            self.mob_storage.clone(),
            self.persistent_state_path.as_ref(),
        ) {
            // A host store realm owns the slot: the realm's one root mob
            // storage, opened once a new realm has its record.
            (Some(host), _, _) => {
                host.realm.ensure_realm_record(&host.realm_id).await?;
                crate::mob_composition_manifest::host_store_mob_storage(&host.realm)
                    .await
                    .map_err(|error| {
                        UnifiedRuntimeBuilderError::Io(format!(
                            "failed to open the host store realm's mob storage: {error}"
                        ))
                    })?
            }
            (None, Some(supplied), _) => supplied,
            // A persistent build keeps its mob storage durable beside the
            // sessions by default.
            (None, None, Some(state_path)) => {
                crate::mob_composition_manifest::persistent_mob_storage(
                    MetaStorageLayout::with_injected_roots(state_path.clone(), None).event_log_db(),
                )
                .map_err(|error| {
                    UnifiedRuntimeBuilderError::Io(format!(
                        "failed to open the persistent mob storage under {}: {error}",
                        state_path.display()
                    ))
                })?
            }
            (None, None, None) => (MobStorage::in_memory(), MobStorageProvenance::default()),
        };

        // Note: blocking I/O (fs, SQLite) — acceptable at startup.
        // The persistent composition: over the state directory, or over a
        // host store realm, whose per-host cache it is built over.
        let persistent = if let Some(host) = self.host_store.as_ref() {
            Some((
                host.cache_root.clone(),
                host.realm_id.clone(),
                self.ephemeral_blobs || host.realm.blobs_declared_ephemeral(),
                crate::mob_handle_runtime::PersistentAuthority::HostStore {
                    cache_dir: host.cache_dir.clone(),
                },
            ))
        } else if let Some(state_path) = self.persistent_state_path.as_ref() {
            std::fs::create_dir_all(state_path).map_err(|e| {
                UnifiedRuntimeBuilderError::Io(format!(
                    "failed to create state directory at {}: {e}",
                    state_path.display()
                ))
            })?;
            let state_layout = MetaStorageLayout::with_injected_roots(state_path.clone(), None);
            Some((
                state_path.clone(),
                state_layout.meerkat_level_realm()?.clone(),
                self.ephemeral_blobs,
                crate::mob_handle_runtime::PersistentAuthority::StateDir,
            ))
        } else {
            None
        };
        let mut spec = if let Some((store_path, realm, ephemeral, authority)) = persistent {
            MobBootstrapSpec::persistent_inner_with_provider_stores(
                definition,
                mob_storage,
                mob_storage_provenance.clone(),
                store_path,
                max_sessions,
                self.blob_injection()?,
                ephemeral,
                self.schedule_store.clone(),
                self.workgraph_store.clone(),
                hook,
                caps,
                after_hook.clone(),
                agent_config,
                self.provider_meerkat_stores.clone(),
                realm,
                self.take_grant_configuration(),
                self.effective_mcp_call_context_provider(),
                self.operation_refusal_observer.clone(),
                self.state_dir_claim.clone(),
                authority,
                self.session_identity_config_root.clone(),
                self.provider_auth_persistence.clone(),
            )
            .await
            .map_err(storage_resolution_builder_error)?
        } else if let Some(ref scratch_dir) = self.scratch_dir {
            std::fs::create_dir_all(scratch_dir).map_err(|e| {
                UnifiedRuntimeBuilderError::Io(format!(
                    "failed to create scratch directory at {}: {e}",
                    scratch_dir.display()
                ))
            })?;

            MobBootstrapSpec::ephemeral_runtime_backed_with_provider_stores(
                definition,
                mob_storage,
                scratch_dir.clone(),
                max_sessions,
                self.blob_injection()?,
                self.schedule_store.clone(),
                self.workgraph_store.clone(),
                hook,
                caps,
                after_hook,
                agent_config,
                self.provider_meerkat_stores.clone(),
                self.take_grant_configuration(),
                self.effective_mcp_call_context_provider(),
                self.operation_refusal_observer.clone(),
                self.session_identity_config_root.clone(),
                self.provider_auth_persistence.clone(),
            )
            .map_err(storage_resolution_builder_error)?
        } else {
            // Ephemeral: create a temp dir that lives as long as the runtime.
            let temp_dir = tempfile::tempdir().map_err(|e| {
                UnifiedRuntimeBuilderError::Io(format!("failed to create temp dir: {e}"))
            })?;
            let store_path = temp_dir.path().to_path_buf();

            let mut spec = MobBootstrapSpec::ephemeral_runtime_backed_with_provider_stores(
                definition,
                mob_storage,
                store_path,
                max_sessions,
                self.blob_injection()?,
                self.schedule_store.clone(),
                self.workgraph_store.clone(),
                hook,
                caps,
                after_hook,
                agent_config,
                None,
                self.take_grant_configuration(),
                self.effective_mcp_call_context_provider(),
                self.operation_refusal_observer.clone(),
                self.session_identity_config_root.clone(),
                self.provider_auth_persistence.clone(),
            )
            .map_err(storage_resolution_builder_error)?;
            spec._ephemeral_dir = Some(Arc::new(temp_dir));
            spec
        };

        spec = spec.with_mob_storage_provenance(mob_storage_provenance);
        // One place for all three compositions above.
        if let Some(policy) = self.fork_instruction_policy {
            spec = spec.with_fork_instruction_policy(policy);
        }
        spec.options = MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: self.default_llm_client.clone(),
        };

        Ok(spec)
    }
}

/// Enforce the documented same-root pairing of `continuity_from_state_dir`
/// and `persistent_state`: both must name ONE physical state directory, or
/// session authority forks from the runtime/blob/workgraph stores it must
/// pair with. The comparison is by canonical (physical) identity, not raw
/// spelling — a lexical compare both refuses a genuinely shared root spelled
/// two ways (symlink vs target, `..`-detour vs plain) and, worse, lets two
/// lexically equal spellings that resolve to different physical directories
/// fork silently. A path that cannot be canonicalized refuses outright with
/// both roots named; this gate never degrades to the lexical compare.
#[allow(
    clippy::result_large_err,
    reason = "UnifiedRuntimeBuilderError is the published builder error; boxing it would change that public type"
)]
fn verify_shared_state_root(
    continuity_dir: &std::path::Path,
    state_path: &std::path::Path,
) -> Result<(), UnifiedRuntimeBuilderError> {
    let canonical_continuity = crate::storage_layout::canonicalize_storage_root(continuity_dir)
        .map_err(|error| {
            UnifiedRuntimeBuilderError::ConflictingConfiguration(format!(
                "cannot canonicalize continuity_from_state_dir root {} to verify it is the \
                 same state directory as persistent_state {}: {error}",
                continuity_dir.display(),
                state_path.display()
            ))
        })?;
    let canonical_state =
        crate::storage_layout::canonicalize_storage_root(state_path).map_err(|error| {
            UnifiedRuntimeBuilderError::ConflictingConfiguration(format!(
                "cannot canonicalize persistent_state root {} to verify it is the same state \
             directory as continuity_from_state_dir root {}: {error}",
                state_path.display(),
                continuity_dir.display()
            ))
        })?;
    if canonical_continuity != canonical_state {
        return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
            format!(
                "continuity_from_state_dir opened {} but persistent_state resolves to {} \
                 (configured as {}); the identity substrate and the runtime stores must \
                 share ONE state directory, or session authority forks from the stores it \
                 must pair with",
                canonical_continuity.display(),
                canonical_state.display(),
                state_path.display()
            ),
        ));
    }
    Ok(())
}

/// Whether two state-directory spellings name one physical directory, by
/// canonical identity. A spelling that cannot be canonicalized (the
/// directory does not exist yet) names no claimed directory.
fn same_directory(left: &std::path::Path, right: &std::path::Path) -> bool {
    match (
        crate::storage_layout::canonicalize_storage_root(left),
        crate::storage_layout::canonicalize_storage_root(right),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// The realm extension authority and documents are keyed by: the installation
/// realm the spec's persistent composition already resolved (the fresh default
/// or an adopted predecessor's recorded realm, which a non-disk provider's
/// meerkat-level stores were opened under too), never a default. Documents
/// live under `persistent_state()`, so the spec must have been composed over
/// that same physical state directory: a supplied `mob_spec()` composed over
/// another directory, or over none, carries no realm of this installation and
/// refuses.
#[cfg(feature = "extension-state")]
#[allow(
    clippy::result_large_err,
    reason = "UnifiedRuntimeBuilderError is the published builder error; boxing it would change that public type"
)]
fn extension_realm(
    spec: &MobBootstrapSpec,
    state_dir: &std::path::Path,
) -> Result<meerkat_core::RealmId, UnifiedRuntimeBuilderError> {
    let Some((composed_dir, realm)) = &spec.composed_meerkat_realm else {
        return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
            "durable extension documents require a mob spec composed over \
             persistent_state(); this spec resolved no installation realm"
                .into(),
        ));
    };
    let canonical = |root: &std::path::Path| {
        crate::storage_layout::canonicalize_storage_root(root).map_err(|error| {
            UnifiedRuntimeBuilderError::ConflictingConfiguration(format!(
                "cannot canonicalize {} to verify the mob spec's state directory: {error}",
                root.display()
            ))
        })
    };
    if canonical(composed_dir)? != canonical(state_dir)? {
        return Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(
            format!(
                "the mob spec's stores were composed over {} but persistent_state() is {}; \
                 extension documents are keyed by the installation realm of the state \
                 directory they live in",
                composed_dir.display(),
                state_dir.display()
            ),
        ));
    }
    Ok(realm.clone())
}

/// The builder-level class of a bootstrap storage refusal.
fn storage_resolution_builder_error(
    error: crate::storage_health::StorageResolutionError,
) -> UnifiedRuntimeBuilderError {
    match error {
        crate::storage_health::StorageResolutionError::Blob(
            crate::storage_health::BlobStoreResolutionError::NonPersistentUndeclared,
        ) => UnifiedRuntimeBuilderError::ConflictingConfiguration(error.to_string()),
        crate::storage_health::StorageResolutionError::Blob(
            crate::storage_health::BlobStoreResolutionError::OpenFailed { .. },
        )
        | crate::storage_health::StorageResolutionError::RuntimeStore(_)
        | crate::storage_health::StorageResolutionError::JobStore(_)
        // An unreadable council store is the same class: a durable slot
        // present on disk that refused at composition time.
        | crate::storage_health::StorageResolutionError::Council(_) => {
            UnifiedRuntimeBuilderError::Io(error.to_string())
        }
        crate::storage_health::StorageResolutionError::Layout(error) => error.into(),
        crate::storage_health::StorageResolutionError::Runtime(error) => {
            UnifiedRuntimeBuilderError::Runtime(error)
        }
        crate::storage_health::StorageResolutionError::Delivery(error) => {
            UnifiedRuntimeBuilderError::Delivery(error)
        }
    }
}

/// An identity-first resume binds to the stored definition its roster was
/// resolved from, and Core refuses it when that definition changed in
/// between. Core's message names only epochs; say what changed relative to
/// the roster.
fn name_a_definition_change_after_the_roster(
    error: UnifiedRuntimeBootstrapError,
) -> UnifiedRuntimeBootstrapError {
    let UnifiedRuntimeBootstrapError::Mob(crate::mob_handle_runtime::MobRuntimeError::Mob(
        changed @ meerkat_mob::MobError::MobDefinitionAuthorityChanged { expected_epoch, .. },
    )) = &error
    else {
        return error;
    };
    let cause = if *expected_epoch == crate::mob_handle_runtime::NO_STORED_DEFINITION_EPOCH {
        "the mob storage gained a definition after this boot resolved its roster from the \
         supplied one"
    } else {
        "the mob storage's definition changed after this boot resolved its roster from it"
    };
    UnifiedRuntimeBootstrapError::IdentityFirst(format!(
        "{cause}, so the mob is not resumed: {changed}"
    ))
}
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::service::{
        CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionService,
    };

    #[tokio::test]
    async fn supplied_mob_storage_keeps_its_owner_and_provenance_in_every_storage_arm() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let (storage, provenance) =
            crate::mob_composition_manifest::persistent_mob_storage(dir.path().join("mob.sqlite3"))
                .expect("persistent mob storage");
        let fork_store = storage
            .forked_participant_store()
            .expect("complete mob storage includes fork custody")
            .clone();

        for arm in ["persistent", "scratch", "temporary"] {
            let definition = MobDefinition::from_toml("[mob]\nid = \"builder-mob-storage\"\n")
                .expect("fixture definition");
            let builder = UnifiedRuntimeBuilder::default()
                .definition(definition)
                .mob_storage(storage.clone(), provenance.clone());
            let builder = match arm {
                "persistent" => builder.persistent_state(dir.path().join("state")),
                "scratch" => builder.scratch_dir(dir.path().join("scratch")),
                _ => builder,
            };
            let spec = builder.resolve_mob_spec().await.expect("spec resolves");
            assert_eq!(spec.mob_storage_provenance, provenance, "{arm}");
            assert!(
                Arc::ptr_eq(
                    spec.storage
                        .forked_participant_store()
                        .expect("the complete supplied storage survives"),
                    &fork_store,
                ),
                "{arm} must retain the supplied storage owner",
            );
        }
    }

    /// A persistent build without `mob_storage()` keeps its mob storage
    /// durable beside the sessions: every resolution names the same
    /// `event_log.sqlite3` under the state directory.
    #[tokio::test]
    async fn persistent_state_without_mob_storage_keeps_mob_storage_durable() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let definition = MobDefinition::from_toml("[mob]\nid = \"builder-mob-default\"\n")
            .expect("fixture definition");
        let builder = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .persistent_state(dir.path());
        let journal =
            MetaStorageLayout::with_injected_roots(dir.path().to_path_buf(), None).event_log_db();
        let first = builder.resolve_mob_spec().await.expect("first spec");
        assert_eq!(
            first.mob_storage_provenance.persistent_path(),
            Some(journal.as_path())
        );
        drop(first);
        assert!(journal.is_file(), "the mob journal is durable");
        let second = builder.resolve_mob_spec().await.expect("second spec");
        assert_eq!(
            second.mob_storage_provenance.persistent_path(),
            Some(journal.as_path()),
            "a restart reopens the same durable mob storage"
        );
    }

    #[tokio::test]
    async fn supplied_erased_spec_refuses_mob_storage_override_before_materialization() {
        let definition = MobDefinition::from_toml("[mob]\nid = \"mob-storage-override\"\n")
            .expect("fixture definition");
        let spec = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .resolve_mob_spec()
            .await
            .expect("empty fixture spec");
        let dir = tempfile::tempdir().expect("temporary directory");
        let state_path = dir.path().join("unopened-state");
        let result = UnifiedRuntimeBuilder::default()
            .mob_spec(spec)
            .mob_storage(
                MobStorage::in_memory(),
                MobStorageProvenance::declared_ephemeral(),
            )
            .persistent_state(&state_path)
            .build()
            .await;
        assert!(matches!(result,
            Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(detail))
                if detail.contains("mob_storage() and mob_spec() are mutually exclusive")));
        assert!(!state_path.exists(), "refusal must precede storage setup");
    }

    #[tokio::test]
    async fn supplied_erased_spec_refuses_mcp_provider_override_before_bootstrap() {
        struct Provider;
        #[async_trait::async_trait]
        impl meerkat_mcp::McpCallContextProvider for Provider {
            async fn prepare(
                &self,
                _: meerkat_mcp::McpCallTarget<'_>,
                _: meerkat_core::types::ToolCallView<'_>,
                _: &meerkat_core::ToolDispatchContext,
            ) -> Result<Option<meerkat_mcp::McpCallContext>, meerkat_mcp::McpCallContextError>
            {
                panic!("erased spec override must refuse before preparing a call")
            }
        }
        let definition =
            MobDefinition::from_toml("[mob]\nid = \"mcp-override\"\n").expect("fixture definition");
        let spec = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .resolve_mob_spec()
            .await
            .expect("empty fixture spec");
        let result = UnifiedRuntimeBuilder::default()
            .mob_spec(spec)
            .mcp_call_context_provider(Arc::new(Provider))
            .build()
            .await;
        assert!(matches!(result,
            Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(detail))
                if detail.contains("mob_spec() is already erased")));
    }

    /// A supplied, already-erased spec cannot take the refusal observer: the
    /// builder refuses the pair instead of running unobserved.
    #[tokio::test]
    async fn supplied_erased_spec_refuses_refusal_observer_before_bootstrap() {
        struct Observer;
        impl meerkat_core::OperationRefusalObserver for Observer {
            fn observe(&self, _: &meerkat_core::OperationRefusalObservation) {
                panic!("an erased spec override must refuse before any agent runs")
            }
        }
        let definition = MobDefinition::from_toml("[mob]\nid = \"observer-override\"\n")
            .expect("fixture definition");
        let spec = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .resolve_mob_spec()
            .await
            .expect("empty fixture spec");
        let result = UnifiedRuntimeBuilder::default()
            .mob_spec(spec)
            .operation_refusal_observer(Arc::new(Observer))
            .build()
            .await;
        assert!(matches!(result,
            Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(detail))
                if detail.contains("refusal observer")
                    && detail.contains("mob_spec() is already erased")));
    }

    /// A supplied, already-erased spec built its agent factories already: the
    /// builder refuses a comms identity root it could not apply.
    #[tokio::test]
    async fn supplied_erased_spec_refuses_session_identity_root_before_bootstrap() {
        let definition = MobDefinition::from_toml("[mob]\nid = \"identity-root-override\"\n")
            .expect("fixture definition");
        let spec = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .resolve_mob_spec()
            .await
            .expect("empty fixture spec");
        let dir = tempfile::tempdir().expect("temporary directory");
        let result = UnifiedRuntimeBuilder::default()
            .mob_spec(spec)
            .session_identity_config_root(dir.path())
            .build()
            .await;
        assert!(matches!(result,
            Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(detail))
                if detail.contains("session_identity_config_root()")
                    && detail.contains("mob_spec() is already erased")));
    }

    /// An empty comms identity root is refused, as the gateway refuses an
    /// empty runtime option, before anything is composed.
    #[tokio::test]
    async fn an_empty_session_identity_root_is_refused() {
        for root in ["", "  "] {
            let definition = MobDefinition::from_toml("[mob]\nid = \"empty-identity-root\"\n")
                .expect("fixture definition");
            let result = UnifiedRuntimeBuilder::default()
                .definition(definition)
                .session_identity_config_root(root)
                .build()
                .await;
            assert!(
                matches!(result,
                    Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(detail))
                        if detail.contains("session_identity_config_root() must not be empty")),
                "{root:?}"
            );
        }
    }

    /// The builder's fork instruction policy reaches the spec of each
    /// composition it owns; unset, the spec keeps Meerkat's default.
    #[tokio::test]
    async fn the_fork_instruction_policy_reaches_every_builder_owned_spec() {
        use meerkat_core::ForkInstructionPolicy;
        let dir = tempfile::tempdir().expect("temporary directory");
        for arm in ["persistent", "scratch", "temporary"] {
            for (index, policy) in [
                None,
                Some(ForkInstructionPolicy::ReactivateInherited),
                Some(ForkInstructionPolicy::Refuse),
            ]
            .into_iter()
            .enumerate()
            {
                let definition = MobDefinition::from_toml("[mob]\nid = \"builder-fork-policy\"\n")
                    .expect("fixture definition");
                let builder = UnifiedRuntimeBuilder::default().definition(definition);
                let builder = match arm {
                    "persistent" => {
                        builder.persistent_state(dir.path().join(format!("state-{index}")))
                    }
                    "scratch" => builder.scratch_dir(dir.path().join(format!("scratch-{index}"))),
                    _ => builder,
                };
                let builder = match policy {
                    Some(policy) => builder.fork_instruction_policy(policy),
                    None => builder,
                };
                let spec = builder.resolve_mob_spec().await.expect("spec resolves");
                assert_eq!(
                    spec.fork_instruction_policy,
                    policy.unwrap_or(ForkInstructionPolicy::RequireExplicitActivation),
                    "{arm} {policy:?}",
                );
            }
        }
    }

    /// A supplied spec carries its own fork instruction policy: the builder
    /// refuses a second one instead of overriding it.
    #[tokio::test]
    async fn supplied_spec_refuses_a_builder_fork_instruction_policy() {
        let definition = MobDefinition::from_toml("[mob]\nid = \"fork-policy-override\"\n")
            .expect("fixture definition");
        let spec = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .resolve_mob_spec()
            .await
            .expect("empty fixture spec");
        let result = UnifiedRuntimeBuilder::default()
            .mob_spec(spec)
            .fork_instruction_policy(meerkat_core::ForkInstructionPolicy::Refuse)
            .build()
            .await;
        assert!(matches!(result,
            Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(detail))
                if detail.contains("fork_instruction_policy()")
                    && detail.contains("mob_spec()")));
    }

    /// A document bundle's declared read/edit tools; composition never
    /// dispatches them.
    #[cfg(feature = "extension-state")]
    struct DocumentProbe {
        read: &'static str,
        edit: &'static str,
    }

    #[cfg(feature = "extension-state")]
    #[async_trait::async_trait]
    impl meerkat_core::AgentToolDispatcher for DocumentProbe {
        fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
            [self.read, self.edit]
                .into_iter()
                .map(|name| {
                    Arc::new(meerkat_core::types::ToolDef::new(
                        name,
                        "Document realm probe",
                        serde_json::json!({"type": "object"}),
                    ))
                })
                .collect::<Vec<_>>()
                .into()
        }

        fn tool_mutation_class(&self, name: &str) -> meerkat_core::ToolMutationClass {
            if name == self.read {
                meerkat_core::ToolMutationClass::ReadOnly
            } else if name == self.edit {
                meerkat_core::ToolMutationClass::Mutating
            } else {
                meerkat_core::ToolMutationClass::Unknown
            }
        }

        async fn dispatch(
            &self,
            _: meerkat_core::types::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            panic!("realm composition must not execute a document operation")
        }
    }

    #[cfg(feature = "extension-state")]
    struct NoDocumentCalls;

    #[cfg(feature = "extension-state")]
    #[async_trait::async_trait]
    impl meerkat_mcp::McpCallContextProvider for NoDocumentCalls {
        async fn prepare(
            &self,
            _: meerkat_mcp::McpCallTarget<'_>,
            _: meerkat_core::types::ToolCallView<'_>,
            _: &meerkat_core::ToolDispatchContext,
        ) -> Result<Option<meerkat_mcp::McpCallContext>, meerkat_mcp::McpCallContextError> {
            panic!("realm composition must not prepare a document MCP call")
        }
    }

    /// A non-disk meerkat-level backend over in-memory stores, each declared
    /// ephemeral.
    #[cfg(feature = "extension-state")]
    struct RemoteRealm;

    #[cfg(feature = "extension-state")]
    #[async_trait::async_trait]
    impl meerkat::storage_provider::RealmStorageProvider for RemoteRealm {
        fn name(&self) -> &'static str {
            "remote-realm"
        }

        async fn open(
            &self,
            ctx: &meerkat::storage_provider::RealmOpenContext,
        ) -> Result<meerkat::storage_provider::RealmStoreSet, meerkat::PersistenceError> {
            Ok(meerkat::storage_provider::RealmStoreSet {
                session_store: Arc::new(meerkat_store::MemoryStore::new()),
                runtime_store: Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
                job_store: Arc::new(meerkat::MemoryDetachedJobStore::new()),
                schedule_store: Arc::new(meerkat_schedule::MemoryScheduleStore::new()),
                workgraph_store: Arc::new(meerkat::MemoryWorkGraphStore::new()),
                blob_store: Arc::new(meerkat_store::MemoryBlobStore::new()),
                artifact_store: Arc::new(meerkat_store::MemoryArtifactStore::new()),
                store_path: ctx.paths.root.clone(),
                projection_root: None,
                durability: [
                    "sessions",
                    "runtime",
                    "jobs",
                    "schedule",
                    "workgraph",
                    "blobs",
                    "artifacts",
                ]
                .iter()
                .map(|domain| {
                    meerkat_core::DurabilityDeclaration::durable(
                        domain,
                        meerkat_core::DurabilityResolution::DeclaredEphemeral,
                    )
                })
                .collect(),
            })
        }
    }

    /// One mob per test: concurrent tests share the process's comms names.
    #[cfg(feature = "extension-state")]
    fn realm_definition(mob: &str) -> MobDefinition {
        MobDefinition::from_toml(&format!(
            "[mob]\nid = \"extension-realm-{mob}\"\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
             [profiles.worker.tools]\ncomms = true\n"
        ))
        .expect("fixture definition")
    }

    /// A state directory the predecessor wrote (its realm directory with a
    /// jobs database), adopted on disk, which records the predecessor realm.
    #[cfg(feature = "extension-state")]
    async fn disk_adopted_state(root: &std::path::Path) -> PathBuf {
        let state = root.join("adopted");
        let jobs =
            meerkat_store::realm_paths_in(&state, crate::storage_predecessor::PREDECESSOR_REALM_ID)
                .jobs_sqlite_path;
        std::fs::create_dir_all(jobs.parent().expect("realm directory"))
            .expect("predecessor realm directory");
        drop(meerkat::SqliteDetachedJobStore::open(&jobs).expect("predecessor jobs"));
        let report = crate::storage_predecessor::adopt_predecessor_state(
            &crate::storage_predecessor::PredecessorAdoptRequest {
                state_dir: state.clone(),
                mode: crate::storage_migrate::MigrateMode::Apply,
                ..Default::default()
            },
        )
        .await;
        assert!(!report.has_errors(), "{:?}", report.errors);
        state
    }

    /// Compose `builder`'s own spec with a native document bundle and a
    /// document MCP binding, bootstrap it and spawn one member. Returns the
    /// realm of the member's resolved caller context, of the native bundle's
    /// documents and of the MCP binding's documents.
    #[cfg(feature = "extension-state")]
    async fn composed_extension_realms(builder: UnifiedRuntimeBuilder) -> [String; 3] {
        use crate::extension_state::{
            ChildMcpAvailability, HostMcpDocumentBinding, ToolBundleContext, ToolBundleRequirements,
        };
        let native = Arc::new(std::sync::Mutex::new(None::<ToolBundleContext>));
        let mcp_realm = Arc::new(std::sync::Mutex::new(None::<String>));
        let (native_capture, mcp_capture) = (Arc::clone(&native), Arc::clone(&mcp_realm));
        let public = meerkat_core::McpServerConfig::stdio(
            "documents",
            "documents-fixture",
            vec!["mcp".into()],
            std::collections::HashMap::from([("DOCUMENT_TOKEN".into(), String::new())]),
        );
        let mut runtime_config = public.clone();
        let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) =
            &mut runtime_config.transport
        else {
            panic!("the fixture descriptor is stdio");
        };
        stdio
            .env
            .insert("DOCUMENT_TOKEN".into(), "host-secret".into());
        let binding = HostMcpDocumentBinding::new(
            public,
            runtime_config,
            ToolBundleRequirements::durable_documents(
                "documents-fixture",
                "documents_read",
                "documents_apply",
            ),
            Arc::new(
                move |context: ToolBundleContext| -> Result<
                    Arc<dyn meerkat_core::AgentToolDispatcher>,
                    String,
                > {
                    *mcp_capture.lock().expect("capture") =
                        Some(context.documents.scope().realm().to_string());
                    Ok(Arc::new(DocumentProbe {
                        read: "documents_read",
                        edit: "documents_apply",
                    }))
                },
            ),
            Arc::new(NoDocumentCalls),
            ChildMcpAvailability::ChildAvailable,
        )
        .expect("document MCP binding");
        let mut builder = builder
            .register_tool_bundle_factory(
                "notes",
                ToolBundleRequirements::durable_documents("notes", "notes_read", "notes_apply"),
                Arc::new(
                    move |context: ToolBundleContext| -> Result<
                        Arc<dyn meerkat_core::AgentToolDispatcher>,
                        String,
                    > {
                        *native_capture.lock().expect("capture") = Some(context);
                        Ok(Arc::new(DocumentProbe {
                            read: "notes_read",
                            edit: "notes_apply",
                        }))
                    },
                ),
            )
            .register_mcp_tool_bundle_factory(binding);
        let mut spec = builder.resolve_mob_spec().await.expect("spec resolves");
        spec.options = MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: false,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        };
        let registry = builder
            .materialize_extension_factories(&mut spec)
            .await
            .expect("extension authority composes")
            .expect("registered extensions activate");
        let runtime = crate::MobRuntime::bootstrap(spec).await.expect("bootstrap");
        let handle = runtime.handle();
        let identity = meerkat_mob::AgentIdentity::from("member");
        handle
            .spawn_spec(SpawnMemberSpec::host_root("worker", identity.clone()))
            .await
            .expect("spawn");
        handle
            .wait_for_members_kickoff_complete(
                std::slice::from_ref(&identity),
                // A settle bound only: the assertions below check state, not elapsed time.
                Some(Duration::from_secs(30)),
            )
            .await
            .expect("kickoff settles");
        let session_id = handle
            .get_member(&identity)
            .await
            .expect("roster")
            .expect("member")
            .bridge_session_id()
            .expect("bridge session")
            .clone();
        let native = native
            .lock()
            .expect("capture")
            .take()
            .expect("native bundle activated");
        let caller = native
            .caller_resolver
            .resolve(
                &meerkat_core::ToolDispatchContext::default()
                    .with_runtime_identity(session_id, None),
                None,
            )
            .await
            .expect("the member's caller context resolves");
        handle.stop().await.expect("stop");
        drop(registry);
        [
            caller.realm().to_string(),
            native.documents.scope().realm().to_string(),
            mcp_realm
                .lock()
                .expect("capture")
                .take()
                .expect("document MCP binding activated"),
        ]
    }

    #[cfg(feature = "extension-state")]
    fn realms(realm: &str) -> [String; 3] {
        [realm.to_string(), realm.to_string(), realm.to_string()]
    }

    #[cfg(feature = "extension-state")]
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_realm_of_a_fresh_installation_is_the_default() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let builder = UnifiedRuntimeBuilder::default()
            .definition(realm_definition("fresh"))
            .persistent_state(dir.path().join("fresh"));
        assert_eq!(
            composed_extension_realms(builder).await,
            realms(crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID)
        );
    }

    #[cfg(feature = "extension-state")]
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_realm_follows_a_disk_adopted_predecessor() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let builder = UnifiedRuntimeBuilder::default()
            .definition(realm_definition("disk"))
            .persistent_state(disk_adopted_state(dir.path()).await);
        assert_eq!(
            composed_extension_realms(builder).await,
            realms(crate::storage_predecessor::PREDECESSOR_REALM_ID)
        );
    }

    #[cfg(feature = "extension-state")]
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_realm_follows_a_declared_non_disk_predecessor() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let state = dir.path().join("remote");
        // The meerkat-level bundle `storage_provider()` installs for a non-disk
        // backend declared as the adopted predecessor.
        let ctx = crate::storage_provider::MetaRealmOpenContext {
            meerkat_realm_declaration: Some(
                crate::storage_provider::MeerkatLevelRealmDeclaration::AdoptedPredecessor,
            ),
            declared_ephemeral_domains: vec!["runtime".into(), "workgraph".into(), "jobs".into()],
            ..crate::storage_provider::MetaRealmOpenContext::for_state_dir(state.clone())
        };
        let stores = crate::storage_provider::open_provider_meerkat_stores(&RemoteRealm, &ctx)
            .await
            .expect("the declared predecessor backend opens");
        assert_eq!(
            stores.realm.as_str(),
            crate::storage_predecessor::PREDECESSOR_REALM_ID
        );
        let mut builder = UnifiedRuntimeBuilder::default()
            .definition(realm_definition("remote"))
            .persistent_state(&state);
        builder.provider_meerkat_stores = Some(stores);
        assert_eq!(
            composed_extension_realms(builder).await,
            realms(crate::storage_predecessor::PREDECESSOR_REALM_ID)
        );
    }

    /// A supplied spec keys documents by the realm its own persistent
    /// composition resolved, over that same state directory only: composed
    /// over another directory, it refuses before document storage opens.
    #[cfg(feature = "extension-state")]
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_realm_of_a_supplied_spec_is_its_own_installation_realm() {
        use crate::extension_state::{ToolBundleContext, ToolBundleRequirements};
        let dir = tempfile::tempdir().expect("temporary directory");
        let adopted = disk_adopted_state(dir.path()).await;
        let supplied = || async {
            MobBootstrapSpec::persistent(
                realm_definition("supplied"),
                MobStorage::in_memory(),
                MobStorageProvenance::declared_ephemeral(),
                adopted.clone(),
                4,
            )
            .await
            .expect("supplied persistent spec")
        };
        let documents_realm = Arc::new(std::sync::Mutex::new(None::<String>));
        let with_bundle = |builder: UnifiedRuntimeBuilder| {
            let capture = Arc::clone(&documents_realm);
            builder.register_tool_bundle_factory(
                "notes",
                ToolBundleRequirements::durable_documents("notes", "notes_read", "notes_apply"),
                Arc::new(
                    move |context: ToolBundleContext| -> Result<
                        Arc<dyn meerkat_core::AgentToolDispatcher>,
                        String,
                    > {
                        *capture.lock().expect("capture") =
                            Some(context.documents.scope().realm().to_string());
                        Ok(Arc::new(DocumentProbe {
                            read: "notes_read",
                            edit: "notes_apply",
                        }))
                    },
                ),
            )
        };

        let elsewhere = dir.path().join("elsewhere");
        let mut builder =
            with_bundle(UnifiedRuntimeBuilder::default().persistent_state(&elsewhere));
        let mut spec = supplied().await;
        let error = match builder.materialize_extension_factories(&mut spec).await {
            Err(error) => error,
            Ok(_) => panic!("a spec composed over another state directory must refuse"),
        };
        assert!(
            matches!(
                &error,
                UnifiedRuntimeBuilderError::ConflictingConfiguration(detail)
                    if detail.contains("composed over")
            ),
            "{error}"
        );
        assert!(
            !elsewhere.exists(),
            "the refusal must precede opening document storage"
        );
        assert!(documents_realm.lock().expect("capture").is_none());
        drop(spec);
        drop(builder);

        let mut builder = with_bundle(UnifiedRuntimeBuilder::default().persistent_state(&adopted));
        let mut spec = supplied().await;
        let registry = builder
            .materialize_extension_factories(&mut spec)
            .await
            .expect("a spec composed over persistent_state() composes")
            .expect("the registered bundle activates");
        assert_eq!(
            documents_realm.lock().expect("capture").take().as_deref(),
            Some(crate::storage_predecessor::PREDECESSOR_REALM_ID)
        );
        drop(registry);
    }

    /// H1: a persistent-state build with an injected blob store that reports
    /// `!is_persistent()` fails composition unless `ephemeral_blobs(true)`
    /// declares the choice — declared, it composes and reports a
    /// non-persistent custom blob slot.
    #[tokio::test]
    async fn persistent_state_gates_non_persistent_blob_store_on_declaration() {
        let dir = tempfile::tempdir().expect("temp dir");
        let memory_blobs: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(crate::blob_store::Base64BlobStoreAdapter::new(Arc::new(
                crate::blob_store::ObjectStoreBlobStore::memory(),
            )));
        let definition = || {
            MobDefinition::from_toml("[mob]\nid = \"blob-declaration-test\"\n")
                .expect("parse test mob definition")
        };

        let undeclared = UnifiedRuntimeBuilder::default()
            .definition(definition())
            .persistent_state(dir.path())
            .blob_store(memory_blobs.clone());
        match undeclared.resolve_mob_spec().await {
            Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(message)) => {
                assert!(
                    message.contains("ephemeral_blobs"),
                    "the error must name the remediation, got: {message}"
                );
            }
            Err(other) => panic!("expected ConflictingConfiguration, got: {other}"),
            Ok(_) => panic!("undeclared non-persistent blob store must fail composition"),
        }

        let declared = UnifiedRuntimeBuilder::default()
            .definition(definition())
            .persistent_state(dir.path())
            .blob_store(memory_blobs)
            .ephemeral_blobs(true);
        let spec = declared
            .resolve_mob_spec()
            .await
            .unwrap_or_else(|e| panic!("declared ephemeral blobs must compose: {e}"));
        let summary = spec
            .resolved_storage
            .expect("persistent builds record their storage");
        assert_eq!(
            summary.blob_durability,
            crate::storage_health::BlobDurability::Custom { persistent: false }
        );
        // The repair commands of a session repair hold derive from the
        // runtime store this build opened: the default persistent layout's
        // single SQLite file, addressed directly.
        assert_eq!(
            summary.runtime_store_locator,
            Some(crate::storage_health::RuntimeStoreLocator::SqliteFile {
                path: dir.path().join(crate::storage_layout::RUNTIME_DB_FILE_NAME),
            })
        );
        let scope = crate::identity_first::SessionRepairScope::from_resolved_storage(&summary)
            .expect("a persistent runtime store derives a repair scope");
        assert_eq!(
            scope.runtime_store,
            Some(dir.path().join(crate::storage_layout::RUNTIME_DB_FILE_NAME))
        );
        assert_eq!(scope.state_root, None);
    }

    fn deferred_capacity_request(prompt: impl Into<String>) -> CreateSessionRequest {
        let build = meerkat_core::service::SessionBuildOptions {
            llm_client_override: Some(meerkat::encode_llm_client_override_for_service(Arc::new(
                meerkat_client::TestClient::default(),
            ))),
            ..Default::default()
        };

        CreateSessionRequest {
            model: "gpt-5.5".to_string(),
            prompt: meerkat_core::ContentInput::Text(prompt.into()),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
            injected_context: Vec::new(),
        }
    }

    #[tokio::test]
    async fn definition_based_ephemeral_spec_provides_runtime_adapter() {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "builder-ephemeral"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"

[profiles.worker.tools]
comms = true
"#,
        )
        .expect("definition parses");

        let builder = UnifiedRuntimeBuilder::default().definition(definition);
        let spec = builder.resolve_mob_spec().await.expect("spec resolves");
        assert!(
            spec.runtime_adapter.is_some(),
            "definition-based ephemeral specs should expose runtime authority",
        );
    }

    #[tokio::test]
    async fn definition_based_ephemeral_spec_uses_configured_max_sessions() {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "builder-max-sessions"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
"#,
        )
        .expect("definition parses");

        let builder = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .max_sessions(65);
        let spec = builder.resolve_mob_spec().await.expect("spec resolves");

        for index in 0..65 {
            SessionService::create_session(
                spec.session_service.as_ref(),
                deferred_capacity_request(format!("session {index}")),
            )
            .await
            .expect("configured capacity should admit session");
        }

        let blocked = SessionService::create_session(
            spec.session_service.as_ref(),
            deferred_capacity_request("one too many"),
        )
        .await
        .expect_err("configured capacity should block the next session");
        assert!(
            blocked.to_string().contains("Max sessions reached (65/65)"),
            "unexpected capacity error: {blocked}",
        );
    }

    #[tokio::test]
    async fn definition_based_spec_accepts_custom_meerkat_config() {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "builder-custom-config"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
"#,
        )
        .expect("definition parses");
        let mut config = meerkat::Config::default();
        config.compaction.auto_compact_threshold = 42_000;
        config.compaction.auto_compact_threshold_explicit = true;
        config.compaction.recent_turn_budget = 2;

        let builder = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .meerkat_config(config)
            .max_sessions(1);
        let spec = builder.resolve_mob_spec().await.expect("spec resolves");

        SessionService::create_session(
            spec.session_service.as_ref(),
            deferred_capacity_request("custom config session"),
        )
        .await
        .expect("custom Meerkat config should still build a usable session service");
    }

    #[test]
    fn model_fallback_host_config_survives_compaction_composition() {
        let mut config = meerkat::Config::default();
        config.model_fallback.enabled = Some(false);
        config.model_fallback.policy.trigger_after_attempts = 7;
        let expected = config.model_fallback.clone();
        let builder = UnifiedRuntimeBuilder::default()
            .meerkat_config(config)
            .compaction(meerkat_core::config::CompactionRuntimeConfig {
                auto_compact_threshold: 120_000,
                auto_compact_threshold_explicit: true,
                ..Default::default()
            });
        let effective = builder
            .effective_meerkat_config()
            .expect("config")
            .expect("present");
        assert_eq!(effective.model_fallback, expected);
        assert_eq!(effective.compaction.auto_compact_threshold, 120_000);
        assert!(!meerkat::Config::default().model_fallback.is_enabled());
    }

    #[tokio::test]
    async fn definition_based_persistent_spec_uses_configured_max_sessions() {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "builder-persistent-max-sessions"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
"#,
        )
        .expect("definition parses");
        let tmp = tempfile::tempdir().expect("temp dir");
        let builder = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .persistent_state(tmp.path().join("state"))
            .max_sessions(2);
        let spec = builder.resolve_mob_spec().await.expect("spec resolves");

        for index in 0..2 {
            SessionService::create_session(
                spec.session_service.as_ref(),
                deferred_capacity_request(format!("persistent session {index}")),
            )
            .await
            .expect("configured persistent capacity should admit session");
        }

        let blocked = SessionService::create_session(
            spec.session_service.as_ref(),
            deferred_capacity_request("persistent one too many"),
        )
        .await
        .expect_err("configured persistent capacity should block the next session");
        assert!(
            blocked.to_string().contains("Max sessions reached (2/2)"),
            "unexpected capacity error: {blocked}",
        );
    }

    #[tokio::test]
    async fn definition_based_spec_rejects_zero_max_sessions() {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "builder-zero-max-sessions"

[profiles.worker]
model = "gpt-5.5"
"#,
        )
        .expect("definition parses");

        let result = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .max_sessions(0)
            .resolve_mob_spec()
            .await;
        assert!(result.is_err(), "zero max sessions should be rejected");
        let err = result.err().expect("zero max sessions error");

        assert!(
            err.to_string().contains("max_sessions"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn builder_accepts_custom_console_log_store_for_ephemeral_mob_state() {
        let store: Arc<dyn ConsoleLogStore> = Arc::new(InMemoryConsoleLogStore::new());
        let builder = UnifiedRuntimeBuilder::default().with_console_log_store(store.clone());

        assert!(
            Arc::ptr_eq(
                builder.console_log_store.as_ref().expect("custom store"),
                &store
            ),
            "builder should retain the exact console log store supplied by the app"
        );
    }

    /// The same-root gate compares PHYSICAL identity, not raw spelling: one
    /// root spelled two equivalent ways must pass, a not-yet-created root
    /// still compares through its deepest existing ancestor, and genuinely
    /// distinct directories must refuse. The lexical compare it replaces got
    /// all of this wrong — and let two lexically equal relative spellings
    /// that resolve under different working directories fork silently.
    #[test]
    fn same_root_gate_compares_physical_identity() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let root = tmp.path().join("state");
        std::fs::create_dir_all(&root).expect("create state root");

        // Equivalent spelling through an existing `..` detour: ONE root.
        let detour = tmp.path().join("detour");
        std::fs::create_dir_all(&detour).expect("create detour dir");
        let spelled_via_detour = detour.join("..").join("state");
        verify_shared_state_root(&root, &spelled_via_detour)
            .expect("equivalent spellings of one root must pass the gate");

        // A not-yet-created state root still compares by physical identity
        // (deepest existing ancestor canonicalized, remainder rejoined).
        let future_root = tmp.path().join("future");
        verify_shared_state_root(&future_root, &detour.join("..").join("future"))
            .expect("not-yet-created roots must still compare physically");

        // Genuinely distinct directories fork session authority: refuse.
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&other).expect("create other root");
        let err = verify_shared_state_root(&root, &other).expect_err("distinct roots must refuse");
        match err {
            UnifiedRuntimeBuilderError::ConflictingConfiguration(message) => {
                assert!(
                    message.contains("share ONE state directory"),
                    "refusal must explain the pairing requirement: {message}"
                );
            }
            other => panic!("expected ConflictingConfiguration, got: {other}"),
        }
    }

    /// Symlinks resolve to their targets before the same-root comparison: a
    /// symlink and its target are the ONE directory they physically are, and
    /// a symlink to a DIFFERENT directory is exactly the silent
    /// session-authority fork the gate exists to refuse, whatever the
    /// spelling suggests.
    #[cfg(unix)]
    #[test]
    fn same_root_gate_resolves_symlinks() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let target = tmp.path().join("state");
        std::fs::create_dir_all(&target).expect("create state root");
        let link = tmp.path().join("state-link");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");

        verify_shared_state_root(&link, &target)
            .expect("a symlink and its target are ONE state directory");

        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("create other root");
        let fork = tmp.path().join("fork-link");
        std::os::unix::fs::symlink(&elsewhere, &fork).expect("create fork symlink");
        assert!(
            verify_shared_state_root(&target, &fork).is_err(),
            "a symlink to another directory must refuse"
        );
    }

    /// End-to-end wiring: `build()` refuses a persistent_state root that is
    /// not the physical directory `continuity_from_state_dir` opened.
    #[tokio::test]
    async fn build_refuses_forked_continuity_and_state_roots() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let result = UnifiedRuntimeBuilder::default()
            .persistent_state(tmp.path().join("stores"))
            .continuity_from_state_dir(tmp.path().join("substrate"))
            .await
            .expect("open the substrate root")
            .build()
            .await;
        match result {
            Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(message)) => {
                assert!(
                    message.contains("share ONE state directory"),
                    "refusal must explain the pairing requirement: {message}"
                );
            }
            Err(other) => panic!("expected ConflictingConfiguration, got: {other}"),
            Ok(_) => panic!("forked continuity/state roots must refuse at build"),
        }
    }

    // ── Session-compaction wiring ───────────────────────────────────────
    //
    // Composition-level pins for the host compaction policy. The behavioral
    // proof that the built session path actually carries a compactor (and
    // that this policy is the trigger it uses) lives in
    // `tests/session_compaction_wiring.rs`, which drives real member turns.

    fn compaction_test_definition(id: &str) -> MobDefinition {
        MobDefinition::from_toml(&format!("[mob]\nid = \"{id}\"\n"))
            .expect("compaction fixture definition parses")
    }

    /// An invalid declaration is a composition error, never a silently
    /// ignored knob.
    #[tokio::test]
    async fn zero_compaction_threshold_refuses_composition() {
        let builder = UnifiedRuntimeBuilder::default()
            .definition(compaction_test_definition("compaction-policy-zero"))
            .compaction(meerkat_core::config::CompactionRuntimeConfig {
                auto_compact_threshold: 0,
                auto_compact_threshold_explicit: true,
                ..Default::default()
            });
        match builder.resolve_mob_spec().await {
            Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(message)) => {
                assert!(message.contains("greater than 0"), "{message}");
            }
            Err(other) => panic!("expected ConflictingConfiguration, got: {other}"),
            Ok(_) => panic!("a zero compaction threshold must refuse composition"),
        }
    }

    /// `compaction()` composes over `meerkat_config()` instead of replacing
    /// it: the rest of the caller's config survives.
    #[tokio::test]
    async fn compaction_policy_composes_over_meerkat_config() {
        let mut config = meerkat::Config::default();
        config.budget.max_tokens = Some(4_242);
        let builder = UnifiedRuntimeBuilder::default()
            .definition(compaction_test_definition("compaction-policy-compose"))
            .meerkat_config(config)
            .compaction(meerkat_core::config::CompactionRuntimeConfig {
                auto_compact_threshold: 77_000,
                auto_compact_threshold_explicit: true,
                ..Default::default()
            });
        let effective = builder
            .effective_meerkat_config()
            .expect("composition succeeds")
            .expect("a configured builder yields a config");
        assert_eq!(effective.compaction.auto_compact_threshold, 77_000);
        assert!(effective.compaction.auto_compact_threshold_explicit);
        assert_eq!(
            effective.budget.max_tokens,
            Some(4_242),
            "the rest of the caller's meerkat_config must survive",
        );
    }

    /// A pre-built spec already carries its own compactor, so accepting a
    /// declaration here would be exactly the dead knob this seam removes.
    #[tokio::test]
    async fn compaction_policy_refuses_the_legacy_mob_spec_path() {
        let temp = tempfile::tempdir().expect("temp dir");
        let spec = MobBootstrapSpec::ephemeral(
            compaction_test_definition("compaction-policy-legacy"),
            MobStorage::in_memory(),
            temp.path().to_path_buf(),
            4,
            None,
        )
        .expect("bootstrap spec");
        let result = UnifiedRuntimeBuilder::default()
            .mob_spec(spec)
            .module_config(MetaConfig {
                modules: Vec::new(),
                discovery: crate::types::DiscoverySpec {
                    namespace: "compaction-policy-legacy".to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            })
            .timeout(Duration::from_secs(5))
            .compaction(meerkat_core::config::CompactionRuntimeConfig::default())
            .build()
            .await;
        match result {
            Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(message)) => {
                assert!(message.contains("mutually exclusive"), "{message}");
            }
            Err(other) => panic!("expected ConflictingConfiguration, got: {other}"),
            Ok(_) => panic!("compaction() with a pre-built mob_spec must refuse"),
        }
    }

    // ── Provider credential store ───────────────────────────────────────

    /// A credential store over `root`, as a host opens one.
    fn credential_store(root: &std::path::Path) -> meerkat::ProviderAuthPersistence {
        meerkat::TokenStoreBackend::File {
            root: root.to_path_buf(),
        }
        .open_with_refresh_authority()
        .expect("credential store")
    }

    /// A supplied, already-erased spec built its agent factories already: the
    /// builder refuses a provider credential store it could not attach.
    #[tokio::test]
    async fn supplied_erased_spec_refuses_provider_credential_store_before_bootstrap() {
        let definition = MobDefinition::from_toml("[mob]\nid = \"credential-store-override\"\n")
            .expect("fixture definition");
        let spec = UnifiedRuntimeBuilder::default()
            .definition(definition)
            .resolve_mob_spec()
            .await
            .expect("empty fixture spec");
        let dir = tempfile::tempdir().expect("temporary directory");
        let result = UnifiedRuntimeBuilder::default()
            .mob_spec(spec)
            .provider_auth_persistence(credential_store(&dir.path().join("credentials")))
            .build()
            .await;
        assert!(matches!(result,
            Err(UnifiedRuntimeBuilderError::ConflictingConfiguration(detail))
                if detail.contains("provider_auth_persistence()")
                    && detail.contains("mob_spec() is already erased")));
    }

    /// The store a host names reaches the member builder's agent factory, and
    /// the live factory where the launch keeps one, for persistent, scratch
    /// and temporary launches. Two runtimes whose agents use the same realm
    /// and binding ids keep their credentials apart when each names its own
    /// root: a credential written through one runtime's member factory lands
    /// under that runtime's root only. A cold reopen of a state directory
    /// with a store freshly opened over the same root reads what the first
    /// life wrote, and a builder that names no store keeps the process
    /// default. The body runs in a child process whose process default store
    /// lies in a scratch directory, and nothing reaches it.
    #[tokio::test]
    async fn named_credential_stores_reach_every_builder_launch_and_stay_apart() {
        let scratch = tempfile::tempdir().expect("scratch directory");
        let config_home = scratch.path().join("config");
        let home = scratch.path().join("home");
        if crate::test_process_isolation::run_body_in_isolated_process_with_env(
            module_path!(),
            "named_credential_stores_reach_every_builder_launch_and_stay_apart",
            &[
                ("XDG_CONFIG_HOME", config_home.as_path()),
                ("HOME", home.as_path()),
            ],
        ) {
            named_credential_stores_body().await;
            return;
        }
        for default_store in [
            config_home.join("meerkat").join("credentials"),
            home.join(".config").join("meerkat").join("credentials"),
            home.join("Library")
                .join("Application Support")
                .join("meerkat")
                .join("credentials"),
        ] {
            assert!(
                !default_store.exists(),
                "nothing may reach the process default store {}",
                default_store.display()
            );
        }
    }

    async fn named_credential_stores_body() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let key = meerkat_core::auth::TokenKey::parse("shared-realm", "shared-binding")
            .expect("token key");
        let mut named = Vec::new();
        for arm in ["persistent", "scratch", "temporary"] {
            let mut launches = Vec::new();
            for name in ["a", "b"] {
                let launch = dir.path().join(arm).join(name);
                let root = launch.join("credentials");
                let store = credential_store(&root);
                named.push(store.authority_id());
                let definition = MobDefinition::from_toml(&format!(
                    "[mob]\nid = \"credentials-{arm}-{name}\"\n"
                ))
                .expect("fixture definition");
                let builder = UnifiedRuntimeBuilder::default()
                    .definition(definition)
                    .provider_auth_persistence(store.clone());
                let builder = match arm {
                    "persistent" => builder.persistent_state(launch.join("state")),
                    "scratch" => builder.scratch_dir(launch.join("scratch")),
                    _ => builder,
                };
                let spec = builder.resolve_mob_spec().await.expect("spec resolves");
                let member = spec
                    .member_provider_auth_persistence
                    .as_ref()
                    .expect("the member factory holds a store");
                assert_eq!(
                    member.authority_id(),
                    store.authority_id(),
                    "{arm}/{name}: the member factory holds the named store"
                );
                if let Some(live) = spec.live_compose_inputs.as_ref() {
                    let live = live
                        .factory
                        .resolution_provider_auth_persistence()
                        .expect("the live factory's store")
                        .expect("the live factory holds a store");
                    assert_eq!(
                        live.authority_id(),
                        store.authority_id(),
                        "{arm}/{name}: the live factory holds the named store"
                    );
                }
                let secret = format!("{arm}-{name}-key");
                member
                    .token_store()
                    .save(
                        &key,
                        &meerkat_core::auth::PersistedTokens::api_key(secret.as_str()),
                    )
                    .await
                    .expect("save through the member factory's store");
                launches.push((root, store, secret, spec));
            }
            for (root, store, secret, _spec) in &launches {
                let stored = store
                    .token_store()
                    .load(&key)
                    .await
                    .expect("load")
                    .expect("the credential is stored");
                assert_eq!(
                    stored.primary_secret.as_deref(),
                    Some(secret.as_str()),
                    "{arm}: each root keeps its own runtime's credential"
                );
                assert!(
                    root.join("shared-realm")
                        .join("shared-binding.json")
                        .is_file(),
                    "{arm}: the credential lives under {}",
                    root.display()
                );
            }
        }

        // A cold reopen of a state directory, with a store freshly opened over
        // the same root, reads what the first life wrote.
        let reopened = dir.path().join("reopened");
        for life in ["first", "second"] {
            let store = credential_store(&reopened.join("credentials"));
            let spec = UnifiedRuntimeBuilder::default()
                .definition(
                    MobDefinition::from_toml("[mob]\nid = \"credentials-reopened\"\n")
                        .expect("fixture definition"),
                )
                .persistent_state(reopened.join("state"))
                .provider_auth_persistence(store.clone())
                .resolve_mob_spec()
                .await
                .expect("spec resolves");
            let member = spec
                .member_provider_auth_persistence
                .as_ref()
                .expect("the member factory holds a store");
            assert_eq!(
                member.authority_id(),
                store.authority_id(),
                "{life} life: the member factory holds the named store"
            );
            if life == "first" {
                member
                    .token_store()
                    .save(
                        &key,
                        &meerkat_core::auth::PersistedTokens::api_key("reopened-key"),
                    )
                    .await
                    .expect("save in the first life");
            } else {
                let stored = member
                    .token_store()
                    .load(&key)
                    .await
                    .expect("load")
                    .expect("the first life's credential");
                assert_eq!(stored.primary_secret.as_deref(), Some("reopened-key"));
            }
        }

        // Without the option the member factory keeps the process default.
        let spec = UnifiedRuntimeBuilder::default()
            .definition(
                MobDefinition::from_toml("[mob]\nid = \"credentials-default\"\n")
                    .expect("fixture definition"),
            )
            .resolve_mob_spec()
            .await
            .expect("spec resolves");
        let default_store = spec
            .member_provider_auth_persistence
            .as_ref()
            .map(meerkat::ProviderAuthPersistence::authority_id);
        assert!(
            default_store.is_none_or(|authority| !named.contains(&authority)),
            "a builder that names no store keeps the process default"
        );
    }

    /// A runtime the builder gave a provider credential store records it, so
    /// its RPC and its Flow Editor refuse a host deploy that could not carry
    /// the store; a runtime on the process default records none.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_builder_records_a_named_credential_store_on_its_runtime() {
        let dir = tempfile::tempdir().expect("temporary directory");
        for (named, id) in [
            (true, "credential-store-recorded"),
            (false, "credential-store-default"),
        ] {
            let mut builder = UnifiedRuntimeBuilder::default().definition(
                MobDefinition::from_toml(&format!("[mob]\nid = \"{id}\"\n"))
                    .expect("fixture definition"),
            );
            if named {
                builder = builder.provider_auth_persistence(credential_store(&dir.path().join(id)));
            }
            let runtime = Box::pin(builder.build()).await.expect("runtime builds");
            assert_eq!(runtime.has_explicit_provider_credentials(), named, "{id}");
            runtime.shutdown().await;
        }
    }
}
