#![allow(
    unused_imports,
    clippy::clone_on_copy,
    clippy::implicit_clone,
    clippy::redundant_clone
)]

mod agent_tools;
mod public_definition;
mod public_mcp;
#[cfg(not(target_arch = "wasm32"))]
mod schedule_host;
mod surface;
pub use agent_tools::{
    AgentMobToolSurface, AgentMobToolSurfaceFactory, archive_session_with_mob_cleanup,
};
pub use public_definition::decode_public_mob_definition;
pub use public_mcp::{
    handle_public_tools_call, public_tool_names, public_tools_list, wrap_public_tool_payload,
};
#[cfg(not(target_arch = "wasm32"))]
pub use schedule_host::MobMcpScheduleHost;
pub use surface::wire_mob_tools;

#[cfg(target_arch = "wasm32")]
mod tokio {
    pub use tokio_with_wasm::alias::*;
}

use async_trait::async_trait;

use meerkat_client::LlmClient;
use meerkat_contracts::{
    MobDefinitionInput, MobLifecycleParams, MobSpawnManyResultEntry, WireMemberRef,
    WireMobLifecycleAction, WireMobLifecycleStatus, WireMobRespawnOutcome, WireMobWireAction,
};
use meerkat_core::AppendSystemContextStatus;
use meerkat_core::ScopedAgentEvent;
use meerkat_core::agent::{
    AgentToolDispatcher, BindOutcome, CommsCapabilityError, CommsRuntime as CoreCommsRuntime,
    DispatcherCapabilities, OpsLifecycleBindError,
};
use meerkat_core::comms::{
    CommsCommand, CommsTrustMutation, CommsTrustMutationResult,
    GeneratedCommsTrustAuthoritySourceKind, PeerId, PeerName, SendError, SendReceipt,
    TrustedPeerDescriptor,
};
use meerkat_core::error::ToolError;
use meerkat_core::interaction::{InteractionId, PeerInputCandidate};
use meerkat_core::ops::{AsyncOpRef, OperationId, OperationResult};
use meerkat_core::ops_lifecycle::{OperationKind, OperationSpec, OpsLifecycleRegistry};
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    SessionControlError, SessionError, SessionHistoryPage, SessionHistoryQuery, SessionInfo,
    SessionQuery, SessionService, SessionServiceCommsExt, SessionServiceControlExt,
    SessionServiceHistoryExt, SessionSummary, SessionUsage, SessionView, StartTurnRequest,
};
use meerkat_core::time_compat::{Instant, SystemTime};
use meerkat_core::types::{
    ContentInput, HandlingMode, RenderMetadata, RunResult, SessionId, ToolCallView, ToolDef,
    ToolProvenance, ToolResult, ToolSourceKind, Usage,
};
use meerkat_core::{AgentEvent, EventEnvelope, EventStream, Provider, StreamError};
use meerkat_mob::definition::SkillSource;
use meerkat_mob::machines::mob_machine::ControlScope;
use meerkat_mob::{
    AgentIdentity, CommandAuthority, FlowId, MobBackendKind, MobBuilder, MobControlPrincipal,
    MobDefinition, MobError, MobHandle, MobId, MobRuntimeMode, MobSessionService, MobState,
    MobStorage, OperatorGrant, ProfileBinding, ProfileName, RunId, ScopeDenial, SpawnMemberSpec,
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, btree_map::Entry};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use ::tokio::sync::{Mutex, RwLock, mpsc};
#[cfg(target_arch = "wasm32")]
use tokio::sync::{Mutex, RwLock, mpsc};

#[derive(Clone)]
struct ManagedMob {
    handle: MobHandle,
    storage_path: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum MobMcpDestroyError {
    #[error("mob destroy incomplete: {}", destroy_report_summary(.report))]
    Incomplete {
        report: meerkat_mob::MobDestroyReport,
    },

    #[error(transparent)]
    Mob(#[from] MobError),
}

/// Error returned by the identity-addressed mob system-context surface.
///
/// Authorization belongs to the mob control plane, while the actual append
/// belongs to the member session service.  Keep both typed so a denied mob
/// principal retains the shared `ScopeDenied` wire detail instead of being
/// collapsed into a session-control string.
#[derive(Debug, thiserror::Error)]
pub enum MobAppendSystemContextError {
    #[error(transparent)]
    Mob(#[from] MobError),

    #[error(transparent)]
    Session(#[from] SessionControlError),
}

impl MobAppendSystemContextError {
    pub fn wire_detail(&self) -> Option<meerkat_contracts::wire::WireMobErrorDetail> {
        match self {
            Self::Mob(error) => error.wire_detail(),
            Self::Session(_) => None,
        }
    }
}

impl MobMcpDestroyError {
    pub fn incomplete_message(report: &meerkat_mob::MobDestroyReport) -> String {
        format!("mob destroy incomplete: {}", destroy_report_summary(report))
    }

    pub fn incomplete_error_data(report: &meerkat_mob::MobDestroyReport) -> serde_json::Value {
        json!({
            "code": "mob_destroy_incomplete",
            "destroy_report": report,
            "retryable": true,
        })
    }

    pub fn error_data(&self) -> Option<serde_json::Value> {
        match self {
            Self::Incomplete { report } => Some(Self::incomplete_error_data(report)),
            Self::Mob(_) => None,
        }
    }

    /// Delegating [`MobError::wire_detail`] (§17.4, ADJ-P7-4): `Mob(inner)`
    /// carries the inner console projection; the `Incomplete` report keeps
    /// its dedicated `mob_destroy_incomplete` data envelope.
    pub fn wire_detail(&self) -> Option<meerkat_contracts::wire::WireMobErrorDetail> {
        match self {
            Self::Incomplete { .. } => None,
            Self::Mob(inner) => inner.wire_detail(),
        }
    }

    fn into_mob_error(self) -> MobError {
        match self {
            Self::Incomplete { report } => MobError::Internal(Self::incomplete_message(&report)),
            Self::Mob(error) => error,
        }
    }

    pub fn into_session_error(self, context: &str) -> SessionError {
        match self {
            Self::Incomplete { report } => SessionError::FailedWithData {
                message: format!("{context}: {}", Self::incomplete_message(&report)),
                data: Self::incomplete_error_data(&report),
            },
            Self::Mob(error) => SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!("{context}: {error}")),
            ),
        }
    }
}

fn destroy_report_summary(report: &meerkat_mob::MobDestroyReport) -> String {
    if !report.errors.is_empty() {
        return report.errors.join("; ");
    }
    if report.remote_cleanup_deadline_exceeded {
        return "remote cleanup deadline exceeded".to_string();
    }
    if !report.orphaned_remote_members.is_empty() {
        return format!(
            "orphaned remote members: {}",
            report
                .orphaned_remote_members
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    "destroy cleanup did not complete".to_string()
}

type DefaultLlmClientProvider = Arc<dyn Fn() -> Option<Arc<dyn LlmClient>> + Send + Sync + 'static>;

#[doc(hidden)]
#[derive(Default)]
pub struct InMemoryArchiveFailureControl {
    failures: RwLock<HashMap<SessionId, String>>,
}

impl InMemoryArchiveFailureControl {
    pub async fn fail_archive(&self, id: SessionId, reason: impl Into<String>) {
        self.failures.write().await.insert(id, reason.into());
    }

    pub async fn clear_archive_failure(&self, id: &SessionId) {
        self.failures.write().await.remove(id);
    }

    async fn failure_for(&self, id: &SessionId) -> Option<String> {
        self.failures.read().await.get(id).cloned()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct KickoffMemberSnapshot {
    pub agent_identity: AgentIdentity,
    #[serde(flatten)]
    pub snapshot: meerkat_mob::MobMemberSnapshot,
}

/// Domain outcome for the RPC ingress composite.  Wire assembly remains in
/// the RPC crate, while admission, cursor capture, ensure, and delivery stay
/// inside one state-owned operation.
#[derive(Debug)]
pub struct MobIngressInteractionOutcome {
    /// Whether the declarative member step spawned or retained the target.
    pub ensure_outcome: meerkat_mob::runtime::EnsureMemberOutcome,
    /// Canonical member-delivery receipt.
    pub delivery: meerkat_mob::MemberDeliveryReceipt,
    /// Structural-event cursor observed before ensure+delivery.
    pub events_after_cursor: u64,
    /// Structural-event cursor observed after delivery admission.
    pub latest_event_cursor: u64,
}

fn persisted_mob_binding(
    view: &meerkat_core::PersistedSessionMetadataView,
) -> Option<meerkat_mob::MobId> {
    // Read the typed durable identity directly off the metadata view. Old
    // persisted rows written before `mob_member_binding` existed decode as
    // `None` and are simply not owned (back-read safe) — they re-acquire a
    // typed binding on the next build/resume. No comms_name string-split, no
    // `mob:{id}` realm format-string, no magic-key peer_meta cross-check.
    let binding = view.mob_member_binding()?;
    Some(meerkat_mob::MobId::from(binding.mob_id.as_str()))
}

/// Typed provenance of the shared realm-scoped profile store.
///
/// Replaces the former `(Option<Arc<dyn RealmProfileStore>>, explicit: bool)`
/// pair: the variant itself encodes *why* the store has its current value, so
/// the persistent-root auto-upgrade decision matches a variant instead of
/// re-deriving intent from a parallel bool. Builder-local only; never
/// serialized, persisted, or crossing any wire/surface boundary.
enum RealmProfileStoreSelection {
    /// Default in-memory store installed by the constructor. Eligible for
    /// auto-upgrade to durable SQLite under a persistent storage root.
    DefaultInMemory(Arc<dyn meerkat_mob::RealmProfileStore>),
    /// Default store that has already been upgraded to durable SQLite under a
    /// persistent storage root. Must not be upgraded again.
    ///
    /// Only constructed on non-wasm targets (the SQLite persistent-root upgrade
    /// is `#[cfg(not(wasm32))]`); on wasm32 it is an unreachable arm.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    DefaultDurable(Arc<dyn meerkat_mob::RealmProfileStore>),
    /// Caller-supplied store (or explicit `None` to disable). Must never be
    /// overridden by the persistent-root auto-upgrade.
    CallerSupplied(Option<Arc<dyn meerkat_mob::RealmProfileStore>>),
}

impl RealmProfileStoreSelection {
    /// Resolved store, regardless of provenance.
    fn store(&self) -> Option<&Arc<dyn meerkat_mob::RealmProfileStore>> {
        match self {
            Self::DefaultInMemory(store) | Self::DefaultDurable(store) => Some(store),
            Self::CallerSupplied(opt) => opt.as_ref(),
        }
    }

    /// Whether this is the auto-upgradeable default in-memory store. Only
    /// consulted by the non-wasm persistent-root auto-upgrade.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn is_default(&self) -> bool {
        matches!(self, Self::DefaultInMemory(_))
    }
}

/// In-memory MCP state for multiple mobs.
pub struct MobMcpState {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    workgraph_service: Option<meerkat::WorkGraphService>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
    default_llm_client_provider: Option<DefaultLlmClientProvider>,
    external_tools_provider: Option<meerkat_mob::ExternalToolsProvider>,
    persistent_storage_root: Option<PathBuf>,
    mobs: RwLock<BTreeMap<MobId, ManagedMob>>,
    /// Per-session locks for single-flight implicit mob creation.
    implicit_mob_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    restore_lock: Mutex<bool>,
    /// Shared realm-scoped profile store for cross-mob profile CRUD, together
    /// with its provenance (single source of truth for the persistent-root
    /// auto-upgrade decision).
    realm_profile_store_selection: RealmProfileStoreSelection,
    /// Skill sources seeded from the owning mob definition.
    ///
    /// Realm profiles carry skill names, not the source bodies/paths. When an
    /// agent creates a child mob from a realm profile, copy matching source
    /// definitions into the child so profile skills still assemble into the
    /// spawned member's system prompt.
    realm_skill_sources: BTreeMap<String, SkillSource>,
    /// Phase 6b (ADJ-P6B-16): the local-branch member live host passed
    /// through to `MobBuilder::with_member_live_host` for every mob this
    /// console creates/resumes. Late-settable (`set_member_live_host`)
    /// because live transports attach to the RPC router after the mob
    /// state is constructed. Absent ⇒ local `member_live_*` verbs reject
    /// typed `LiveTransportUnavailable` (honest degradation).
    member_live_host:
        std::sync::RwLock<Option<Arc<dyn meerkat_runtime::member_live::MemberLiveHost>>>,
    /// One host-process reverse-lane acceptor shared by every mob builder in
    /// this surface. Absent is honest fail-closed degradation for mixed-host
    /// edges; surfaces must supply an explicit dialable composition.
    controlling_acceptor: Option<meerkat_mob::ControllingAcceptorConfig>,
    /// The console principal this state serves (phase 5, chokepoint (b) —
    /// DEC-P5E-8). Injected at construction, never ambient (gotcha #19):
    /// every v1 local surface (RPC/REST/stdio/public-MCP/CLI/embedder)
    /// passes `Owner` (A16). v2 bearer auth threads per-connection
    /// `External` principals through this same seam — the deterministic
    /// non-owner test lanes already exercise it (ADJ-P5-10).
    console_principal: MobControlPrincipal,
}

impl MobMcpState {
    /// `console_principal` is REQUIRED (no default): the caller states which
    /// principal this console serves. Local single-user surfaces mint
    /// `MobControlPrincipal::Owner` (A16 — behavior byte-identical to
    /// pre-phase-5); an External principal makes every mob verb subject to its
    /// ControlScope grants.
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        console_principal: MobControlPrincipal,
    ) -> Self {
        let runtime_adapter = session_service.runtime_adapter();
        Self::new_with_runtime_adapter(session_service, runtime_adapter, console_principal)
    }

    pub fn new_with_runtime_adapter(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
        console_principal: MobControlPrincipal,
    ) -> Self {
        Self {
            console_principal,
            session_service,
            runtime_adapter,
            workgraph_service: None,
            default_llm_client: None,
            default_llm_client_provider: None,
            external_tools_provider: None,
            persistent_storage_root: None,
            mobs: RwLock::new(BTreeMap::new()),
            implicit_mob_locks: Mutex::new(HashMap::new()),
            restore_lock: Mutex::new(false),
            realm_profile_store_selection: RealmProfileStoreSelection::DefaultInMemory(Arc::new(
                meerkat_mob::InMemoryRealmProfileStore::new(),
            )),
            realm_skill_sources: BTreeMap::new(),
            member_live_host: std::sync::RwLock::new(None),
            controlling_acceptor: None,
        }
    }

    /// Install the process-scoped reverse-lane acceptor used by every mob
    /// created or resumed through this state.
    pub fn with_controlling_acceptor(
        mut self,
        config: meerkat_mob::ControllingAcceptorConfig,
    ) -> Self {
        self.controlling_acceptor = Some(config);
        self
    }

    /// Phase 6b (ADJ-P6B-16): install the local-branch member live host.
    /// Applies to mobs created/resumed AFTER the install (the live plane
    /// attaches before any mob verb is served on the RPC surface).
    pub fn set_member_live_host(
        &self,
        live_host: Arc<dyn meerkat_runtime::member_live::MemberLiveHost>,
    ) {
        *self
            .member_live_host
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(live_host);
    }

    /// Set the shared realm profile store for cross-mob profile CRUD.
    pub fn with_realm_profile_store(
        mut self,
        store: Option<Arc<dyn meerkat_mob::RealmProfileStore>>,
    ) -> Self {
        self.realm_profile_store_selection = RealmProfileStoreSelection::CallerSupplied(store);
        self
    }

    /// Returns a reference to the realm profile store, if configured.
    pub fn realm_profile_store(&self) -> Option<&Arc<dyn meerkat_mob::RealmProfileStore>> {
        self.realm_profile_store_selection.store()
    }

    pub fn with_persistent_storage_root(mut self, runtime_root: Option<PathBuf>) -> Self {
        self.persistent_storage_root = runtime_root.map(|root| {
            let mob_root = Self::persistent_mob_root(&root);
            // Auto-create realm profile store when persistent storage is available.
            #[cfg(not(target_arch = "wasm32"))]
            if self.realm_profile_store_selection.is_default() {
                let db_path = mob_root.join(Self::REALM_PROFILE_STORE_FILE_NAME);
                match meerkat_mob::SqliteRealmProfileStore::open(&db_path) {
                    Ok(store) => {
                        self.realm_profile_store_selection =
                            RealmProfileStoreSelection::DefaultDurable(
                                Arc::new(store) as Arc<dyn meerkat_mob::RealmProfileStore>
                            );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "failed to create realm profile store at {}: {e}",
                            db_path.display()
                        );
                    }
                }
            }
            mob_root
        });
        self
    }

    pub fn with_workgraph_service(mut self, service: Option<meerkat::WorkGraphService>) -> Self {
        self.workgraph_service = service;
        self
    }

    pub fn with_default_llm_client(mut self, client: Option<Arc<dyn LlmClient>>) -> Self {
        self.default_llm_client = client;
        self
    }

    pub fn with_default_llm_client_provider(
        mut self,
        provider: Option<DefaultLlmClientProvider>,
    ) -> Self {
        self.default_llm_client_provider = provider;
        self
    }

    pub fn with_external_tools_provider(
        mut self,
        provider: Option<meerkat_mob::ExternalToolsProvider>,
    ) -> Self {
        self.external_tools_provider = provider;
        self
    }

    /// Seed skill source definitions available to realm-referenced profiles.
    pub fn with_realm_skill_sources(mut self, sources: BTreeMap<String, SkillSource>) -> Self {
        self.realm_skill_sources = sources;
        self
    }

    async fn hydrate_definition_skill_sources(
        &self,
        definition: &mut MobDefinition,
    ) -> Result<(), MobError> {
        if self.realm_skill_sources.is_empty() {
            return Ok(());
        }

        let mut skill_names = BTreeSet::new();
        for binding in definition.profiles.values() {
            match binding {
                ProfileBinding::Inline(profile) => {
                    skill_names.extend(profile.skills.iter().cloned());
                }
                ProfileBinding::RealmRef { realm_profile } => {
                    let Some(store) = self.realm_profile_store_selection.store() else {
                        continue;
                    };
                    let stored = store.get(realm_profile).await.map_err(|error| {
                        MobError::Internal(format!(
                            "failed to load realm profile '{realm_profile}' while hydrating skill sources: {error}"
                        ))
                    })?;
                    if let Some(stored) = stored {
                        skill_names.extend(stored.profile.skills.iter().cloned());
                    }
                }
            }
        }

        for skill_name in skill_names {
            if definition.skills.contains_key(&skill_name) {
                continue;
            }
            if let Some(source) = self.realm_skill_sources.get(&skill_name) {
                definition.skills.insert(skill_name, source.clone());
            }
        }

        Ok(())
    }

    /// Reserved filename of the realm-scoped profile store inside the mob
    /// persistent root. This database lives in the same directory as the
    /// per-mob `*.db` files but is NOT a mob event log; the restore scan must
    /// never treat it as a mob storage candidate.
    #[cfg(not(target_arch = "wasm32"))]
    const REALM_PROFILE_STORE_FILE_NAME: &str = "realm_profiles.db";

    fn persistent_mob_root(runtime_root: &Path) -> PathBuf {
        runtime_root.join("mobs")
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn escape_mob_id_for_path(mob_id: &MobId) -> String {
        let mut escaped = String::with_capacity(mob_id.to_string().len() * 3);
        for byte in mob_id.to_string().bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
                escaped.push(char::from(byte));
            } else {
                escaped.push('%');
                escaped.push_str(&format!("{byte:02X}"));
            }
        }
        escaped
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn persistent_storage_path(root: &Path, mob_id: &MobId) -> PathBuf {
        root.join(format!("{}.db", Self::escape_mob_id_for_path(mob_id)))
    }

    fn default_llm_client_snapshot(&self) -> Option<Arc<dyn LlmClient>> {
        self.default_llm_client.clone().or_else(|| {
            self.default_llm_client_provider
                .as_ref()
                .and_then(|provider| provider())
        })
    }

    fn configure_builder(&self, mut builder: MobBuilder) -> MobBuilder {
        builder = builder
            .with_session_service(self.session_service.clone())
            .allow_ephemeral_sessions(!self.session_service.supports_persistent_sessions())
            .with_default_external_tools_provider(self.external_tools_provider.clone())
            .with_workgraph_service(self.workgraph_service.clone());
        if let Some(adapter) = &self.runtime_adapter {
            builder = builder.with_runtime_adapter(adapter.clone());
        }
        if let Some(client) = self.default_llm_client_snapshot() {
            builder = builder.with_default_llm_client(client);
        }
        if let Some(live_host) = self
            .member_live_host
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            builder = builder.with_member_live_host(live_host);
        }
        if let Some(acceptor) = &self.controlling_acceptor {
            builder = builder.with_controlling_acceptor(acceptor.clone());
        }
        builder
    }

    fn bind_realm_profile_store(&self, storage: MobStorage) -> MobStorage {
        storage.with_realm_profile_store(self.realm_profile_store_selection.store().cloned())
    }

    async fn storage_for_new_mob(
        &self,
        mob_id: &MobId,
    ) -> Result<(MobStorage, Option<PathBuf>), MobError> {
        #[cfg(target_arch = "wasm32")]
        let _ = mob_id;

        #[cfg(not(target_arch = "wasm32"))]
        if self.session_service.supports_persistent_sessions()
            && let Some(root) = &self.persistent_storage_root
        {
            tokio::fs::create_dir_all(root).await.map_err(|error| {
                MobError::Internal(format!(
                    "failed to create persistent mob root '{}': {error}",
                    root.display()
                ))
            })?;
            let path = Self::persistent_storage_path(root, mob_id);
            let storage = self.bind_realm_profile_store(MobStorage::persistent(&path)?);
            return Ok((storage, Some(path)));
        }

        Ok((self.bind_realm_profile_store(MobStorage::in_memory()), None))
    }

    /// Remove the mob's durable storage files. Fails CLOSED: a surviving
    /// db file after destroy is durable truth contradicting the reported
    /// terminal state, so the caller must surface the failure rather than
    /// report a clean destroy over a still-present store.
    async fn remove_storage_files(path: Option<&Path>) -> Result<(), MobError> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            Ok(())
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(path) = path else { return Ok(()) };
            let mut paths = Vec::with_capacity(3);
            paths.push(path.to_path_buf());
            for suffix in ["-wal", "-shm"] {
                let mut value = path.as_os_str().to_os_string();
                value.push(suffix);
                paths.push(PathBuf::from(value));
            }

            for path in &paths {
                Self::remove_one_storage_file(path).await?;
            }
            // Removal succeeded; verify the primary db path is actually
            // gone so a concurrent by-path re-open cannot silently
            // resurrect it behind a clean destroy report.
            match tokio::fs::metadata(&paths[0]).await {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(MobError::Internal(format!(
                    "mob storage '{}' post-remove verification failed: {error}",
                    paths[0].display()
                ))),
                Ok(_) => Err(MobError::Internal(format!(
                    "mob storage '{}' still exists after removal (recreated by a live handle?)",
                    paths[0].display()
                ))),
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn remove_one_storage_file(path: &Path) -> Result<(), MobError> {
        let mut last_error = None;
        let mut delay = Duration::from_millis(10);
        for attempt in 0..5 {
            match tokio::fs::remove_file(path).await {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => {
                    last_error = Some(error);
                    if attempt < 4 {
                        ::tokio::time::sleep(delay).await;
                        delay = delay.saturating_mul(2);
                    }
                }
            }
        }
        Err(MobError::Internal(format!(
            "failed to remove mob storage file '{}': {}",
            path.display(),
            last_error.map_or_else(|| "unknown error".to_string(), |error| error.to_string())
        )))
    }

    async fn restore_from_persistent_storage(&self) -> Result<(), MobError> {
        #[cfg(target_arch = "wasm32")]
        {
            Ok(())
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(root) = &self.persistent_storage_root else {
                return Ok(());
            };
            tokio::fs::create_dir_all(root).await.map_err(|error| {
                MobError::Internal(format!(
                    "failed to create persistent mob root '{}': {error}",
                    root.display()
                ))
            })?;

            let mut dir = tokio::fs::read_dir(root).await.map_err(|error| {
                MobError::Internal(format!(
                    "failed to read persistent mob root '{}': {error}",
                    root.display()
                ))
            })?;

            while let Some(entry) = dir.next_entry().await.map_err(|error| {
                MobError::Internal(format!(
                    "failed to iterate persistent mob root '{}': {error}",
                    root.display()
                ))
            })? {
                let path = entry.path();
                let file_type = entry.file_type().await.map_err(|error| {
                    MobError::Internal(format!(
                        "failed to inspect persistent mob entry '{}': {error}",
                        path.display()
                    ))
                })?;
                if !file_type.is_file()
                    || path.extension().and_then(|ext| ext.to_str()) != Some("db")
                {
                    continue;
                }
                // The realm-scoped profile store DB lives in this same
                // directory (see `with_persistent_storage_root`) but is not a
                // mob event log. Opening it as mob storage spawns a SQLite
                // event-bus watcher thread and then deletes the file because
                // its mob_events log is empty — silently destroying realm
                // profiles. Never treat it as a mob storage candidate.
                if path.file_name().and_then(|name| name.to_str())
                    == Some(Self::REALM_PROFILE_STORE_FILE_NAME)
                {
                    continue;
                }

                let storage = self.bind_realm_profile_store(MobStorage::persistent(&path)?);
                if storage.is_event_log_empty().await? {
                    if let Err(error) = Self::remove_storage_files(Some(path.as_path())).await {
                        tracing::error!(error = %error, "empty mob store sweep failed");
                    }
                    continue;
                }

                let handle = self
                    .configure_builder(MobBuilder::for_resume(storage))
                    .resume()
                    .await?;
                let mob_id = handle.definition().id.clone();
                match self.mobs.write().await.entry(mob_id.clone()) {
                    Entry::Vacant(entry) => {
                        entry.insert(ManagedMob {
                            handle,
                            storage_path: Some(path.clone()),
                        });
                    }
                    Entry::Occupied(_) => {
                        let _ = handle.destroy().await;
                        return Err(MobError::Internal(format!(
                            "duplicate persisted mob id during restore: {mob_id}"
                        )));
                    }
                }
            }

            let _ = self
                .scavenge_orphaned_bridge_session_scoped_mobs_inner()
                .await;
            Ok(())
        }
    }

    async fn ensure_restored(&self) -> Result<(), MobError> {
        if self.persistent_storage_root.is_none() {
            return Ok(());
        }
        let mut restored = self.restore_lock.lock().await;
        if *restored {
            return Ok(());
        }
        self.restore_from_persistent_storage().await?;
        *restored = true;
        Ok(())
    }

    async fn ensure_restored_best_effort(&self, action: &str) -> bool {
        match self.ensure_restored().await {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(action, error = %error, "mob state restore failed");
                false
            }
        }
    }

    /// Access the underlying session service.
    pub fn session_service(&self) -> Arc<dyn MobSessionService> {
        self.session_service.clone()
    }

    pub async fn mob_create_definition(
        &self,
        definition: MobDefinition,
    ) -> Result<MobId, MobError> {
        self.mob_create_definition_inner(definition, None).await
    }

    pub async fn mob_create_from_mobpack(
        &self,
        definition: MobDefinition,
        packed_skills: BTreeMap<String, Vec<u8>>,
        source_identity: meerkat_mob::MobDefinitionSourceIdentity,
    ) -> Result<MobId, MobError> {
        // (b) gate, ADJ-P5-11 (same class as mob_create_definition).
        self.require_console_owner(ControlScope::SendCommand)?;
        let mut definition = MobBuilder::lower_mobpack_definition(definition, &packed_skills)?;
        definition.source_identity = Some(source_identity);
        let mob_id = definition.id.clone();
        self.ensure_restored().await?;
        if let Some(existing) = self.mobs.read().await.get(&mob_id) {
            let existing_definition = existing.handle.definition();
            if existing_definition.source_identity == definition.source_identity
                && existing_definition == &definition
            {
                return Ok(mob_id);
            }
            return Err(MobError::Internal(format!(
                "duplicate mobpack create for mob id '{mob_id}' does not match existing verified pack identity"
            )));
        }
        let (storage, storage_path) = self.storage_for_new_mob(&mob_id).await?;
        let handle = self
            .configure_builder(MobBuilder::new(definition, storage))
            .create()
            .await?;
        match self.mobs.write().await.entry(mob_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(ManagedMob {
                    handle,
                    storage_path,
                });
                Ok(mob_id)
            }
            Entry::Occupied(_) => {
                if let Err(error) = handle.destroy().await {
                    tracing::warn!(
                        mob_id = %mob_id,
                        error = %error,
                        "duplicate mobpack create cleanup failed"
                    );
                }
                if let Err(error) = Self::remove_storage_files(storage_path.as_deref()).await {
                    tracing::error!(error = %error, "duplicate mobpack storage cleanup failed");
                }
                Err(MobError::Internal(format!(
                    "duplicate mobpack create raced for mob id '{mob_id}' before verified identity could be committed"
                )))
            }
        }
    }

    #[doc(hidden)]
    pub async fn mob_create_definition_with_owner_bridge_session(
        &self,
        definition: MobDefinition,
        owner_bridge_session_id: SessionId,
        destroy_on_owner_archive: bool,
        implicit_delegation_mob: bool,
    ) -> Result<MobId, MobError> {
        self.mob_create_definition_inner(
            definition,
            Some((
                owner_bridge_session_id,
                destroy_on_owner_archive,
                implicit_delegation_mob,
            )),
        )
        .await
    }

    async fn mob_create_definition_inner(
        &self,
        mut definition: MobDefinition,
        owner_bridge_session_authority: Option<(SessionId, bool, bool)>,
    ) -> Result<MobId, MobError> {
        // (b) gate, ADJ-P5-11: no per-mob grant can authorize creating a
        // mob that does not exist yet.
        self.require_console_owner(ControlScope::SendCommand)?;
        self.hydrate_definition_skill_sources(&mut definition)
            .await?;
        let mob_id = definition.id.clone();
        self.ensure_restored().await?;
        if self.mobs.read().await.contains_key(&mob_id) {
            return Err(MobError::Internal(format!("mob already exists: {mob_id}")));
        }
        let (storage, storage_path) = self.storage_for_new_mob(&mob_id).await?;
        let mut builder = self.configure_builder(MobBuilder::new(definition.clone(), storage));
        if let Some((owner_bridge_session_id, destroy_on_owner_archive, implicit_delegation_mob)) =
            owner_bridge_session_authority
        {
            builder = builder.with_owner_bridge_session_create_authority(
                owner_bridge_session_id,
                destroy_on_owner_archive,
                implicit_delegation_mob,
            );
        }
        let handle = builder.create().await?;
        match self.mobs.write().await.entry(mob_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(ManagedMob {
                    handle,
                    storage_path,
                });
            }
            Entry::Occupied(_) => {
                // Race-safe duplicate guard: avoid leaking the just-created runtime.
                if let Err(error) = handle.destroy().await {
                    tracing::warn!(
                        mob_id = %mob_id,
                        error = %error,
                        "duplicate mob create cleanup failed"
                    );
                }
                if let Err(error) = Self::remove_storage_files(storage_path.as_deref()).await {
                    tracing::error!(error = %error, "duplicate mob create storage cleanup failed");
                }
                return Err(MobError::Internal(format!("mob already exists: {mob_id}")));
            }
        }
        Ok(mob_id)
    }

    /// Register an existing mob handle in this dispatcher state.
    pub async fn mob_insert_handle(&self, mob_id: MobId, handle: MobHandle) {
        self.mobs.write().await.insert(
            mob_id,
            ManagedMob {
                handle,
                storage_path: None,
            },
        );
    }

    /// Return known mob handles without asking each mob actor for live status.
    ///
    /// Observation surfaces use this while child mobs are actively processing:
    /// a status query would queue behind the same actor work the UI is trying
    /// to observe.
    pub async fn mob_handles_snapshot(&self) -> Result<Vec<(MobId, MobHandle)>, MobError> {
        // (b) gate, ADJ-P5-11: cross-mob enumeration is Owner-only in v1 —
        // a per-mob List grant cannot authorize it (this is also mob_list's
        // gate; the raw-handle snapshot must not be a graded sibling).
        self.require_console_owner(ControlScope::List)?;
        self.ensure_restored().await?;
        Ok(self
            .mobs
            .read()
            .await
            .iter()
            .map(|(id, managed)| (id.clone(), managed.handle.clone()))
            .collect())
    }

    pub async fn mob_list(&self) -> Result<Vec<(MobId, MobState)>, MobError> {
        Ok(self
            .mob_handles_snapshot()
            .await?
            .into_iter()
            .map(|(id, handle)| (id, handle.status_observation_snapshot()))
            .collect())
    }

    pub async fn mob_status(&self, mob_id: &MobId) -> Result<MobState, MobError> {
        let handle = self.handle_for(mob_id).await?;
        // (b) resolver: QueryPhase carries a non-Result reply channel, so
        // its typed List deny lives here (FLAG-W-E-1 in scope_gate.rs).
        self.require_console_scope(&handle, ControlScope::List)?;
        handle.status().await
    }

    pub async fn mob_stop(&self, mob_id: &MobId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.stop().await
    }

    /// Rotate the supervisor bridge for the mob, returning the structured
    /// rotation report so RPC/MCP clients can inspect per-member outcomes
    /// instead of guessing from a bare success signal. Finding C10:
    /// `MobHandle::rotate_supervisor()` existed in the Rust API but had no
    /// operator-facing RPC surface.
    pub async fn mob_rotate_supervisor(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::SupervisorRotationReport, MobError> {
        self.handle_for(mob_id).await?.rotate_supervisor().await
    }

    pub async fn mob_resume(&self, mob_id: &MobId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.resume().await
    }

    pub async fn mob_complete(&self, mob_id: &MobId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.complete().await
    }

    pub async fn mob_reset(&self, mob_id: &MobId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.reset().await
    }

    pub async fn mob_lifecycle_action(
        &self,
        mob_id: &MobId,
        action: WireMobLifecycleAction,
    ) -> Result<Option<meerkat_mob::MobDestroyReport>, MobMcpDestroyError> {
        match action {
            WireMobLifecycleAction::Stop => {
                self.mob_stop(mob_id).await?;
                Ok(None)
            }
            WireMobLifecycleAction::Resume => {
                self.mob_resume(mob_id).await?;
                Ok(None)
            }
            WireMobLifecycleAction::Complete => {
                self.mob_complete(mob_id).await?;
                Ok(None)
            }
            WireMobLifecycleAction::Reset => {
                self.mob_reset(mob_id).await?;
                Ok(None)
            }
            WireMobLifecycleAction::Destroy => self.mob_destroy(mob_id).await.map(Some),
        }
    }

    /// Destroy a mob. Rejects implicit delegation mobs — use
    /// [`destroy_bridge_session_mobs`](Self::destroy_bridge_session_mobs) for
    /// bridge-session cleanup.
    ///
    /// Returns the structured [`meerkat_mob::MobDestroyReport`] only after
    /// canonical destroy completes. Partial cleanup is surfaced as
    /// [`MobMcpDestroyError::Incomplete`] with the report attached so callers
    /// can retry or inspect the still-retained mob authority.
    pub async fn mob_destroy(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError> {
        // The implicit-mob guard is a roster/lifecycle projection.  Admit the
        // destructive verb first so an ungranted principal cannot use the
        // guard to distinguish implicit from explicit mobs.
        self.admitted_handle_for(mob_id, ControlScope::Retire)
            .await?;
        if self.is_implicit_mob(mob_id).await {
            return Err(MobMcpDestroyError::Mob(MobError::Internal(
                "Cannot destroy implicit delegation mob directly. \
                 It is cleaned up automatically when the owning session is archived."
                    .to_string(),
            )));
        }
        self.mob_destroy_unchecked(mob_id).await
    }

    /// Destroy a mob without the implicit-mob guard.
    ///
    /// Used by session cleanup paths and canonical implicit-mob reconciliation.
    pub(crate) async fn mob_destroy_unchecked(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError> {
        self.ensure_restored().await?;
        self.mob_destroy_unchecked_loaded(mob_id).await
    }

    async fn mob_destroy_unchecked_loaded(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError> {
        let managed = {
            let mobs = self.mobs.read().await;
            mobs.get(mob_id)
                .cloned()
                .ok_or_else(|| MobMcpDestroyError::Mob(MobError::MobNotFound(mob_id.clone())))?
        };

        // Destroy rides the console-bound handle so chokepoint (a) sees the
        // console principal (the raw registry handle would bypass the gate).
        let bound = managed
            .handle
            .clone()
            .with_command_authority(CommandAuthority::principal(self.console_principal.clone()));
        match bound.destroy().await {
            Ok(report) => {
                let removed = self.mobs.write().await.remove(mob_id);
                let storage_path = removed
                    .as_ref()
                    .and_then(|managed| managed.storage_path.clone())
                    .or_else(|| managed.storage_path.clone());
                drop(removed);
                drop(managed);
                Self::remove_storage_files(storage_path.as_deref())
                    .await
                    .map_err(MobMcpDestroyError::Mob)?;
                Ok(report)
            }
            Err(meerkat_mob::MobDestroyError::Incomplete { report }) => {
                Err(MobMcpDestroyError::Incomplete { report })
            }
            Err(meerkat_mob::MobDestroyError::Mob(error)) => Err(MobMcpDestroyError::Mob(error)),
            Err(other) => {
                // MobDestroyError is #[non_exhaustive]; future variants we
                // haven't coded for fall through to a generic internal
                // error so the caller still gets a readable message.
                Err(MobMcpDestroyError::Mob(MobError::Internal(format!(
                    "mob destroy failed: {other}"
                ))))
            }
        }
    }

    pub async fn mob_spawn(
        &self,
        mob_id: &MobId,
        profile: ProfileName,
        identity: AgentIdentity,
        runtime_mode: Option<MobRuntimeMode>,
        backend: Option<MobBackendKind>,
        placement: Option<meerkat_mob::machines::mob_machine::HostId>,
    ) -> Result<meerkat_mob::SpawnResult, MobError> {
        let mut spec = SpawnMemberSpec::new(profile, identity);
        spec.runtime_mode = runtime_mode;
        spec.backend = backend;
        spec.placement = placement;
        self.mob_spawn_spec(mob_id, spec).await
    }

    pub async fn mob_spawn_spec(
        &self,
        mob_id: &MobId,
        spec: SpawnMemberSpec,
    ) -> Result<meerkat_mob::SpawnResult, MobError> {
        self.handle_for(mob_id).await?.spawn_spec(spec).await
    }

    pub async fn mob_spawn_many(
        &self,
        mob_id: &MobId,
        specs: Vec<SpawnMemberSpec>,
    ) -> Result<Vec<Result<meerkat_mob::SpawnResult, meerkat_mob::MobSpawnManyFailure>>, MobError>
    {
        // Batch authorization is one top-level decision.  Without this gate
        // an empty batch succeeds for an ungranted caller and non-empty scope
        // denials are laundered into per-item spawn failures.
        self.admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?
            .spawn_many(specs)
            .await
    }

    pub async fn mob_retire(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.retire(identity).await
    }

    #[doc(hidden)]
    pub async fn archive_mob_owned_bridge_session_with_cleanup(
        &self,
        bridge_session_id: &SessionId,
        cleanup_context: &'static str,
    ) -> Result<bool, SessionError> {
        let bridge_session_key = bridge_session_id.to_string();
        let mob_owned = self.owns_live_bridge_session(bridge_session_id).await
            || self
                .owns_service_reported_bridge_session(bridge_session_id)
                .await
            || self.owns_persisted_bridge_session(bridge_session_id).await;
        if !mob_owned {
            return Ok(false);
        }

        match self
            .retire_member_by_bridge_session_id(bridge_session_id)
            .await
        {
            Ok(()) => {
                self.destroy_bridge_session_mobs(&bridge_session_key)
                    .await
                    .map_err(|error| error.into_session_error(cleanup_context))?;
                Ok(true)
            }
            Err(MobError::BridgeSessionNotInLiveAuthority { .. }) => {
                if self
                    .has_bridge_session_scoped_mobs(&bridge_session_key)
                    .await
                {
                    self.destroy_bridge_session_mobs(&bridge_session_key)
                        .await
                        .map_err(|error| error.into_session_error(cleanup_context))?;
                    Ok(true)
                } else {
                    Err(SessionError::NotFound {
                        id: bridge_session_id.clone(),
                    })
                }
            }
            Err(error) => Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to archive mob-owned bridge session '{bridge_session_id}': {error}"
                )),
            )),
        }
    }

    #[doc(hidden)]
    pub async fn retire_member_by_bridge_session_id(
        &self,
        bridge_session_id: &SessionId,
    ) -> Result<(), MobError> {
        self.ensure_restored().await?;
        // Derive membership from authoritative live mob roster state rather than
        // maintaining a separate reverse index that can drift during retirement,
        // respawn, or policy-driven auto-spawn flows.
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        let mut resolved = None;
        for mob_id in mob_ids {
            let roster = self.handle_for(&mob_id).await?.roster().await;
            if let Some(entry) = roster.find_by_bridge_session_id(bridge_session_id) {
                resolved = Some((mob_id, entry.agent_identity.clone()));
                break;
            }
        }
        let Some((mob_id, identity)) = resolved else {
            if self
                .owns_service_reported_bridge_session(bridge_session_id)
                .await
                || self.owns_persisted_bridge_session(bridge_session_id).await
            {
                return self
                    .session_service()
                    .archive_with_mob_lifecycle_authority(bridge_session_id)
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to archive persisted mob-owned bridge session '{bridge_session_id}': {error}"
                        ))
                    });
            }
            return Err(MobError::BridgeSessionNotInLiveAuthority {
                bridge_session_id: bridge_session_id.to_string(),
            });
        };
        self.mob_retire(&mob_id, identity).await
    }

    #[doc(hidden)]
    pub async fn owns_live_bridge_session(&self, bridge_session_id: &SessionId) -> bool {
        if !self
            .ensure_restored_best_effort("owns_live_bridge_session")
            .await
        {
            return false;
        }
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        for mob_id in mob_ids {
            if let Ok(handle) = self.handle_for(&mob_id).await
                && handle.roster().await.has_bridge_session(bridge_session_id)
            {
                return true;
            }
        }
        false
    }

    #[doc(hidden)]
    pub async fn owns_service_reported_bridge_session(
        &self,
        bridge_session_id: &SessionId,
    ) -> bool {
        if !self
            .session_service()
            .has_live_session(bridge_session_id)
            .await
            .unwrap_or(false)
        {
            return false;
        }
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        for mob_id in mob_ids {
            if self
                .session_service()
                .session_belongs_to_mob(bridge_session_id, &mob_id)
                .await
            {
                return true;
            }
        }
        false
    }

    #[doc(hidden)]
    pub async fn owns_persisted_bridge_session(&self, bridge_session_id: &SessionId) -> bool {
        // Ownership routing needs only the typed identity facts on session
        // metadata — read the metadata-only seam, never the full document.
        let Some(view) = self
            .session_service()
            .load_persisted_session_metadata(bridge_session_id)
            .await
            .ok()
            .flatten()
        else {
            return false;
        };

        let Some(mob_id) = persisted_mob_binding(&view) else {
            return false;
        };
        match self.handle_for(&mob_id).await {
            Ok(handle) => handle.roster().await.has_bridge_session(bridge_session_id),
            Err(_) => {
                self.session_service()
                    .session_belongs_to_mob(bridge_session_id, &mob_id)
                    .await
            }
        }
    }

    pub async fn mob_wire(
        &self,
        mob_id: &MobId,
        local: AgentIdentity,
        target: meerkat_mob::PeerTarget,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.wire(local, target).await
    }

    pub async fn mob_wire_members_batch(
        &self,
        mob_id: &MobId,
        edges: Vec<(AgentIdentity, AgentIdentity)>,
    ) -> Result<meerkat_mob::MobWireMembersBatchReport, MobError> {
        self.handle_for(mob_id)
            .await?
            .wire_members_batch(edges)
            .await
    }

    pub async fn mob_unwire(
        &self,
        mob_id: &MobId,
        local: AgentIdentity,
        target: meerkat_mob::PeerTarget,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.unwire(local, target).await
    }

    pub async fn mob_list_members(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<meerkat_mob::runtime::MobMemberListEntry>, MobError> {
        let handle = self.handle_for(mob_id).await?;
        // (b) resolver: the member list is a watch-read projection that
        // never enters the actor queue (FLAG-W-E-1 in scope_gate.rs).
        self.require_console_scope(&handle, ControlScope::List)?;
        handle.list_members_including_retiring().await.pipe(Ok)
    }

    /// Declaratively ensure one member after a top-level SendCommand
    /// admission.  The underlying machine first projects spawn-vs-retain;
    /// without this wrapper the retain path never reaches a scoped command.
    pub async fn mob_ensure_member(
        &self,
        mob_id: &MobId,
        spec: SpawnMemberSpec,
    ) -> Result<meerkat_mob::runtime::EnsureMemberOutcome, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?
            .ensure_member(spec)
            .await
    }

    /// Reconcile one desired roster under its complete outer authority.
    /// Reconcile can spawn in every mode and can additionally retire when
    /// `retire_stale` is requested, so both admissions happen before the raw
    /// machine projection is evaluated.
    pub async fn mob_reconcile(
        &self,
        mob_id: &MobId,
        desired: Vec<SpawnMemberSpec>,
        options: meerkat_mob::runtime::ReconcileOptions,
    ) -> Result<meerkat_mob::runtime::ReconcileReport, MobError> {
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?;
        if options.retire_stale {
            handle.admit_control_scope(ControlScope::Retire).await?;
        }
        handle.reconcile(desired, options).await
    }

    /// Filter the operational member projection under List admission before
    /// the handle reads its actor-published watch state.
    pub async fn mob_list_members_matching(
        &self,
        mob_id: &MobId,
        filter: meerkat_mob::runtime::MemberFilter,
    ) -> Result<Vec<meerkat_mob::runtime::MobMemberListEntry>, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::List)
            .await?
            .list_members_matching(filter)
            .await
    }

    /// Resolve the member runtime mode and private bridge session for
    /// `mob/turn_start` only after SendCommand admission.
    pub async fn mob_turn_start_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<(Option<MobRuntimeMode>, Option<SessionId>), MobError> {
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?;
        let runtime_mode = handle
            .list_members()
            .await
            .into_iter()
            .find(|entry| &entry.agent_identity == identity)
            .map(|entry| entry.runtime_mode);
        let bridge_session_id = handle.resolve_bridge_session_id(identity).await;
        Ok((runtime_mode, bridge_session_id))
    }

    pub async fn mob_append_system_context(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        req: AppendSystemContextRequest,
    ) -> Result<(SessionId, AppendSystemContextResult), MobAppendSystemContextError> {
        // Session-control mechanics sit below a mob operator verb.  Enter the
        // serialized mob gate before resolving the member's private session
        // identity or touching its transcript.
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?;
        let bridge_session_id = handle
            .resolve_bridge_session_id(identity)
            .await
            .ok_or_else(|| {
                MobAppendSystemContextError::Session(SessionControlError::InvalidRequest {
                    message: format!("member has no session: {identity}"),
                })
            })?;
        self.wait_for_member_system_context_boundary(&bridge_session_id)
            .await
            .map_err(MobAppendSystemContextError::Session)?;
        let result = self
            .session_service()
            .append_system_context(&bridge_session_id, req)
            .await
            .map_err(MobAppendSystemContextError::Session)?;
        Ok((bridge_session_id, result))
    }

    async fn wait_for_member_system_context_boundary(
        &self,
        bridge_session_id: &SessionId,
    ) -> Result<(), SessionControlError> {
        let Some(adapter) = &self.runtime_adapter else {
            return Ok(());
        };
        if !adapter.contains_session(bridge_session_id).await {
            return Ok(());
        }

        let deadline = Instant::now() + Duration::from_mins(2);
        loop {
            if !adapter.contains_session(bridge_session_id).await {
                return Ok(());
            }
            let Some(snapshot) = adapter
                .meerkat_machine_spine_snapshot(bridge_session_id)
                .await
            else {
                return Ok(());
            };
            let active_boundary = snapshot.control.phase == meerkat_runtime::RuntimeState::Running
                || snapshot.control.current_run_id.is_some()
                || snapshot.inputs.current_run_id.is_some()
                || !snapshot.inputs.queue.is_empty()
                || !snapshot.inputs.steer_queue.is_empty();
            if !active_boundary {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(SessionControlError::Session(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "timed out waiting for member runtime boundary before appending system context for {bridge_session_id}: phase={:?}, control_run={:?}, ingress_run={:?}, queue_len={}, steer_queue_len={}",
                        snapshot.control.phase,
                        snapshot.control.current_run_id,
                        snapshot.inputs.current_run_id,
                        snapshot.inputs.queue.len(),
                        snapshot.inputs.steer_queue.len(),
                    )),
                )));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub async fn mob_resolve_bridge_session_id(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<Option<SessionId>, MobError> {
        Ok(self
            .handle_for(mob_id)
            .await?
            .resolve_bridge_session_id(identity)
            .await)
    }

    pub async fn mob_member_send(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        content: ContentInput,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<meerkat_mob::MemberDeliveryReceipt, MobError> {
        // MemberHandle acquisition reads the roster before SubmitWork reaches
        // its actor command.  Admit first so a missing member cannot become an
        // authorization oracle.
        self.admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?
            .member(&identity)
            .await?
            .send_with_render_metadata(content, handling_mode, render_metadata)
            .await
    }

    pub async fn mob_ingress_interaction(
        &self,
        mob_id: &MobId,
        spec: SpawnMemberSpec,
        content: ContentInput,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<MobIngressInteractionOutcome, MobError> {
        let identity = spec.identity.clone();
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?;
        let events_after_cursor = handle.events().latest_cursor().await?;
        let ensure_outcome = handle.ensure_member(spec).await?;
        let delivery = handle
            .member(&identity)
            .await?
            .send_with_render_metadata(content, handling_mode, render_metadata)
            .await?;
        let latest_event_cursor = handle.events().latest_cursor().await?;
        Ok(MobIngressInteractionOutcome {
            ensure_outcome,
            delivery,
            events_after_cursor,
            latest_event_cursor,
        })
    }

    pub async fn mob_events(
        &self,
        mob_id: &MobId,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<meerkat_mob::MobEvent>, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::SubscribeEvents)
            .await?
            .events()
            .poll(after_cursor, limit)
            .await
    }

    pub async fn mob_events_strict(
        &self,
        mob_id: &MobId,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<meerkat_mob::MobEvent>, MobError> {
        // Strict polling reads the latest cursor before its ordinary PollEvents
        // command.  Gate before that read so ScopeDenied wins over StaleCursor
        // and the destroyed-mob event-store fallback remains protected.
        self.admitted_handle_for(mob_id, ControlScope::SubscribeEvents)
            .await?
            .events()
            .poll_strict(after_cursor, limit)
            .await
    }

    pub async fn mob_latest_event_cursor(&self, mob_id: &MobId) -> Result<u64, MobError> {
        let handle = self.handle_for(mob_id).await?;
        // (b) resolver: the cursor read is an event-store read outside the
        // actor queue — same class as the subscription verbs.
        self.require_console_scope(&handle, ControlScope::SubscribeEvents)?;
        handle.events().latest_cursor().await
    }

    /// Submit a unit of work to a mob member through the work-lane.
    ///
    /// Thin wrapper over [`meerkat_mob::MobHandle::submit_work`] for the
    /// mob-surface crate. Finding C4 — the work-lane Rust API
    /// (`submit_work`/`cancel_work`/`cancel_all_work`) was Rust-only;
    /// this exposes it to RPC/HTTP consumers such as mobkit.
    pub async fn mob_submit_work(
        &self,
        mob_id: &MobId,
        runtime_id: meerkat_mob::AgentRuntimeId,
        fence_token: meerkat_mob::FenceToken,
        work_ref: meerkat_mob::WorkRef,
        spec: meerkat_mob::WorkSpec,
    ) -> Result<meerkat_mob::WorkDeliveryReceipt, MobError> {
        self.handle_for(mob_id)
            .await?
            .submit_work(runtime_id, fence_token, work_ref, spec)
            .await
    }

    /// Explicitly conclude the kickoff objective owned by a member.
    pub async fn mob_conclude_objective(
        &self,
        mob_id: &MobId,
        identity: &meerkat_mob::AgentIdentity,
        objective_id: meerkat_core::interaction::ObjectiveId,
        outcome: impl Into<String>,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id)
            .await?
            .conclude_objective(identity, objective_id, outcome)
            .await
    }

    pub async fn mob_bind_objective_owner(
        &self,
        mob_id: &MobId,
        owner_identity: meerkat_mob::AgentIdentity,
        objective_id: meerkat_core::interaction::ObjectiveId,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id)
            .await?
            .bind_objective_owner(owner_identity, objective_id)
            .await
    }

    fn objective_owner_identity_for_session(
        bridge_session_id: &SessionId,
    ) -> meerkat_mob::AgentIdentity {
        meerkat_mob::AgentIdentity::objective_lead_for_session(bridge_session_id)
    }

    pub async fn objective_principal_for_bridge_session(
        &self,
        bridge_session_id: &SessionId,
    ) -> Result<Option<(MobId, meerkat_mob::AgentIdentity)>, MobError> {
        self.ensure_restored().await?;
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        // A real roster binding is authoritative when the same session also
        // owns an implicit child mob: conclusions belong in the lead's mob.
        for mob_id in &mob_ids {
            let roster = self.handle_for(mob_id).await?.roster().await;
            if let Some(entry) = roster.find_by_bridge_session_id(bridge_session_id) {
                return Ok(Some((mob_id.clone(), entry.agent_identity.clone())));
            }
        }
        for mob_id in mob_ids {
            let handle = self.handle_for(&mob_id).await?;
            if handle.owns_bridge_session(bridge_session_id).await? {
                return Ok(Some((
                    mob_id,
                    Self::objective_owner_identity_for_session(bridge_session_id),
                )));
            }
        }
        Ok(None)
    }

    pub async fn objective_principal_for_mob_owner_session(
        &self,
        mob_id: &MobId,
        bridge_session_id: &SessionId,
    ) -> Result<meerkat_mob::AgentIdentity, MobError> {
        let handle = self.handle_for(mob_id).await?;
        let roster = handle.roster().await;
        if let Some(entry) = roster.find_by_bridge_session_id(bridge_session_id) {
            return Ok(entry.agent_identity.clone());
        }
        drop(roster);
        if handle.owns_bridge_session(bridge_session_id).await? {
            return Ok(Self::objective_owner_identity_for_session(
                bridge_session_id,
            ));
        }
        Err(MobError::Internal(format!(
            "bridge session {bridge_session_id} is not an objective lead for mob {mob_id}"
        )))
    }

    #[doc(hidden)]
    pub async fn member_for_bridge_session(
        &self,
        bridge_session_id: &SessionId,
    ) -> Result<Option<(MobId, meerkat_mob::AgentIdentity)>, MobError> {
        self.ensure_restored().await?;
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        for mob_id in mob_ids {
            let roster = self.handle_for(&mob_id).await?.roster().await;
            if let Some(entry) = roster.find_by_bridge_session_id(bridge_session_id) {
                return Ok(Some((mob_id, entry.agent_identity.clone())));
            }
        }
        Ok(None)
    }

    /// Resolve the current runtime binding for an opaque member reference
    /// under the scope of the operation that will consume it.
    ///
    /// This deliberately does not call `mob_list_members`: resolving a write
    /// target must not secretly require `List`, and a denied write must fail
    /// before revealing whether the member exists.
    async fn member_binding_after_admission(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        required: ControlScope,
    ) -> Result<(meerkat_mob::AgentRuntimeId, meerkat_mob::FenceToken), MobError> {
        let handle = self.admitted_handle_for(mob_id, required).await?;
        let entry = handle
            .list_members_including_retiring()
            .await
            .into_iter()
            .find(|entry| &entry.agent_identity == identity)
            .ok_or_else(|| MobError::MemberNotFound(identity.clone()))?;
        entry.binding_atoms().ok_or_else(|| {
            MobError::Internal(format!(
                "member {identity} has no MobMachine runtime binding"
            ))
        })
    }

    /// Resolve a member binding for a SendCommand-class operation without
    /// imposing the unrelated List scope.
    pub async fn mob_member_binding_for_send_command(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<(meerkat_mob::AgentRuntimeId, meerkat_mob::FenceToken), MobError> {
        self.member_binding_after_admission(mob_id, identity, ControlScope::SendCommand)
            .await
    }

    /// Resolve a member binding for a Cancel-class operation without
    /// imposing the unrelated List scope.
    pub async fn mob_member_binding_for_cancel(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<(meerkat_mob::AgentRuntimeId, meerkat_mob::FenceToken), MobError> {
        self.member_binding_after_admission(mob_id, identity, ControlScope::Cancel)
            .await
    }

    /// Cancel a previously submitted unit of work. Finding C4.
    pub async fn mob_cancel_work(
        &self,
        mob_id: &MobId,
        work_ref: meerkat_mob::WorkRef,
    ) -> Result<(), MobError> {
        // Per-item cancellation is intentionally unsupported, but the
        // authorization decision still precedes that honest capability error.
        self.admitted_handle_for(mob_id, ControlScope::Cancel)
            .await?
            .cancel_work(work_ref)
            .await
    }

    /// Cancel all in-flight work for a specific mob member. Finding C4.
    pub async fn mob_cancel_all_work(
        &self,
        mob_id: &MobId,
        runtime_id: meerkat_mob::AgentRuntimeId,
        fence_token: meerkat_mob::FenceToken,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id)
            .await?
            .cancel_all_work(runtime_id, fence_token)
            .await
    }

    pub async fn mob_list_flows(&self, mob_id: &MobId) -> Result<Vec<String>, MobError> {
        let flows = self
            .admitted_handle_for(mob_id, ControlScope::List)
            .await?
            .list_flows();
        Ok(flows
            .into_iter()
            .map(|flow_id| flow_id.to_string())
            .collect())
    }

    pub async fn mob_run_flow(
        &self,
        mob_id: &MobId,
        flow_id: FlowId,
        params: serde_json::Value,
    ) -> Result<RunId, MobError> {
        self.mob_run_flow_with_stream(mob_id, flow_id, params, None)
            .await
    }

    pub async fn mob_run_flow_with_stream(
        &self,
        mob_id: &MobId,
        flow_id: FlowId,
        params: serde_json::Value,
        scoped_event_tx: Option<mpsc::Sender<ScopedAgentEvent>>,
    ) -> Result<RunId, MobError> {
        // `run_flow_with_stream`'s preview command is the SendCommand-scoped
        // outer admission and runs before any flow-target provisioning read.
        self.handle_for(mob_id)
            .await?
            .run_flow_with_stream(flow_id, params, scoped_event_tx)
            .await
    }

    pub async fn mob_flow_status(
        &self,
        mob_id: &MobId,
        run_id: RunId,
    ) -> Result<Option<meerkat_mob::MobRun>, MobError> {
        self.handle_for(mob_id).await?.flow_status(run_id).await
    }

    pub async fn mob_list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&FlowId>,
    ) -> Result<Vec<meerkat_mob::MobRun>, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::List)
            .await?
            .list_runs(flow_id)
            .await
    }

    pub async fn mob_cancel_flow(&self, mob_id: &MobId, run_id: RunId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.cancel_flow(run_id).await
    }

    pub async fn mob_respawn(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        initial_message: Option<meerkat_core::types::ContentInput>,
    ) -> Result<meerkat_mob::MemberRespawnReceipt, meerkat_mob::MobRespawnError> {
        let handle = self.handle_for(mob_id).await?;
        handle.respawn(identity, initial_message).await
    }

    pub async fn mob_force_cancel(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id)
            .await?
            .force_cancel_member(identity)
            .await
    }

    pub async fn mob_member_status(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MobMemberSnapshot, MobError> {
        // The retiring-member fast path reads a machine watch and can return
        // before ProjectMemberStatus reaches the actor gate.  Admit first so
        // every lifecycle phase has identical denial precedence.
        self.admitted_handle_for(mob_id, ControlScope::List)
            .await?
            .member_status(identity)
            .await
    }

    pub async fn realtime_validate_session_target(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.session_service.read(session_id).await.map(|_| ())
    }

    pub async fn mob_wait_kickoff(
        &self,
        mob_id: &MobId,
        member_ids: Option<Vec<AgentIdentity>>,
        timeout_ms: Option<u64>,
    ) -> Result<Vec<KickoffMemberSnapshot>, MobError> {
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SubscribeEvents)
            .await?;
        // Admission is the authorization event for the composite wait.  Its
        // internal snapshot collection must not impose an unrelated `List`
        // grant after the wait has already been admitted.
        let admitted =
            handle.with_command_authority(CommandAuthority::principal(MobControlPrincipal::Owner));
        let timeout = timeout_ms.map(Duration::from_millis);
        let snapshots = match member_ids {
            Some(ids) => {
                admitted
                    .wait_for_members_kickoff_complete(&ids, timeout)
                    .await?
            }
            None => admitted.wait_for_kickoff_complete(timeout).await?,
        };
        Ok(snapshots
            .into_iter()
            .map(|(identity, snapshot)| KickoffMemberSnapshot {
                agent_identity: identity,
                snapshot,
            })
            .collect())
    }

    pub async fn mob_wait_ready(
        &self,
        mob_id: &MobId,
        member_ids: Option<Vec<AgentIdentity>>,
        timeout_ms: Option<u64>,
    ) -> Result<Vec<KickoffMemberSnapshot>, MobError> {
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SubscribeEvents)
            .await?;
        let admitted =
            handle.with_command_authority(CommandAuthority::principal(MobControlPrincipal::Owner));
        let timeout = timeout_ms.map(Duration::from_millis);
        let snapshots = match member_ids {
            Some(ids) => admitted.wait_for_members_ready(&ids, timeout).await?,
            None => admitted.wait_for_ready(timeout).await?,
        };
        Ok(snapshots
            .into_iter()
            .map(|(identity, snapshot)| KickoffMemberSnapshot {
                agent_identity: identity,
                snapshot,
            })
            .collect())
    }

    pub async fn mob_spawn_helper(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        prompt: String,
        options: meerkat_mob::HelperOptions,
    ) -> Result<meerkat_mob::HelperResult, MobError> {
        self.handle_for(mob_id)
            .await?
            .spawn_helper(identity, prompt, options)
            .await
    }

    pub async fn mob_fork_helper(
        &self,
        mob_id: &MobId,
        source_identity: &AgentIdentity,
        identity: AgentIdentity,
        prompt: String,
        fork_context: meerkat_mob::ForkContext,
        options: meerkat_mob::HelperOptions,
    ) -> Result<meerkat_mob::HelperResult, MobError> {
        self.handle_for(mob_id)
            .await?
            .fork_helper(source_identity, identity, prompt, fork_context, options)
            .await
    }

    /// Subscribe to mob-wide events (all members, continuously updated).
    ///
    /// (b) gate at subscription ADMISSION only: a later revoke/expiry does
    /// not tear down an already-open stream in v1 (ADJ-P5-17).
    pub async fn subscribe_mob_events(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobEventRouterHandle, MobError> {
        let handle = self.handle_for(mob_id).await?;
        self.require_console_scope(&handle, ControlScope::SubscribeEvents)?;
        handle.subscribe_mob_events().await
    }

    /// Subscribe to agent-level events for a specific member.
    ///
    /// (b) gate at subscription admission only (ADJ-P5-17).
    pub async fn subscribe_agent_events(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<meerkat_core::comms::EventStream, MobError> {
        let handle = self.handle_for(mob_id).await?;
        self.require_console_scope(&handle, ControlScope::SubscribeEvents)?;
        handle.subscribe_agent_events(identity).await
    }

    /// Find the implicit delegation mob for the given bridge session, if one exists.
    ///
    /// Scans the in-memory mob registry for a mob whose generated MobMachine
    /// owner-bridge authority marks it implicit and indexed to the given bridge
    /// session ID.
    /// Does NOT match explicit mobs that merely share the same owner.
    #[doc(hidden)]
    pub async fn find_implicit_mob_for_bridge_session(
        &self,
        bridge_session_id: &str,
    ) -> Option<MobId> {
        let bridge_session_id = SessionId::parse(bridge_session_id).ok()?;
        if !self
            .ensure_restored_best_effort("find_implicit_mob_for_bridge_session")
            .await
        {
            return None;
        }
        let mobs = self.mobs.read().await;
        mobs.iter()
            .find(|(_, m)| {
                m.handle
                    .owner_bridge_session_lifecycle_authority()
                    .is_some_and(|authority| {
                        authority.implicit_delegation_mob
                            && authority.bridge_session_id == bridge_session_id
                    })
            })
            .map(|(id, _)| id.clone())
    }

    /// Check whether the given mob is an implicit delegation mob.
    ///
    /// Checks generated MobMachine owner-bridge authority.
    #[doc(hidden)]
    pub async fn is_implicit_mob(&self, mob_id: &MobId) -> bool {
        if !self.ensure_restored_best_effort("is_implicit_mob").await {
            return false;
        }
        let mobs = self.mobs.read().await;
        mobs.get(mob_id)
            .and_then(|m| m.handle.owner_bridge_session_lifecycle_authority())
            .is_some_and(|authority| authority.implicit_delegation_mob)
    }

    /// Find all mobs indexed to the given owner bridge session
    /// (both implicit and explicit).
    #[doc(hidden)]
    pub async fn find_mobs_for_bridge_session(&self, bridge_session_id: &str) -> Vec<MobId> {
        let bridge_session_id = match SessionId::parse(bridge_session_id) {
            Ok(session_id) => session_id,
            Err(_) => return Vec::new(),
        };
        if !self
            .ensure_restored_best_effort("find_mobs_for_bridge_session")
            .await
        {
            return Vec::new();
        }
        let mobs = self.mobs.read().await;
        mobs.iter()
            .filter_map(|(id, m)| {
                if m.handle
                    .owner_bridge_session_lifecycle_authority()
                    .is_some_and(|authority| authority.bridge_session_id == bridge_session_id)
                {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    async fn find_bridge_session_scoped_mobs(&self, bridge_session_id: &str) -> Vec<MobId> {
        let bridge_session_id = match SessionId::parse(bridge_session_id) {
            Ok(session_id) => session_id,
            Err(_) => return Vec::new(),
        };
        if !self
            .ensure_restored_best_effort("find_bridge_session_scoped_mobs")
            .await
        {
            return Vec::new();
        }
        let mobs = self.mobs.read().await;
        mobs.iter()
            .filter_map(|(id, m)| {
                if m.handle
                    .owner_bridge_session_lifecycle_authority()
                    .is_some_and(|authority| {
                        authority.destroy_on_owner_archive
                            && authority.bridge_session_id == bridge_session_id
                    })
                {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    #[doc(hidden)]
    pub async fn has_bridge_session_scoped_mobs(&self, bridge_session_id: &str) -> bool {
        !self
            .find_bridge_session_scoped_mobs(bridge_session_id)
            .await
            .is_empty()
    }

    async fn implicit_mob_matches_session_model(
        &self,
        mob_id: &MobId,
        bridge_session_id: &str,
        model: &str,
    ) -> bool {
        let delegate_profile = ProfileName::from("delegate");
        let bridge_session_id = match SessionId::parse(bridge_session_id) {
            Ok(session_id) => session_id,
            Err(_) => return false,
        };
        let mobs = self.mobs.read().await;
        mobs.get(mob_id).is_some_and(|managed| {
            let definition = managed.handle.definition();
            managed
                .handle
                .owner_bridge_session_lifecycle_authority()
                .is_some_and(|authority| {
                    authority.implicit_delegation_mob
                        && authority.bridge_session_id == bridge_session_id
                })
                && definition
                    .resolve_inline_profile(&delegate_profile)
                    .is_some_and(|profile| profile.model == model)
        })
    }

    /// Ensure the canonical implicit delegation mob exists for the given
    /// owner bridge session/model pair.
    ///
    /// This is the sole owner of implicit-mob model reconciliation. Tool
    /// surfaces may provide a cached mob-id hint for the fast path, but they
    /// must not destroy or recreate mobs themselves.
    #[doc(hidden)]
    pub async fn ensure_implicit_mob_for_model(
        &self,
        bridge_session_id: &str,
        model: &str,
        cached_mob_id: Option<&MobId>,
    ) -> Result<(MobId, bool), MobError> {
        if let Some(cached_mob_id) = cached_mob_id
            && self
                .implicit_mob_matches_session_model(cached_mob_id, bridge_session_id, model)
                .await
        {
            return Ok((cached_mob_id.clone(), false));
        }

        // Get or create a per-owner-bridge-session lock
        let session_lock = {
            let mut locks = self.implicit_mob_locks.lock().await;
            locks
                .entry(bridge_session_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = session_lock.lock().await;

        if let Some(mob_id) = self
            .find_implicit_mob_for_bridge_session(bridge_session_id)
            .await
        {
            if self
                .implicit_mob_matches_session_model(&mob_id, bridge_session_id, model)
                .await
            {
                return Ok((mob_id, false));
            }
            self.mob_destroy_unchecked(&mob_id)
                .await
                .map_err(MobMcpDestroyError::into_mob_error)?;
        }

        let owner_bridge_session_id = SessionId::parse(bridge_session_id).map_err(|error| {
            MobError::Internal(format!(
                "invalid implicit mob owner bridge session id '{bridge_session_id}': {error}"
            ))
        })?;
        let mob_id = self
            .mob_create_definition_with_owner_bridge_session(
                MobDefinition::implicit(bridge_session_id, model),
                owner_bridge_session_id,
                true,
                true,
            )
            .await?;
        Ok((mob_id, true))
    }

    /// Get or create the implicit delegation mob for the given owner bridge
    /// session.
    ///
    /// Uses a per-bridge-session mutex to ensure single-flight creation: the first
    /// caller creates the mob, concurrent callers block and receive the same
    /// mob ID. The mob is indexed to the owning bridge session and marked
    /// bridge-session-scoped.
    #[doc(hidden)]
    pub async fn get_or_create_implicit_mob_for_bridge_session(
        &self,
        bridge_session_id: &str,
        model: &str,
    ) -> Result<MobId, MobError> {
        self.ensure_implicit_mob_for_model(bridge_session_id, model, None)
            .await
            .map(|(mob_id, _created)| mob_id)
    }

    /// Destroy all bridge-session-scoped mobs for the given owner bridge session.
    ///
    /// Called during session archive to clean up mobs whose cleanup truth is
    /// `DestroyOnOwnerArchive`. Owner indexing alone is not sufficient.
    #[doc(hidden)]
    pub async fn destroy_bridge_session_mobs(
        &self,
        bridge_session_id: &str,
    ) -> Result<(), MobMcpDestroyError> {
        self.ensure_restored().await?;
        let mob_ids = self
            .find_bridge_session_scoped_mobs(bridge_session_id)
            .await;
        if mob_ids.is_empty() {
            return Ok(());
        }
        let mut first_error = None;
        for mob_id in &mob_ids {
            if let Err(error) = self.mob_destroy_unchecked(mob_id).await {
                tracing::warn!(
                    mob_id = %mob_id,
                    bridge_session_id = %bridge_session_id,
                    error = %error,
                    "failed to destroy bridge-session-scoped mob during cleanup"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        // Prune the per-bridge-session lock to avoid unbounded growth in long-lived processes.
        let mut locks = self.implicit_mob_locks.lock().await;
        locks.remove(bridge_session_id);
        Ok(())
    }

    /// Scavenge orphaned bridge-session-scoped mobs whose owning sessions no
    /// longer exist.
    ///
    /// Called during hydration at startup. For each mob marked
    /// `DestroyOnOwnerArchive`, checks whether the indexed owning bridge session
    /// still exists. If not, destroys the mob. Returns the list of scavenged
    /// mob IDs.
    #[doc(hidden)]
    pub async fn scavenge_orphaned_bridge_session_scoped_mobs(&self) -> Vec<MobId> {
        if !self
            .ensure_restored_best_effort("scavenge_orphaned_bridge_session_scoped_mobs")
            .await
        {
            return Vec::new();
        }
        self.scavenge_orphaned_bridge_session_scoped_mobs_inner()
            .await
    }

    /// Compatibility wrapper for callers that still use session-centric naming.
    pub async fn scavenge_orphaned_session_scoped_mobs(&self) -> Vec<MobId> {
        self.scavenge_orphaned_bridge_session_scoped_mobs().await
    }

    async fn scavenge_orphaned_bridge_session_scoped_mobs_inner(&self) -> Vec<MobId> {
        // Collect bridge-session-scoped cleanup candidates under a read lock.
        let candidates: Vec<(MobId, SessionId)> = {
            let mobs = self.mobs.read().await;
            mobs.iter()
                .filter_map(|(id, m)| {
                    m.handle
                        .owner_bridge_session_lifecycle_authority()
                        .filter(|authority| authority.destroy_on_owner_archive)
                        .map(|authority| (id.clone(), authority.bridge_session_id))
                })
                .collect()
        };

        let mut scavenged = Vec::new();
        for (mob_id, bridge_session_id) in candidates {
            let is_orphan = match self.session_service.read(&bridge_session_id).await {
                Ok(_) => false,
                Err(SessionError::NotFound { .. }) => true,
                Err(_) => false, // Unknown error — don't scavenge
            };
            if is_orphan {
                if let Err(error) = self.mob_destroy_unchecked_loaded(&mob_id).await {
                    tracing::warn!(
                        mob_id = %mob_id,
                        bridge_session_id = %bridge_session_id,
                        error = %error,
                        "failed to scavenge orphaned bridge-session-scoped mob"
                    );
                } else {
                    tracing::info!(
                        mob_id = %mob_id,
                        bridge_session_id = %bridge_session_id,
                        "scavenged orphaned bridge-session-scoped mob"
                    );
                    scavenged.push(mob_id);
                    // Prune per-bridge-session lock to avoid unbounded growth.
                    let mut locks = self.implicit_mob_locks.lock().await;
                    locks.remove(&bridge_session_id.to_string());
                }
            }
        }
        scavenged
    }

    /// Look up the [`MobHandle`] for a given mob ID.
    ///
    /// Returns `MobError::MobNotFound` if the mob is not found.
    /// Every handle this console hands out is rebound to the console
    /// principal (chokepoint (b) principal minting, DEC-P5E-8): actor-routed
    /// verbs are then gated once, at chokepoint (a), against this principal.
    pub async fn handle_for(&self, mob_id: &MobId) -> Result<MobHandle, MobError> {
        self.ensure_restored().await?;
        self.mobs
            .read()
            .await
            .get(mob_id)
            .map(|m| {
                m.handle
                    .clone()
                    .with_command_authority(CommandAuthority::principal(
                        self.console_principal.clone(),
                    ))
            })
            .ok_or_else(|| MobError::MobNotFound(mob_id.clone()))
    }

    /// Resolve a console-bound handle and enter the actor-linearized scope
    /// gate before a composite surface performs any raw roster, machine-watch,
    /// event-store, or member-session lookup.
    ///
    /// This is intentionally the only state-level escape hatch for composite
    /// handlers.  Callers receive the same principal-bound handle after the
    /// admission; they do not receive Owner authority.
    async fn admitted_handle_for(
        &self,
        mob_id: &MobId,
        required: ControlScope,
    ) -> Result<MobHandle, MobError> {
        let handle = self.handle_for(mob_id).await?;
        handle.admit_control_scope(required).await?;
        Ok(handle)
    }

    /// Chokepoint (b) gate for verbs that never send a MobCommand
    /// (ADJ-P5-11): `mob_create` / `mob_list` are Owner-only in v1 — a
    /// per-mob grant cannot authorize creating a mob that does not exist
    /// yet, and a per-mob List grant cannot authorize cross-mob
    /// enumeration. Typed deny; `presented` is the caller's resolved set
    /// for the (nonexistent / cross-mob) target: the empty set.
    fn require_console_owner(&self, required: ControlScope) -> Result<(), MobError> {
        match &self.console_principal {
            MobControlPrincipal::Owner => Ok(()),
            _ => Err(MobError::ScopeDenied(ScopeDenial {
                required,
                presented: BTreeSet::new(),
            })),
        }
    }

    /// Chokepoint (b) resolver gate for per-mob verbs that bypass the actor
    /// (watch-read projections, event-router subscriptions — DEC-P5E-9).
    /// Owner short-circuits without a clock read (A16); external principals
    /// resolve against the mob's committed machine projection with ONE
    /// wall-clock read per decision. Revocation/expiry take effect at
    /// admission time only: an already-open stream survives a later revoke
    /// (ADJ-P5-17; teardown-on-revoke is a recorded v2 hardening).
    fn require_console_scope(
        &self,
        handle: &MobHandle,
        required: ControlScope,
    ) -> Result<(), MobError> {
        if matches!(self.console_principal, MobControlPrincipal::Owner) {
            return Ok(());
        }
        // Pre-epoch clock fails CLOSED (u64::MAX ⇒ every finite expiry is
        // expired) — a broken clock must never revive expired grants.
        let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(u64::MAX);
        handle
            .resolve_control_policy(now_ms)?
            .require(required)
            .map_err(MobError::from)
    }

    // ─── Control-scope grants (phase 5, §17.2 SD-3) ──────────────────
    //
    // The three wrappers do no scope checking themselves: grant/revoke are
    // gated at chokepoint (a) under AdminGrants, and the grants read
    // self-gates in its actor arm (owner implicit ⇒ zero v1 behavior
    // change).

    pub async fn mob_grant_scopes(
        &self,
        mob_id: &MobId,
        principal: meerkat_core::auth::PrincipalId,
        scopes: BTreeSet<ControlScope>,
        expires_at_ms: Option<u64>,
    ) -> Result<OperatorGrant, MobError> {
        self.handle_for(mob_id)
            .await?
            .grant_scopes(
                self.console_principal.clone(),
                principal,
                scopes,
                expires_at_ms,
            )
            .await
    }

    pub async fn mob_revoke_scopes(
        &self,
        mob_id: &MobId,
        principal: meerkat_core::auth::PrincipalId,
        scopes: Option<BTreeSet<ControlScope>>,
    ) -> Result<bool, MobError> {
        self.handle_for(mob_id)
            .await?
            .revoke_scopes(self.console_principal.clone(), principal, scopes)
            .await
    }

    pub async fn mob_grants(&self, mob_id: &MobId) -> Result<Vec<OperatorGrant>, MobError> {
        self.handle_for(mob_id)
            .await?
            .grants(self.console_principal.clone())
            .await
    }

    // ─── Phase-7 console verbs (§17, DEC-P7A-7 ratified table) ───────
    //
    // The ONE console assembly seam: RPC handlers, the public MCP
    // observation tools, and the CLI verbs all consume exactly these
    // methods (ADJ-P7-3). Actor-routed verbs are gated once at chokepoint
    // (a) against the console principal injected here; the two watch-read
    // projections (`mob_hosts`, `mob_route_installs`) bypass the actor and
    // take the existing chokepoint-(b) resolver gate under `List`.

    /// By-identity member transcript page (SD-1). THE single
    /// `MobMemberHistoryResult` envelope-assembly point (ADJ-P7-3):
    /// placement + provenance come verbatim from the actor's placement
    /// switch (the ADJ-P7-2 domain enrichment) — local pages are
    /// `placement: None` + `ControllingHostVerified`, bridge-served pages
    /// are `placement: Some(host)` + `HostClaimed`. No surface re-derives
    /// them. `ReadHistory`-gated at chokepoint (a).
    pub async fn mob_member_history(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        from_index: Option<u64>,
        limit: Option<u32>,
    ) -> Result<meerkat_contracts::wire::MobMemberHistoryResult, MobError> {
        let domain = self
            .handle_for(mob_id)
            .await?
            .member_history(self.console_principal.clone(), identity, from_index, limit)
            .await?;
        Ok(meerkat_contracts::wire::MobMemberHistoryResult {
            page: domain.page,
            generation: domain.generation,
            placement: domain
                .placement
                .map(|host| meerkat_contracts::wire::WireHostRef(host.as_str().to_string())),
            provenance: domain.provenance,
        })
    }

    /// Force-cancel's HARD sibling (DEC-P6E-8). `Cancel`-gated at
    /// chokepoint (a).
    pub async fn mob_hard_cancel_member(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        reason: String,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id)
            .await?
            .hard_cancel_member(self.console_principal.clone(), identity, reason)
            .await
    }

    /// Open a live realtime channel on a member (§16.4). The returned
    /// [`meerkat_contracts::wire::LiveOpenResult`] carries the owning
    /// host's URL + single-use token VERBATIM (DEC-P6B-C7/C8): never
    /// logged, Debug-formatted, or retained here. `Live`-gated at
    /// chokepoint (a).
    pub async fn mob_member_live_open(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        turning_mode: Option<meerkat_contracts::wire::RealtimeTurningMode>,
        transport: Option<meerkat_contracts::wire::LiveOpenTransport>,
    ) -> Result<meerkat_contracts::wire::LiveOpenResult, MobError> {
        self.handle_for(mob_id)
            .await?
            .member_live_open(
                self.console_principal.clone(),
                identity,
                turning_mode,
                transport,
            )
            .await
    }

    /// Close one NAMED live channel (close-what-you-name, DEC-P6B-C9).
    /// `Live`-gated at chokepoint (a).
    pub async fn mob_member_live_close(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        channel_id: String,
    ) -> Result<meerkat_contracts::wire::LiveCloseStatus, MobError> {
        self.handle_for(mob_id)
            .await?
            .member_live_close(self.console_principal.clone(), identity, channel_id)
            .await
    }

    /// Live point read; `channel_id: None` is the reply-loss discovery
    /// primitive (ADJ-P6B-2). `Live`-gated at chokepoint (a).
    pub async fn mob_member_live_status(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        channel_id: Option<String>,
    ) -> Result<meerkat_mob::MemberLiveStatusDomain, MobError> {
        self.handle_for(mob_id)
            .await?
            .member_live_status(self.console_principal.clone(), identity, channel_id)
            .await
    }

    /// Drive one DL10 live control verb (commit_input / interrupt /
    /// truncate / refresh — closed vocabulary). `Live`-gated at
    /// chokepoint (a).
    pub async fn mob_member_live_control(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        channel_id: String,
        verb: meerkat_contracts::wire::supervisor_bridge::BridgeLiveControlVerb,
    ) -> Result<meerkat_contracts::wire::supervisor_bridge::BridgeLiveControlOutcome, MobError>
    {
        self.handle_for(mob_id)
            .await?
            .member_live_control(self.console_principal.clone(), identity, channel_id, verb)
            .await
    }

    /// Bind a member-host daemon (§7.2 step 2). Ambient authority comes
    /// from the `handle_for` rebind; the actor arm gates `AdminHost` at
    /// chokepoint (a).
    pub async fn mob_bind_host(
        &self,
        mob_id: &MobId,
        request: meerkat_mob::HostBindRequest,
    ) -> Result<meerkat_mob::HostBindReport, MobError> {
        self.handle_for(mob_id).await?.bind_host(request).await
    }

    /// Revoke a bound (or bind-requested) member host. `AdminHost`-gated
    /// at chokepoint (a); the typed report names the released
    /// materializations (DEC-P7B-14 — never fabricated surface-side).
    pub async fn mob_revoke_host(
        &self,
        mob_id: &MobId,
        host_id: &str,
    ) -> Result<meerkat_mob::HostRevokeReport, MobError> {
        self.handle_for(mob_id).await?.revoke_host(host_id).await
    }

    /// Host roster projection (SD-5). Watch-read that bypasses the actor
    /// ⇒ chokepoint-(b) resolver gate under `List` (the ADJ-P5-12
    /// status-read classification extended to the phase-7 projections).
    pub async fn mob_hosts(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_contracts::wire::MobHostsResult, MobError> {
        let handle = self.handle_for(mob_id).await?;
        self.require_console_scope(&handle, ControlScope::List)?;
        handle.hosts()
    }

    /// Route-install obligation projection (ADJ-P4-9a). Watch-read ⇒
    /// chokepoint-(b) `List` gate. The drain (`drive_route_installs`)
    /// stays surface-less on EVERY surface (DEC-P7B-16).
    pub async fn mob_route_installs(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_contracts::wire::MobRouteInstallsResult, MobError> {
        let handle = self.handle_for(mob_id).await?;
        self.require_console_scope(&handle, ControlScope::List)?;
        handle.route_installs().await
    }

    // ─── Realm profile CRUD ──────────────────────────────────────────

    fn require_realm_profile_store(
        &self,
    ) -> Result<&Arc<dyn meerkat_mob::RealmProfileStore>, meerkat_mob::MobError> {
        self.realm_profile_store_selection.store().ok_or_else(|| {
            meerkat_mob::MobError::Internal("realm profile store not configured".to_string())
        })
    }

    /// Create a new realm profile.
    pub async fn realm_profile_create(
        &self,
        name: &str,
        profile: &meerkat_mob::Profile,
    ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobError> {
        // Realm-wide mutation has no target mob whose grants could authorize
        // it. Keep the v1 cross-mob posture identical to mob_create.
        self.require_console_owner(ControlScope::SendCommand)?;
        let store = self.require_realm_profile_store()?;
        store
            .create(name, profile)
            .await
            .map_err(|e| meerkat_mob::MobError::Internal(e.to_string()))
    }

    /// Get a realm profile by name.
    pub async fn realm_profile_get(
        &self,
        name: &str,
    ) -> Result<Option<meerkat_mob::StoredRealmProfile>, meerkat_mob::MobError> {
        self.require_console_owner(ControlScope::List)?;
        let store = self.require_realm_profile_store()?;
        store
            .get(name)
            .await
            .map_err(|e| meerkat_mob::MobError::Internal(e.to_string()))
    }

    /// List all realm profiles.
    pub async fn realm_profile_list(
        &self,
    ) -> Result<Vec<meerkat_mob::StoredRealmProfile>, meerkat_mob::MobError> {
        self.require_console_owner(ControlScope::List)?;
        let store = self.require_realm_profile_store()?;
        store
            .list()
            .await
            .map_err(|e| meerkat_mob::MobError::Internal(e.to_string()))
    }

    /// Update a realm profile with CAS revision.
    pub async fn realm_profile_update(
        &self,
        name: &str,
        profile: &meerkat_mob::Profile,
        expected_revision: u64,
    ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobError> {
        self.require_console_owner(ControlScope::SendCommand)?;
        let store = self.require_realm_profile_store()?;
        store
            .update(name, profile, expected_revision)
            .await
            .map_err(|e| meerkat_mob::MobError::Internal(e.to_string()))
    }

    /// Delete a realm profile with CAS revision.
    pub async fn realm_profile_delete(
        &self,
        name: &str,
        expected_revision: u64,
    ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobError> {
        self.require_console_owner(ControlScope::SendCommand)?;
        let store = self.require_realm_profile_store()?;
        store
            .delete(name, expected_revision)
            .await
            .map_err(|e| meerkat_mob::MobError::Internal(e.to_string()))
    }

    /// Create MCP state backed by an in-memory local session service.
    ///
    /// Mints `Owner` internally: the in-memory dev/test console is a local
    /// single-user surface (A16 posture, DEC-P5E-8).
    pub fn new_in_memory() -> Arc<Self> {
        Self::new_in_memory_as(MobControlPrincipal::Owner)
    }

    /// [`Self::new_in_memory`] serving an explicit console principal — the
    /// production principal-binding seam (ADJ-P5-10) with an in-memory
    /// session service. A `MobControlPrincipal::External` console makes
    /// every mob verb subject to that principal's ControlScope grants; this
    /// is the deterministic non-owner lane the scope-matrix rows drive and
    /// the byte-identical path v2 bearer auth lands on.
    pub fn new_in_memory_as(console_principal: MobControlPrincipal) -> Arc<Self> {
        let service = Arc::new(LocalSessionService::new());
        Arc::new(Self::new(service, console_principal))
    }
}

struct LocalCommsRuntime {
    name: String,
    peer_id: PeerId,
    public_key_bytes: [u8; 32],
    address: String,
    key: String,
    trusted: RwLock<HashMap<String, BTreeSet<GeneratedCommsTrustAuthoritySourceKind>>>,
    trusted_descriptors: RwLock<
        HashMap<String, HashMap<GeneratedCommsTrustAuthoritySourceKind, TrustedPeerDescriptor>>,
    >,
    private_trusted: RwLock<HashMap<String, BTreeSet<GeneratedCommsTrustAuthoritySourceKind>>>,
    meerkat_machine_trust_owner:
        std::sync::RwLock<Option<meerkat_core::comms::GeneratedPeerCommsOwnerToken>>,
    mob_machine_trust_owner: RwLock<Option<Arc<dyn std::any::Any + Send + Sync>>>,
    notify: Arc<tokio::sync::Notify>,
}

fn register_live_actor(
    registry: &meerkat_session::LiveSessionActorRegistry,
    slot: &meerkat_session::LiveSessionActorWitnessSlot,
    session_id: SessionId,
) -> Result<meerkat_session::LiveSessionActorWitness, SessionError> {
    let system_context_state = meerkat_core::SystemContextStateHandle::new(Default::default())
        .map_err(|error| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "live actor system-context authority rejected initialization: {error}"
            )))
        })?;
    registry.insert_and_publish(slot, session_id, system_context_state)
}

fn begin_live_actor_materialization(
    bindings: Option<&meerkat_core::SessionRuntimeBindings>,
) -> Result<Option<meerkat_runtime::RuntimeActorMaterializationPermit>, SessionError> {
    let Some(bindings) = bindings else {
        return Ok(None);
    };
    match meerkat_runtime::begin_session_runtime_actor_materialization(bindings) {
        Ok(permit) => Ok(Some(permit)),
        Err(meerkat_runtime::RuntimeActorMaterializationError::RegistrationClosed) => {
            Err(SessionError::NotFound {
                id: bindings.session_id().clone(),
            })
        }
        Err(meerkat_runtime::RuntimeActorMaterializationError::InvalidAuthority(reason)) => Err(
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(reason)),
        ),
    }
}

async fn commit_live_actor_materialization_or_discard<S>(
    service: &S,
    permit: Option<meerkat_runtime::RuntimeActorMaterializationPermit>,
    actor_witness: &meerkat_session::LiveSessionActorWitness,
) -> Result<(), SessionError>
where
    S: MobSessionService + ?Sized,
{
    let Some(permit) = permit else {
        return Ok(());
    };
    if let Err(error) = permit.commit() {
        let cleanup = service
            .discard_live_session_actor_under_runtime_turn_boundary(actor_witness)
            .await;
        return Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(match cleanup {
                Ok(_) => format!(
                    "runtime actor materialization commit failed for session {}: {error}",
                    actor_witness.session_id()
                ),
                Err(cleanup_error) => format!(
                    "runtime actor materialization commit failed for session {}: {error}; exact actor cleanup also failed: {cleanup_error}",
                    actor_witness.session_id()
                ),
            }),
        ));
    }
    Ok(())
}

fn encode_ed25519_public_key(bytes: &[u8; 32]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity("ed25519:".len() + 44);
    encoded.push_str("ed25519:");
    for chunk in bytes.chunks_exact(3) {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | chunk[2] as u32;
        encoded.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        encoded.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        encoded.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        encoded.push(TABLE[(n & 0x3f) as usize] as char);
    }
    let remainder = bytes.chunks_exact(3).remainder();
    if !remainder.is_empty() {
        let b0 = remainder[0];
        let b1 = remainder.get(1).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8);
        encoded.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        encoded.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if remainder.len() == 2 {
            encoded.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
            encoded.push('=');
        } else {
            encoded.push('=');
            encoded.push('=');
        }
    }
    encoded
}

impl LocalCommsRuntime {
    fn new(name: &str) -> Self {
        let mut public_key_bytes = [0u8; 32];
        for (index, byte) in name.bytes().enumerate() {
            let slot = index % public_key_bytes.len();
            public_key_bytes[slot] = public_key_bytes[slot]
                .wrapping_add(byte)
                .wrapping_add(index as u8);
        }
        if public_key_bytes == [0u8; 32] {
            public_key_bytes[0] = 1;
        }
        let peer_id = PeerId::from_ed25519_pubkey(&public_key_bytes);
        Self {
            name: name.to_string(),
            peer_id,
            public_key_bytes,
            address: format!("inproc://{name}"),
            key: encode_ed25519_public_key(&public_key_bytes),
            trusted: RwLock::new(HashMap::new()),
            trusted_descriptors: RwLock::new(HashMap::new()),
            private_trusted: RwLock::new(HashMap::new()),
            meerkat_machine_trust_owner: std::sync::RwLock::new(None),
            mob_machine_trust_owner: RwLock::new(None),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    async fn validate_generated_trust_authority_owner(
        &self,
        authority: &meerkat_core::comms::CommsTrustMutationAuthority,
    ) -> Result<(), SendError> {
        let expected_meerkat = self
            .meerkat_machine_trust_owner
            .read()
            .map_err(|_| {
                SendError::Validation("poisoned meerkat_machine_trust_owner lock".to_string())
            })?
            .clone();
        let expected_mob = self.mob_machine_trust_owner.read().await.clone();
        authority
            .validate_target_source_owner_token(expected_meerkat.as_ref(), expected_mob.as_ref())
            .map_err(SendError::Validation)
    }

    async fn add_generated_trust_source(
        &self,
        peer: TrustedPeerDescriptor,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
        private: bool,
    ) -> Result<bool, SendError> {
        let peer_id = peer.peer_id.as_str().to_string();
        let mut trusted = self.trusted.write().await;
        let mut descriptors = self.trusted_descriptors.write().await;
        let mut private_trusted = self.private_trusted.write().await;

        let source_exists = trusted
            .get(&peer_id)
            .is_some_and(|sources| sources.contains(&source_kind));
        let existing_descriptor = descriptors
            .get(&peer_id)
            .and_then(|by_source| by_source.get(&source_kind));
        let private_source_exists = private_trusted
            .get(&peer_id)
            .is_some_and(|sources| sources.contains(&source_kind));

        match (source_exists, existing_descriptor) {
            (true, Some(existing)) if existing == &peer && private_source_exists == private => {
                return Ok(false);
            }
            (false, None) if !private_source_exists => {}
            _ => {
                return Err(SendError::Validation(format!(
                    "generated trust source {source_kind:?} for {peer_id} already owns different trust material"
                )));
            }
        }

        descriptors
            .entry(peer_id.clone())
            .or_default()
            .insert(source_kind, peer);
        let created = trusted
            .entry(peer_id.clone())
            .or_default()
            .insert(source_kind);
        if private {
            private_trusted
                .entry(peer_id)
                .or_default()
                .insert(source_kind);
        }
        Ok(created)
    }

    async fn remove_generated_trust_source(
        &self,
        peer_id: &str,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> bool {
        let mut trusted = self.trusted.write().await;
        let mut descriptors = self.trusted_descriptors.write().await;
        let mut private_trusted = self.private_trusted.write().await;
        let removed = remove_trust_source(&mut trusted, peer_id, source_kind);
        if !removed {
            return false;
        }
        if let Some(by_source) = descriptors.get_mut(peer_id) {
            by_source.remove(&source_kind);
            if by_source.is_empty() {
                descriptors.remove(peer_id);
            }
        }
        if let Some(sources) = private_trusted.get_mut(peer_id) {
            sources.remove(&source_kind);
            if sources.is_empty() {
                private_trusted.remove(peer_id);
            }
        }
        true
    }
}

impl meerkat_core::handles::PeerCommsInstallTarget for LocalCommsRuntime {
    fn install_generated_peer_comms_handle(
        &self,
        install: meerkat_core::handles::GeneratedPeerCommsInstall,
    ) -> Result<(), String> {
        let target_peer_id = self.generated_peer_comms_target_endpoint()?.peer_id;
        if install.target_peer_id() != target_peer_id {
            return Err(format!(
                "generated peer-comms install targets peer_id {} but runtime peer_id is {}",
                install.target_peer_id(),
                target_peer_id
            ));
        }
        let owner_token = install.owner_token();
        let mut expected = self
            .meerkat_machine_trust_owner
            .write()
            .map_err(|_| "poisoned meerkat_machine_trust_owner lock".to_string())?;
        if let Some(existing) = expected.as_ref()
            && !existing.same_owner(&owner_token)
        {
            return Err(
                "target runtime is already bound to a different generated MeerkatMachine trust owner"
                    .to_string(),
            );
        }
        *expected = Some(owner_token);
        Ok(())
    }
}

fn remove_trust_source(
    trusted: &mut HashMap<String, BTreeSet<GeneratedCommsTrustAuthoritySourceKind>>,
    peer_id: &str,
    source_kind: GeneratedCommsTrustAuthoritySourceKind,
) -> bool {
    let Some(sources) = trusted.get_mut(peer_id) else {
        return false;
    };
    if !sources.remove(&source_kind) {
        return false;
    }
    if sources.is_empty() {
        trusted.remove(peer_id);
    }
    true
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreCommsRuntime for LocalCommsRuntime {
    fn peer_id(&self) -> Option<PeerId> {
        Some(self.peer_id)
    }

    fn public_key(&self) -> Option<String> {
        Some(self.key.clone())
    }

    fn public_key_bytes(&self) -> Option<[u8; 32]> {
        Some(self.public_key_bytes)
    }

    fn comms_name(&self) -> Option<String> {
        Some(self.name.clone())
    }

    fn advertised_address(&self) -> Option<String> {
        Some(self.address.clone())
    }

    async fn apply_trust_mutation(
        &self,
        mutation: CommsTrustMutation,
    ) -> Result<CommsTrustMutationResult, SendError> {
        match mutation {
            CommsTrustMutation::AddTrustedPeer { peer, authority } => {
                self.validate_generated_trust_authority_owner(&authority)
                    .await?;
                authority
                    .validate_public_add(self.peer_id(), &peer)
                    .map_err(SendError::Validation)?;
                TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
                    .map_err(SendError::Validation)?;
                let created = self
                    .add_generated_trust_source(peer, authority.trust_row_owner_kind(), false)
                    .await?;
                Ok(CommsTrustMutationResult::Added { created })
            }
            CommsTrustMutation::RemoveTrustedPeer { peer_id, authority } => {
                self.validate_generated_trust_authority_owner(&authority)
                    .await?;
                let parsed_peer_id = PeerId::parse(&peer_id)
                    .map_err(|err| SendError::Validation(err.to_string()))?;
                authority
                    .validate_public_remove(self.peer_id(), parsed_peer_id)
                    .map_err(SendError::Validation)?;
                let removed = self
                    .remove_generated_trust_source(&peer_id, authority.trust_row_owner_kind())
                    .await;
                Ok(CommsTrustMutationResult::Removed { removed })
            }
            CommsTrustMutation::AddPrivateTrustedPeer { peer, authority } => {
                self.validate_generated_trust_authority_owner(&authority)
                    .await?;
                authority
                    .validate_private_add(self.peer_id(), &peer)
                    .map_err(SendError::Validation)?;
                TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
                    .map_err(SendError::Validation)?;
                let created = self
                    .add_generated_trust_source(peer, authority.trust_row_owner_kind(), true)
                    .await?;
                Ok(CommsTrustMutationResult::Added { created })
            }
            CommsTrustMutation::RemovePrivateTrustedPeer { peer_id, authority } => {
                self.validate_generated_trust_authority_owner(&authority)
                    .await?;
                let parsed_peer_id = PeerId::parse(&peer_id)
                    .map_err(|err| SendError::Validation(err.to_string()))?;
                authority
                    .validate_private_remove(self.peer_id(), parsed_peer_id)
                    .map_err(SendError::Validation)?;
                let removed = self
                    .remove_generated_trust_source(&peer_id, authority.trust_row_owner_kind())
                    .await;
                Ok(CommsTrustMutationResult::Removed { removed })
            }
        }
    }

    async fn trusted_peer_projection_snapshot_for_source(
        &self,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> Result<Vec<TrustedPeerDescriptor>, CommsCapabilityError> {
        let descriptors = self.trusted_descriptors.read().await;
        let private_trusted = self.private_trusted.read().await;
        let mut peers = descriptors
            .iter()
            .filter_map(|(peer_id, by_source)| {
                if private_trusted
                    .get(peer_id)
                    .is_some_and(|sources| sources.contains(&source_kind))
                {
                    return None;
                }
                by_source.get(&source_kind).cloned()
            })
            .collect::<Vec<_>>();
        peers.sort_by(|left, right| left.peer_id.as_str().cmp(&right.peer_id.as_str()));
        Ok(peers)
    }

    async fn install_generated_mob_trust_owner(
        &self,
        owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        let mut expected = self.mob_machine_trust_owner.write().await;
        if let Some(existing) = expected.as_ref() {
            if Arc::ptr_eq(existing, &owner) {
                return Ok(());
            }
            return Err(SendError::Validation(
                "target runtime is already bound to a different generated MobMachine trust owner"
                    .to_string(),
            ));
        }
        *expected = Some(owner);
        Ok(())
    }

    async fn validate_recovered_generated_mob_trust_owner(
        &self,
        owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        let expected = self.mob_machine_trust_owner.read().await;
        if let Some(existing) = expected.as_ref()
            && !Arc::ptr_eq(existing, &owner)
        {
            return Err(SendError::Validation(
                "target runtime is already bound to a different generated MobMachine trust owner"
                    .to_string(),
            ));
        }
        Ok(())
    }

    async fn install_recovered_generated_mob_trust_owner(
        &self,
        owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        let mut expected = self.mob_machine_trust_owner.write().await;
        if let Some(existing) = expected.as_ref() {
            if Arc::ptr_eq(existing, &owner) {
                return Ok(());
            }
            return Err(SendError::Validation(
                "target runtime is already bound to a different generated MobMachine trust owner"
                    .to_string(),
            ));
        }
        *expected = Some(owner);
        Ok(())
    }

    async fn add_private_trusted_peer(
        &self,
        _peer: TrustedPeerDescriptor,
    ) -> Result<(), SendError> {
        Err(SendError::Unsupported(
            "add_private_trusted_peer requires apply_trust_mutation authority".to_string(),
        ))
    }

    async fn send(&self, _cmd: CommsCommand) -> Result<SendReceipt, SendError> {
        Ok(SendReceipt::InputAccepted {
            interaction_id: InteractionId(uuid::Uuid::nil()),
            stream_reserved: false,
        })
    }

    async fn drain_messages(&self) -> Vec<String> {
        Vec::new()
    }

    fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        self.notify.clone()
    }

    async fn drain_peer_input_candidates(&self) -> Vec<PeerInputCandidate> {
        Vec::new()
    }
}

struct LocalSessionActor {
    comms: Arc<LocalCommsRuntime>,
    witness: meerkat_session::LiveSessionActorWitness,
}

struct LocalSessionService {
    sessions: RwLock<HashMap<SessionId, LocalSessionActor>>,
    actor_registry: meerkat_session::LiveSessionActorRegistry,
    archived_views: RwLock<HashMap<SessionId, SessionView>>,
    pending_context: RwLock<HashMap<SessionId, Vec<AppendSystemContextRequest>>>,
    /// Per-session broadcast channels for event streaming.
    event_txs:
        RwLock<HashMap<SessionId, tokio::sync::broadcast::Sender<EventEnvelope<AgentEvent>>>>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    counter: std::sync::atomic::AtomicU64,
    archive_delay_ms: std::sync::atomic::AtomicU64,
    archive_failures: Arc<InMemoryArchiveFailureControl>,
}

impl LocalSessionService {
    fn new() -> Self {
        Self::new_with_archive_failures(Arc::new(InMemoryArchiveFailureControl::default()))
    }

    fn new_with_archive_failures(archive_failures: Arc<InMemoryArchiveFailureControl>) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            actor_registry: meerkat_session::LiveSessionActorRegistry::default(),
            archived_views: RwLock::new(HashMap::new()),
            pending_context: RwLock::new(HashMap::new()),
            event_txs: RwLock::new(HashMap::new()),
            runtime_adapter: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
            counter: std::sync::atomic::AtomicU64::new(0),
            archive_delay_ms: std::sync::atomic::AtomicU64::new(0),
            archive_failures,
        }
    }

    fn set_archive_delay_ms(&self, delay_ms: u64) {
        self.archive_delay_ms
            .store(delay_ms, std::sync::atomic::Ordering::Relaxed);
    }

    async fn retire_with_machine_archive_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        if !self.sessions.read().await.contains_key(session_id)
            && !self.runtime_adapter.contains_session(session_id).await
        {
            return Ok(());
        }

        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
        match meerkat_runtime::RuntimeControlPlane::retire(&*self.runtime_adapter, &runtime_id)
            .await
        {
            Ok(_) => Ok(()),
            Err(meerkat_runtime::RuntimeControlPlaneError::NotFound(_)) => {
                self.runtime_adapter
                    .register_session(session_id.clone())
                    .await
                    .map_err(|error| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("machine archive register before retire failed: {error}"),
                        ))
                    })?;
                meerkat_runtime::RuntimeControlPlane::retire(&*self.runtime_adapter, &runtime_id)
                    .await
                    .map(|_| ())
                    .map_err(|error| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("machine archive retire failed after registration: {error}"),
                        ))
                    })
            }
            Err(error) => Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "machine archive retire failed: {error}"
                )),
            )),
        }
    }

    async fn create_session_with_actor_slot(
        &self,
        req: CreateSessionRequest,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<RunResult, SessionError> {
        let build = req.build;
        let sid = build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            .map(|session| session.id().clone())
            .unwrap_or_default();
        let n = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = build
            .as_ref()
            .and_then(|build| build.comms_name.clone())
            .unwrap_or_else(|| format!("session-{n}"));
        let provided_bindings = match build.as_ref().map(|build| &build.runtime_build_mode) {
            Some(meerkat_core::RuntimeBuildMode::SessionOwned(bindings)) => {
                if bindings.session_id() != &sid {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "machine-prepared session bindings for {} do not match created session {}",
                            bindings.session_id(),
                            sid
                        )),
                    ));
                }
                Some(bindings.clone())
            }
            Some(meerkat_core::RuntimeBuildMode::StandaloneEphemeral) | None => None,
        };
        self.runtime_adapter
            .register_session(sid.clone())
            .await
            .map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "machine register session failed: {error}"
                )))
            })?;
        let machine_prepared = provided_bindings.is_some();
        let bindings = match provided_bindings {
            Some(bindings) => bindings,
            None => self
                .runtime_adapter
                .prepare_bindings(sid.clone())
                .await
                .map_err(|error| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "machine prepare bindings failed: {error}"
                    )))
                })?,
        };
        let actor_materialization_permit =
            begin_live_actor_materialization(machine_prepared.then_some(&bindings))?;
        let comms = Arc::new(LocalCommsRuntime::new(&name));
        bindings
            .install_peer_comms_on(comms.as_ref())
            .map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "machine peer-comms install failed: {error}"
                )))
            })?;

        let actor_witness = {
            let mut sessions = self.sessions.write().await;
            if sessions.contains_key(&sid) {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "live session actor is already registered: {sid}"
                    )),
                ));
            }
            let witness =
                register_live_actor(&self.actor_registry, actor_witness_slot, sid.clone())?;
            sessions.insert(
                sid.clone(),
                LocalSessionActor {
                    comms,
                    witness: witness.clone(),
                },
            );
            witness
        };
        commit_live_actor_materialization_or_discard(
            self,
            actor_materialization_permit,
            &actor_witness,
        )
        .await?;
        self.pending_context
            .write()
            .await
            .insert(sid.clone(), Vec::new());
        let (tx, _) = tokio::sync::broadcast::channel::<EventEnvelope<AgentEvent>>(256);
        self.event_txs.write().await.insert(sid.clone(), tx);
        Ok(RunResult {
            text: "ok".to_string(),
            session_id: sid,
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionService for LocalSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let actor_witness_slot = meerkat_session::LiveSessionActorWitnessSlot::default();
        self.create_session_with_actor_slot(req, &actor_witness_slot)
            .await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        // Drain any staged system context so the append_system_context contract
        // is honored (staged context is consumed on the next turn).
        let staged_context = {
            let mut pending = self.pending_context.write().await;
            let entry = pending.entry(id.clone()).or_default();
            std::mem::take(entry)
        };
        let effective_prompt = if staged_context.is_empty() {
            req.prompt.clone()
        } else {
            let staged_sections = staged_context
                .iter()
                .map(|append| match append.source.as_deref() {
                    Some(source) => format!("[SYSTEM CONTEXT:{source}] {}", append.text()),
                    None => format!("[SYSTEM CONTEXT] {}", append.text()),
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut blocks = vec![meerkat_core::types::ContentBlock::Text {
                text: format!("{staged_sections}\n\n"),
            }];
            blocks.extend(req.prompt.clone().into_blocks());
            meerkat_core::types::ContentInput::Blocks(blocks)
        };

        let event_tx = self.event_txs.read().await.get(id).cloned();
        let next_seq = |seq: &mut u64| {
            let current = *seq;
            *seq += 1;
            current
        };
        if let Some(event_tx) = event_tx {
            let mut seq = 1u64;
            let _ = event_tx.send(EventEnvelope::new_session(
                id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::RunStarted {
                    session_id: id.clone(),
                    input: meerkat_core::types::RunInput::Content {
                        content: effective_prompt.clone(),
                    },
                },
            ));
            let _ = event_tx.send(EventEnvelope::new_session(
                id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::TurnStarted { turn_number: 1 },
            ));
            let usage = Usage::default();
            let turn_usage = usage.clone();
            let _ = event_tx.send(EventEnvelope::new_session(
                id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::types::StopReason::EndTurn,
                    usage: turn_usage,
                },
            ));
            let _ = event_tx.send(EventEnvelope::new_session(
                id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::RunCompleted {
                    session_id: id.clone(),
                    result: "ok".to_string(),
                    structured_output: None,
                    extraction_required: false,
                    usage,
                    terminal_cause_kind: None,
                },
            ));
        }
        Ok(RunResult {
            text: "ok".to_string(),
            session_id: id.clone(),
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        })
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(())
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        Ok(self.sessions.read().await.contains_key(id))
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        if !self.sessions.read().await.contains_key(id) {
            return self
                .archived_views
                .read()
                .await
                .get(id)
                .cloned()
                .ok_or_else(|| SessionError::NotFound { id: id.clone() });
        }
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                is_active: false,
                model: "claude-sonnet-4-5".to_string(),
                provider: Provider::Anthropic,
                last_assistant_text: None,
                labels: Default::default(),
            },
            billing: SessionUsage {
                total_tokens: 0,
                usage: Usage::default(),
            },
        })
    }

    async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        Ok(self
            .sessions
            .read()
            .await
            .keys()
            .map(|id| SessionSummary {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                is_active: false,
                labels: Default::default(),
            })
            .collect())
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        if let Some(reason) = self.archive_failures.failure_for(id).await {
            return Err(SessionError::Unsupported(reason));
        }
        let archive_delay_ms = self
            .archive_delay_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        if archive_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(archive_delay_ms)).await;
        }
        let removed = {
            let mut sessions = self.sessions.write().await;
            if sessions.contains_key(id) && !self.actor_registry.remove_current(id) {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "live actor registry omitted current session {id} during archive"
                    )),
                ));
            }
            sessions.remove(id)
        };
        self.pending_context.write().await.remove(id);
        self.event_txs.write().await.remove(id);
        if removed.is_some() {
            self.archived_views.write().await.insert(
                id.clone(),
                SessionView {
                    state: SessionInfo {
                        session_id: id.clone(),
                        created_at: SystemTime::now(),
                        updated_at: SystemTime::now(),
                        message_count: 0,
                        is_active: false,
                        model: "claude-sonnet-4-5".to_string(),
                        provider: Provider::Anthropic,
                        last_assistant_text: None,
                        labels: Default::default(),
                    },
                    billing: SessionUsage {
                        total_tokens: 0,
                        usage: Usage::default(),
                    },
                },
            );
            Ok(())
        } else {
            Err(SessionError::NotFound { id: id.clone() })
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionServiceCommsExt for LocalSessionService {
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .map(|actor| actor.comms.clone() as Arc<dyn CoreCommsRuntime>)
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        let sessions = self.sessions.read().await;
        let actor = sessions.get(session_id)?;
        actor.comms.event_injector()
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        let sessions = self.sessions.read().await;
        let actor = sessions.get(session_id)?;
        actor.comms.interaction_event_injector()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionServiceControlExt for LocalSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() }.into());
        }
        let mut pending = self.pending_context.write().await;
        let entry = pending.entry(id.clone()).or_default();
        if let Some(key) = req.idempotency_key.as_deref()
            && entry
                .iter()
                .any(|existing| existing.idempotency_key.as_deref() == Some(key))
        {
            return Ok(AppendSystemContextResult {
                status: AppendSystemContextStatus::Staged,
            });
        }
        entry.push(req);
        Ok(AppendSystemContextResult {
            status: AppendSystemContextStatus::Staged,
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionServiceHistoryExt for LocalSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        if self.sessions.read().await.contains_key(id)
            || self.archived_views.read().await.contains_key(id)
        {
            return Ok(SessionHistoryPage::from_messages(id.clone(), &[], query));
        }
        Err(SessionError::NotFound { id: id.clone() })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobSessionService for LocalSessionService {
    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: CreateSessionRequest,
    ) -> Result<RunResult, SessionError> {
        <Self as SessionService>::create_session(self, req).await
    }

    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        req: CreateSessionRequest,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<RunResult, SessionError> {
        self.create_session_with_actor_slot(req, actor_witness_slot)
            .await
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.archive_with_mob_lifecycle_authority(session_id).await
    }

    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.discard_live_session(session_id).await
    }

    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        let removed = {
            let mut sessions = self.sessions.write().await;
            let Some(actor) = sessions.get(witness.session_id()) else {
                return Ok(false);
            };
            if !actor.witness.eq(witness) {
                return Ok(false);
            }
            if !self.actor_registry.compare_remove(witness) {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "live actor registry rejected current witness for {}",
                        witness.session_id()
                    )),
                ));
            }
            sessions.remove(witness.session_id()).is_some()
        };
        if removed {
            self.pending_context
                .write()
                .await
                .remove(witness.session_id());
            self.event_txs.write().await.remove(witness.session_id());
        }
        Ok(removed)
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        let txs = self.event_txs.read().await;
        let tx = txs
            .get(session_id)
            .ok_or_else(|| StreamError::NotFound(format!("session {session_id}")))?;
        let rx = tx.subscribe();
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            loop {
                match rx.recv().await {
                    Ok(event) => return Some((event, rx)),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        })))
    }

    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    async fn live_session_actor_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        Ok(self.actor_registry.contains(session_id))
    }

    fn runtime_adapter(&self) -> Option<std::sync::Arc<meerkat_runtime::MeerkatMachine>> {
        Some(Arc::clone(&self.runtime_adapter))
    }

    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.retire_with_machine_archive_authority(session_id)
            .await?;
        SessionService::archive(self, session_id).await
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        {
            let mut sessions = self.sessions.write().await;
            if sessions.contains_key(session_id) && !self.actor_registry.remove_current(session_id)
            {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "live actor registry omitted current session {session_id} during discard"
                    )),
                ));
            }
            sessions.remove(session_id);
        }
        self.pending_context.write().await.remove(session_id);
        self.event_txs.write().await.remove(session_id);
        Ok(())
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        req: meerkat_core::service::StartTurnRequest,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
        let run_result = <Self as SessionService>::start_turn(self, session_id, req).await?;
        Ok(
            meerkat_core::lifecycle::core_executor::CoreApplyOutput::with_run_result(
                meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft {
                    run_id,
                    boundary,
                    contributing_input_ids,
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                run_result,
            ),
        )
    }

    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        true
    }
}

impl MobMcpState {
    pub fn new_in_memory_with_archive_delay(delay_ms: u64) -> Arc<Self> {
        let session_service = Arc::new(LocalSessionService::new());
        session_service.set_archive_delay_ms(delay_ms);
        Arc::new(Self::new(session_service, MobControlPrincipal::Owner))
    }

    #[doc(hidden)]
    pub fn new_in_memory_with_archive_failure_control()
    -> (Arc<Self>, Arc<InMemoryArchiveFailureControl>) {
        let failures = Arc::new(InMemoryArchiveFailureControl::default());
        let session_service = Arc::new(LocalSessionService::new_with_archive_failures(
            failures.clone(),
        ));
        (
            Arc::new(Self::new(session_service, MobControlPrincipal::Owner)),
            failures,
        )
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}

pub struct MobMcpDispatcher {
    state: Arc<MobMcpState>,
    tools: Arc<[Arc<ToolDef>]>,
    ops_registry: Option<Arc<dyn OpsLifecycleRegistry>>,
    owner_bridge_session_id: Option<SessionId>,
}

impl MobMcpDispatcher {
    pub fn new(state: Arc<MobMcpState>) -> Self {
        const PRIMER: &str = "A mob is a managed multi-agent team with shared lifecycle/events: \
            create -> spawn members -> wire/unwire trust -> stop/resume -> complete/destroy.";
        const COMMON: &str = "Use real mob tools (no simulation). Keep and reuse the returned \
            mob_id from mob_create. All mob_* tools except mob_create and mob_list require mob_id.";
        let tools = vec![
            // ── Mob-level (mob_*) ──────────────────────────────────────
            tool(
                "mob_create",
                &format!("{PRIMER} Create a new mob from a definition. Returns mob_id."),
                json!({"type":"object","properties":{"definition":{"type":"object"}},"required":["definition"]}),
            ),
            tool(
                "mob_list",
                &format!("List mobs or get detail for one. Omit mob_id for summary of all mobs; \
                     provide mob_id for detailed status of that mob. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"}}}),
            ),
            tool(
                "mob_lifecycle",
                &format!("Lifecycle action on a mob. action: stop | resume | reset | complete | destroy. {COMMON}"),
                lifecycle_input_schema(),
            ),
            tool(
                "mob_events",
                &format!("Fetch mob lifecycle events. Optional: after_cursor, limit. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"after_cursor":{"type":"integer"},"limit":{"type":"integer"}},"required":["mob_id"]}),
            ),
            tool(
                "mob_run_flow",
                &format!("Start a configured flow run. Required: mob_id, flow_id. Optional params object. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"flow_id":{"type":"string"},"params":{"type":"object"}},"required":["mob_id","flow_id"]}),
            ),
            tool(
                "mob_flow_status",
                &format!("Read flow run status and ledgers by run_id. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"run_id":{"type":"string"}},"required":["mob_id","run_id"]}),
            ),
            tool(
                "mob_cancel_flow",
                &format!("Cancel an in-flight flow run by run_id. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"run_id":{"type":"string"}},"required":["mob_id","run_id"]}),
            ),
            // ── Member-level (mob_* member ops) ────────────────────────
            tool(
                "mob_spawn_member",
                &format!("Spawn one or more mob members. Required: mob_id, specs[].profile, specs[].agent_identity. \
                     Optional per-spec: backend=session|external, runtime_mode=autonomous_host|turn_driven, \
                     initial_message, labels (key-value map), context (opaque JSON), \
                     placement (comms peer id of a bound member host). {COMMON}"),
                json!({
                    "type":"object",
                    "properties":{
                        "mob_id":{"type":"string"},
                        "specs":{
                            "type":"array",
                            "items":{
                                "type":"object",
                                "properties":{
                                    "profile":{"type":"string"},
                                    "agent_identity":{"type":"string"},
                                    "initial_message": content_input_schema(),
                                    "backend":{"type":"string","enum":["session","external"]},
                                    "binding": runtime_binding_schema(),
                                    "runtime_mode":{"type":"string","enum":["autonomous_host","turn_driven"]},
                                    "labels":{"type":"object","additionalProperties":{"type":"string"}},
                                    "context":{"type":"object"},
                                    "placement":{"type":"string","description":"Comms peer id of a bound member host to place this member on (multi-host mobs)"}
                                },
                                "required":["profile","agent_identity"]
                            }
                        }
                    },
                    "required":["mob_id","specs"]
                }),
            ),
            tool(
                "mob_retire_member",
                &format!("Retire a spawned mob member by identity. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"agent_identity":{"type":"string"}},"required":["mob_id","agent_identity"]}),
            ),
            tool(
                "mob_list_members",
                &format!("List current members in a mob. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"}},"required":["mob_id"]}),
            ),
            tool(
                "mob_wire",
	                &format!("Wire or unwire bidirectional trust between a local member and a peer target. \
	                     action: wire | unwire. Use external_binding for wire and external name handles for unwire. {COMMON}"),
                json!({
                    "type":"object",
                    "properties":{
                        "mob_id":{"type":"string"},
                        "agent_identity":{"type":"string"},
                        "peer":{
                            "oneOf":[
                                {
                                    "type":"object",
                                    "properties":{"local":{"type":"string"}},
                                    "required":["local"],
                                    "additionalProperties":false
                                },
                                {
                                    "type":"object",
                                    "properties":{
	                                        "external_binding":{
	                                            "type":"object",
	                                            "properties":{
	                                                "name":{"type":"string"},
                                                "address":{"type":"string"},
                                                "identity":{
                                                    "type":"object",
                                                    "properties":{
                                                        "kind":{"type":"string","enum":["ed25519_public_key"]},
                                                        "public_key":{"type":"string"}
                                                    },
                                                    "required":["kind","public_key"],
                                                    "additionalProperties":false
                                                }
	                                            },
	                                            "required":["name","address","identity"],
	                                            "additionalProperties":false
	                                        }
	                                    },
	                                    "required":["external_binding"],
	                                    "additionalProperties":false
	                                },
	                                {
	                                    "type":"object",
	                                    "properties":{
	                                        "external":{
	                                            "type":"object",
	                                            "properties":{
	                                                "name":{"type":"string"}
	                                            },
	                                            "required":["name"],
	                                            "additionalProperties":false
	                                        }
	                                    },
	                                    "required":["external"],
	                                    "additionalProperties":false
	                                }
	                            ]
	                        },
                        "action":{"type":"string","enum":["wire","unwire"]}
                    },
                    "required":["mob_id","agent_identity","peer","action"]
                }),
            ),
            tool(
                "mob_respawn",
                &format!("Retire and re-spawn a member with the same profile. \
                     Required: mob_id, agent_identity. Optional: initial_message. \
                     Returns an identity-native respawn receipt. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"agent_identity":{"type":"string"},"initial_message": content_input_schema()},"required":["mob_id","agent_identity"]}),
            ),
            tool(
                "mob_force_cancel",
                &format!("Force-cancel a member's in-flight turn. Unlike retire, this \
                     interrupts immediately without graceful shutdown. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"agent_identity":{"type":"string"}},"required":["mob_id","agent_identity"]}),
            ),
            tool(
                "mob_member_status",
                &format!("Get execution status snapshot for a member. Returns status, \
                     output_preview (the current bridge session's last committed assistant text), \
                     tokens_used, and is_final. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"agent_identity":{"type":"string"}},"required":["mob_id","agent_identity"]}),
            ),
            tool(
                "mob_wait_kickoff",
                &format!(
                    "Wait until autonomous kickoff turns complete. Optional: member_ids (subset), timeout_ms. Returns member snapshots. {COMMON}"
                ),
                json!({
                    "type":"object",
                    "properties":{
                        "mob_id":{"type":"string"},
                        "member_ids":{"type":"array","items":{"type":"string"}},
                        "timeout_ms":{"type":"integer","minimum":1}
                    },
                    "required":["mob_id"]
                }),
            ),
            tool(
                "mob_wait_ready",
                &format!(
                    "Wait until mob startup readiness (members bound but kickoff not required). Optional: member_ids (subset), timeout_ms. Returns member snapshots; in session-bound agent turns this may detach and return status plus operation_id. {COMMON}"
                ),
                json!({
                    "type":"object",
                    "properties":{
                        "mob_id":{"type":"string"},
                        "member_ids":{"type":"array","items":{"type":"string"}},
                        "timeout_ms":{"type":"integer","minimum":1}
                    },
                    "required":["mob_id"]
                }),
            ),
        ]
        .into();
        Self {
            state,
            tools,
            ops_registry: None,
            owner_bridge_session_id: None,
        }
    }

    fn dispatch_detached_wait_ready(
        &self,
        call: ToolCallView<'_>,
        mob_id: MobId,
        member_ids: Option<Vec<AgentIdentity>>,
        timeout_ms: Option<u64>,
    ) -> Option<Result<meerkat_core::ToolDispatchOutcome, ToolError>> {
        let registry = self.ops_registry.as_ref()?.clone();
        let owner_bridge_session_id = self.owner_bridge_session_id.clone()?;
        let operation_id = OperationId::new();
        let spec = OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: owner_bridge_session_id,
            display_name: format!("mob_wait_ready {}", mob_id.as_str()),
            source_label: "mob_wait_ready".to_string(),
            operation_source: None,
            child_session_id: None,
            expect_peer_channel: false,
        };

        if let Err(error) = registry.register_operation(spec) {
            return Some(Err(ToolError::execution_failed(format!(
                "register mob_wait_ready background operation: {error}"
            ))));
        }
        if let Err(error) = registry.provisioning_succeeded(&operation_id) {
            return Some(Err(ToolError::execution_failed(format!(
                "start mob_wait_ready background operation: {error}"
            ))));
        }

        let state = Arc::clone(&self.state);
        let registry_for_task = Arc::clone(&registry);
        let operation_id_for_task = operation_id.clone();
        let mob_id_for_task = mob_id.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            match state
                .mob_wait_ready(&mob_id_for_task, member_ids, timeout_ms)
                .await
            {
                Ok(members) => {
                    let content = match serde_json::to_string(&json!({ "members": members })) {
                        Ok(content) => content,
                        Err(error) => {
                            let _ = registry_for_task.fail_operation(
                                &operation_id_for_task,
                                format!("encode mob_wait_ready background result: {error}"),
                            );
                            return;
                        }
                    };
                    let duration_ms =
                        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                    let result = OperationResult {
                        id: operation_id_for_task.clone(),
                        content,
                        is_error: false,
                        duration_ms,
                        tokens_used: 0,
                    };
                    let _ = registry_for_task.complete_operation(&operation_id_for_task, result);
                }
                Err(error) => {
                    let _ =
                        registry_for_task.fail_operation(&operation_id_for_task, error.to_string());
                }
            }
        });

        Some(Self::encode_detached_wait_result(
            call,
            mob_id,
            operation_id,
        ))
    }

    fn encode_detached_wait_result(
        call: ToolCallView<'_>,
        mob_id: MobId,
        operation_id: OperationId,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let content = serde_json::to_string(&json!({
            "status": "waiting",
            "mob_id": mob_id,
            "operation_id": operation_id,
            "wait_policy": "detached"
        }))
        .map_err(|error| ToolError::execution_failed(format!("encode tool result: {error}")))?;
        Ok(meerkat_core::ToolDispatchOutcome::new(
            ToolResult::new(call.id.to_string(), content, false),
            vec![AsyncOpRef::detached(operation_id)],
            vec![],
        ))
    }
}

fn tool(name: &str, description: &str, input_schema: serde_json::Value) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: name.into(),
        description: description.to_string(),
        input_schema,
        provenance: Some(ToolProvenance {
            kind: ToolSourceKind::Mob,
            source_id: "mob".into(),
        }),
    })
}

fn lifecycle_input_schema() -> serde_json::Value {
    // K1: ONE infallible schema generator — no fail-open null-schema arm.
    meerkat_core::schema::tool_input_schema_for::<MobLifecycleParams>()
}

fn content_input_schema() -> serde_json::Value {
    json!({
        "oneOf": [
            { "type": "string" },
            {
                "type": "array",
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "text" },
                                "text": { "type": "string" }
                            },
                            "required": ["type", "text"]
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "image" },
                                "media_type": { "type": "string" },
                                "data": { "type": "string" }
                            },
                            "required": ["type", "media_type", "data"]
                        }
                    ]
                }
            }
        ]
    })
}

fn runtime_binding_schema() -> serde_json::Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "session" }
                },
                "required": ["kind"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "external" },
                    "address": { "type": "string" },
                    "bootstrap_token": { "type": "string" },
                    "identity": {
                        "type": "object",
                        "properties": {
                            "kind": { "const": "ed25519_public_key" },
                            "public_key": { "type": "string" }
                        },
                        "required": ["kind", "public_key"],
                        "additionalProperties": false
                    }
                },
                "required": ["kind", "address", "identity"],
                "additionalProperties": false
            }
        ]
    })
}

fn encode(call: ToolCallView<'_>, payload: serde_json::Value) -> Result<ToolResult, ToolError> {
    let content = serde_json::to_string(&payload)
        .map_err(|e| ToolError::execution_failed(format!("encode result: {e}")))?;
    Ok(ToolResult::new(call.id.to_string(), content, false))
}

fn map_mob_err(call: ToolCallView<'_>, err: MobError) -> ToolError {
    ToolError::execution_failed(format!("tool '{}' failed: {err}", call.name))
}

fn map_destroy_err(call: ToolCallView<'_>, err: MobMcpDestroyError) -> ToolError {
    match err {
        MobMcpDestroyError::Incomplete { report } => ToolError::execution_failed_with_data(
            format!(
                "tool '{}' failed: mob destroy incomplete: {}",
                call.name,
                destroy_report_summary(&report)
            ),
            MobMcpDestroyError::incomplete_error_data(&report),
        ),
        MobMcpDestroyError::Mob(error) => map_mob_err(call, error),
    }
}

#[derive(Deserialize)]
struct MobCreateArgs {
    definition: MobDefinitionInput,
}
#[derive(Deserialize)]
struct MobListArgs {
    #[serde(default)]
    mob_id: Option<String>,
}
#[derive(Deserialize)]
struct MobIdArgs {
    mob_id: String,
}
#[derive(Debug, Deserialize)]
struct MobSpawnMeerkatArgs {
    profile: String,
    agent_identity: String,
    #[serde(default)]
    initial_message: Option<ContentInput>,
    #[serde(default)]
    backend: Option<MobBackendKind>,
    #[serde(default)]
    binding: Option<meerkat_contracts::WireRuntimeBinding>,
    #[serde(default)]
    runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    context: Option<serde_json::Value>,
    #[serde(default)]
    additional_instructions: Option<Vec<String>>,
    /// Host placement (comms peer id of a bound member host, multi-host
    /// mobs ADJ-7); admission stays with the spawn-exec ladder.
    #[serde(default)]
    placement: Option<String>,
}
#[derive(Debug, Deserialize)]
struct SpawnManyMeerkatsArgs {
    mob_id: String,
    specs: Vec<MobSpawnMeerkatArgs>,
}
#[derive(Deserialize)]
struct RetireArgs {
    mob_id: String,
    agent_identity: String,
}
#[derive(Deserialize)]
struct WireActionArgs {
    mob_id: String,
    agent_identity: String,
    peer: WireActionPeerTarget,
    action: WireMobWireAction,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WireActionPeerTarget {
    Local(WireActionLocalPeerTarget),
    ExternalBinding(WireActionExternalBindingTarget),
    External(WireActionExternalHandleTarget),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireActionLocalPeerTarget {
    local: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireActionExternalBindingTarget {
    external_binding: WireActionExternalBinding,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireActionExternalHandleTarget {
    external: WireActionExternalHandle,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireActionExternalBinding {
    name: String,
    address: String,
    identity: meerkat_contracts::WireTrustedPeerIdentity,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireActionExternalHandle {
    name: String,
}

fn runtime_binding_from_wire(
    binding: meerkat_contracts::WireRuntimeBinding,
) -> Result<meerkat_mob::RuntimeBinding, String> {
    match binding {
        meerkat_contracts::WireRuntimeBinding::Session => Ok(meerkat_mob::RuntimeBinding::Session),
        meerkat_contracts::WireRuntimeBinding::External {
            address,
            bootstrap_token,
            identity,
        } => {
            let resolved = identity.resolve().map_err(|err| err.to_string())?;
            Ok(meerkat_mob::RuntimeBinding::External {
                peer_id: resolved.peer_id.to_string(),
                address,
                bootstrap_token,
                pubkey: resolved.pubkey,
            })
        }
    }
}

/// Project the canonical mob lifecycle state into its wire mirror. Keeps the
/// surface emitting a closed typed status rather than the bare `MobState` text.
///
/// Exported so sibling surfaces (e.g. the JSON-RPC `mob/list` / `mob/status`
/// handlers) consume this single `MobState -> WireMobLifecycleStatus` owner
/// rather than re-deriving the projection from `MobState::to_string()`.
pub fn wire_mob_lifecycle_status(state: MobState) -> WireMobLifecycleStatus {
    match state {
        MobState::Creating => WireMobLifecycleStatus::Creating,
        MobState::Running => WireMobLifecycleStatus::Running,
        MobState::Stopped => WireMobLifecycleStatus::Stopped,
        MobState::Completed => WireMobLifecycleStatus::Completed,
        MobState::Destroyed => WireMobLifecycleStatus::Destroyed,
    }
}

impl WireActionArgs {
    fn resolve(
        self,
    ) -> Result<
        (
            String,
            AgentIdentity,
            meerkat_mob::PeerTarget,
            WireMobWireAction,
        ),
        String,
    > {
        let action = self.action;
        let target = match self.peer {
            WireActionPeerTarget::Local(WireActionLocalPeerTarget { local }) => {
                meerkat_mob::PeerTarget::Local(local.into())
            }
            WireActionPeerTarget::ExternalBinding(WireActionExternalBindingTarget {
                external_binding,
            }) => {
                meerkat_mob::PeerTarget::ExternalBinding(meerkat_mob::ExternalPeerBindingSpec::new(
                    external_binding.name,
                    external_binding.address,
                    external_binding.identity,
                ))
            }
            WireActionPeerTarget::External(WireActionExternalHandleTarget { external }) => {
                if matches!(action, WireMobWireAction::Wire) {
                    return Err("wire external peer requires external_binding".to_string());
                }
                let peer_name = PeerName::new(external.name)
                    .map_err(|e| format!("invalid external peer name: {e}"))?;
                meerkat_mob::PeerTarget::ExternalName(peer_name)
            }
        };
        Ok((
            self.mob_id,
            AgentIdentity::from(self.agent_identity),
            target,
            action,
        ))
    }
}
#[derive(Deserialize)]
struct RunFlowArgs {
    mob_id: String,
    flow_id: String,
    #[serde(default)]
    params: serde_json::Value,
}
#[derive(Deserialize)]
struct FlowStatusArgs {
    mob_id: String,
    run_id: String,
}
#[derive(Deserialize)]
struct EventsArgs {
    mob_id: String,
    #[serde(default)]
    after_cursor: u64,
    #[serde(default = "default_limit")]
    limit: usize,
}
#[derive(Deserialize)]
struct RespawnArgs {
    mob_id: String,
    agent_identity: String,
    #[serde(default)]
    initial_message: Option<ContentInput>,
}
#[derive(Deserialize)]
struct ForceCancelArgs {
    mob_id: String,
    agent_identity: String,
}
#[derive(Deserialize)]
struct MeerkatStatusArgs {
    mob_id: String,
    agent_identity: String,
}
#[derive(Deserialize)]
struct WaitKickoffArgs {
    mob_id: String,
    #[serde(default)]
    member_ids: Option<Vec<String>>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}
#[derive(Deserialize)]
struct WaitReadyArgs {
    mob_id: String,
    #[serde(default)]
    member_ids: Option<Vec<String>>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}
fn default_limit() -> usize {
    100
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for MobMcpDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tools.clone()
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let started = Instant::now();
        tracing::info!(
            target: "mob_tools",
            "MobMcpDispatcher::dispatch start tool={} tool_use_id={}",
            call.name,
            call.id
        );
        let result = match call.name {
            // ── Mob-level ──────────────────────────────────────────────
            "mob_create" => {
                let args: MobCreateArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let definition = decode_public_mob_definition(args.definition)
                    .map_err(|e| ToolError::invalid_arguments(call.name, e))?;
                let mob_id = self
                    .state
                    .mob_create_definition(definition)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"mob_id": mob_id}))
            }
            "mob_list" => {
                let args: MobListArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                if let Some(mob_id) = args.mob_id {
                    let status = self
                        .state
                        .mob_status(&MobId::from(mob_id))
                        .await
                        .map_err(|e| map_mob_err(call, e))?;
                    encode(call, json!({"status": wire_mob_lifecycle_status(status)}))
                } else {
                    let mobs = self
                        .state
                        .mob_list()
                        .await
                        .map_err(|e| map_mob_err(call, e))?;
                    encode(
                        call,
                        json!({"mobs": mobs.into_iter().map(|(id, status)| json!({"mob_id": id, "status": wire_mob_lifecycle_status(status)})).collect::<Vec<_>>() }),
                    )
                }
            }
            "mob_lifecycle" => {
                let args: MobLifecycleParams = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let mob_id = MobId::from(args.mob_id);
                // MCP tool surface treats destroy as "() on success"; the
                // structured MobDestroyReport is available via public/RPC
                // lifecycle result envelopes for consumers that need detail.
                let _destroy_report = self
                    .state
                    .mob_lifecycle_action(&mob_id, args.action)
                    .await
                    .map_err(|e| map_destroy_err(call, e))?;
                encode(call, json!({"ok": true}))
            }
            "mob_events" => {
                let args: EventsArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let events = self
                    .state
                    .mob_events(&MobId::from(args.mob_id), args.after_cursor, args.limit)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"events": events}))
            }
            "mob_run_flow" => {
                let args: RunFlowArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let run_id = self
                    .state
                    .mob_run_flow(
                        &MobId::from(args.mob_id),
                        FlowId::from(args.flow_id),
                        args.params,
                    )
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"run_id": run_id}))
            }
            "mob_flow_status" => {
                let args: FlowStatusArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let run_id = args.run_id.parse::<RunId>().map_err(|e| {
                    ToolError::invalid_arguments(call.name, format!("invalid run_id: {e}"))
                })?;
                let run = self
                    .state
                    .mob_flow_status(&MobId::from(args.mob_id), run_id)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                let run = meerkat_mob::MobRun::public_flow_status_run_value(run.as_ref())
                    .map_err(|e| map_mob_err(call, e))?;
                let result = serde_json::to_value(meerkat_contracts::MobFlowStatusResult { run })
                    .map_err(|e| {
                    ToolError::invalid_arguments(
                        call.name,
                        format!("failed to encode flow status: {e}"),
                    )
                })?;
                encode(call, result)
            }
            "mob_cancel_flow" => {
                let args: FlowStatusArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let run_id = args.run_id.parse::<RunId>().map_err(|e| {
                    ToolError::invalid_arguments(call.name, format!("invalid run_id: {e}"))
                })?;
                self.state
                    .mob_cancel_flow(&MobId::from(args.mob_id), run_id)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"ok": true}))
            }
            // ── Member-level ───────────────────────────────────────────
            "mob_spawn_member" => {
                let args: SpawnManyMeerkatsArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let specs = args
                    .specs
                    .into_iter()
                    .map(|spec| {
                        let mut s = SpawnMemberSpec::new(spec.profile, spec.agent_identity);
                        s.initial_message = spec.initial_message;
                        s.runtime_mode = spec.runtime_mode;
                        s.backend = spec.backend;
                        s.binding = spec
                            .binding
                            .map(runtime_binding_from_wire)
                            .transpose()
                            .map_err(|e| ToolError::invalid_arguments(call.name, e))?;
                        s.context = spec.context;
                        s.labels = spec.labels;
                        s.additional_instructions = spec.additional_instructions;
                        s.placement = spec
                            .placement
                            .map(meerkat_mob::machines::mob_machine::HostId::from);
                        Ok(s)
                    })
                    .collect::<Result<Vec<_>, ToolError>>()?;
                let mob_id = MobId::from(args.mob_id);
                let results = self
                    .state
                    .mob_spawn_many(&mob_id, specs)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                let results = results
                    .into_iter()
                    .map(
                        |result: Result<
                            meerkat_mob::SpawnResult,
                            meerkat_mob::MobSpawnManyFailure,
                        >| {
                            match result {
                                Ok(spawn_result) => {
                                    let identity = spawn_result.agent_identity.to_string();
                                    json!(MobSpawnManyResultEntry::spawned(
                                        identity.clone(),
                                        WireMemberRef::encode(mob_id.as_str(), &identity),
                                    ))
                                }
                                Err(error) => json!(MobSpawnManyResultEntry::failed(
                                    error.cause(),
                                    error.to_string(),
                                )),
                            }
                        },
                    )
                    .collect::<Vec<_>>();
                encode(call, json!({"results": results}))
            }
            "mob_retire_member" => {
                let args: RetireArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                self.state
                    .mob_retire(
                        &MobId::from(args.mob_id),
                        AgentIdentity::from(args.agent_identity),
                    )
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"ok": true}))
            }
            "mob_list_members" => {
                let args: MobIdArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let rows: Vec<meerkat_mob::runtime::MobMemberListEntry> = self
                    .state
                    .mob_list_members(&MobId::from(args.mob_id))
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"members": rows}))
            }
            "mob_wire" => {
                let args: WireActionArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let (mob_id, local, target, action) = args
                    .resolve()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e))?;
                let mob_id = MobId::from(mob_id);
                match action {
                    WireMobWireAction::Wire => self
                        .state
                        .mob_wire(&mob_id, local, target)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                    WireMobWireAction::Unwire => self
                        .state
                        .mob_unwire(&mob_id, local, target)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                }
                encode(call, json!({"ok": true}))
            }
            "mob_respawn" => {
                let args: RespawnArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                match self
                    .state
                    .mob_respawn(
                        &MobId::from(args.mob_id),
                        AgentIdentity::from(args.agent_identity.as_str()),
                        args.initial_message,
                    )
                    .await
                {
                    Ok(receipt) => encode(
                        call,
                        json!({
                            "status": WireMobRespawnOutcome::Completed,
                            "receipt": receipt,
                        }),
                    ),
                    Err(meerkat_mob::MobRespawnError::TopologyRestoreFailed {
                        receipt,
                        failed_peer_ids,
                    }) => encode(
                        call,
                        json!({
                            "status": WireMobRespawnOutcome::TopologyRestoreFailed,
                            "receipt": receipt,
                            "failed_peer_ids": failed_peer_ids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>(),
                        }),
                    ),
                    Err(e) => return Err(map_mob_err(call, MobError::Internal(e.to_string()))),
                }
            }
            "mob_force_cancel" => {
                let args: ForceCancelArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                self.state
                    .mob_force_cancel(
                        &MobId::from(args.mob_id),
                        AgentIdentity::from(args.agent_identity),
                    )
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"ok": true}))
            }
            "mob_member_status" => {
                let args: MeerkatStatusArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let mob_id = MobId::from(args.mob_id);
                let identity = AgentIdentity::from(args.agent_identity);
                let snapshot = self
                    .state
                    .mob_member_status(&mob_id, &identity)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                let member_ref =
                    meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), identity.as_str());
                let result = snapshot.to_member_status_result(member_ref).map_err(|e| {
                    ToolError::invalid_arguments(
                        call.name,
                        format!("failed to project mob member status: {e}"),
                    )
                })?;
                encode(call, json!(result))
            }
            "mob_wait_kickoff" => {
                let args: WaitKickoffArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let member_ids = args.member_ids.map(|ids| {
                    ids.into_iter()
                        .map(|id| AgentIdentity::from(id.as_str()))
                        .collect::<Vec<_>>()
                });
                let members = self
                    .state
                    .mob_wait_kickoff(&MobId::from(args.mob_id), member_ids, args.timeout_ms)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"members": members}))
            }
            "mob_wait_ready" => {
                let args: WaitReadyArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let member_ids = args.member_ids.map(|ids| {
                    ids.into_iter()
                        .map(|id| AgentIdentity::from(id.as_str()))
                        .collect::<Vec<_>>()
                });
                let mob_id = MobId::from(args.mob_id);
                if self.ops_registry.is_some() && self.owner_bridge_session_id.is_some() {
                    self.state
                        .handle_for(&mob_id)
                        .await
                        .map_err(|e| map_mob_err(call, e))?;
                }
                if let Some(result) = self.dispatch_detached_wait_ready(
                    call,
                    mob_id.clone(),
                    member_ids.clone(),
                    args.timeout_ms,
                ) {
                    return result;
                }
                let members = self
                    .state
                    .mob_wait_ready(&mob_id, member_ids, args.timeout_ms)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"members": members}))
            }
            _ => Err(ToolError::not_found(call.name)),
        };
        match &result {
            Ok(_) => tracing::info!(
                target: "mob_tools",
                "MobMcpDispatcher::dispatch ok tool={} elapsed_ms={}",
                call.name,
                started.elapsed().as_millis()
            ),
            Err(err) => tracing::warn!(
                target: "mob_tools",
                "MobMcpDispatcher::dispatch err tool={} elapsed_ms={} err={}",
                call.name,
                started.elapsed().as_millis(),
                err
            ),
        }
        result.map(Into::into)
    }

    fn capabilities(&self) -> DispatcherCapabilities {
        DispatcherCapabilities {
            ops_lifecycle: true,
        }
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn OpsLifecycleRegistry>,
        owner_bridge_session_id: SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        if Arc::strong_count(&self) != 1 {
            return Err(OpsLifecycleBindError::SharedOwnership);
        }
        let this = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        Ok(BindOutcome::Bound(Arc::new(Self {
            state: this.state,
            tools: this.tools,
            ops_registry: Some(registry),
            owner_bridge_session_id: Some(owner_bridge_session_id),
        })))
    }
}

#[derive(Debug, Clone)]
pub struct McpToolError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

impl McpToolError {
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }

    pub fn capability_unavailable(message: impl Into<String>) -> Self {
        Self {
            code: meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            message: message.into(),
            data: None,
        }
    }

    pub fn method_not_found(message: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: message.into(),
            data: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }

    pub fn destroy_incomplete(report: &meerkat_mob::MobDestroyReport) -> Self {
        Self {
            code: meerkat_contracts::ErrorCode::InternalError.jsonrpc_code(),
            message: MobMcpDestroyError::incomplete_message(report),
            data: Some(MobMcpDestroyError::incomplete_error_data(report)),
        }
    }

    /// Shared typed-detail envelope (§17.4, DEC-P7B-9): `code` is the
    /// stable `ErrorCode::jsonrpc_code()` (recoverable via
    /// `from_jsonrpc_code`), `data` is the BARE detail struct — the SAME
    /// data shape RPC ships, no MCP-only wrapper. A detail that fails to
    /// serialize fails CLOSED as an internal error (mirrors
    /// `mob_scope_denied_error`), never as a silently detail-less success
    /// shape.
    fn from_wire_detail(
        detail: meerkat_contracts::wire::WireMobErrorDetail,
        message: String,
    ) -> Self {
        match detail.detail_value() {
            Ok(data) => Self {
                code: detail.code().jsonrpc_code(),
                message,
                data: Some(data),
            },
            Err(e) => Self::internal(format!("failed to serialize mob error detail: {e}")),
        }
    }

    /// §17.4 typed rendering for mob errors (DEC-P7B-9): the four console
    /// codes carry stable JSON-RPC codes + typed data via
    /// [`MobError::wire_detail`]; everything else keeps the byte-identical
    /// legacy `invalid_params(err.to_string())` fallback.
    pub fn from_mob(err: &MobError) -> Self {
        match err.wire_detail() {
            Some(detail) => Self::from_wire_detail(detail, err.to_string()),
            None => Self::invalid_params(err.to_string()),
        }
    }

    /// [`Self::from_mob`] over the respawn wrapper (delegates through
    /// `MobRespawnError::wire_detail`).
    pub fn from_mob_respawn(err: &meerkat_mob::MobRespawnError) -> Self {
        match err.wire_detail() {
            Some(detail) => Self::from_wire_detail(detail, err.to_string()),
            None => Self::invalid_params(err.to_string()),
        }
    }

    /// [`Self::from_mob`] over the destroy wrapper: the `Incomplete` arm
    /// keeps its dedicated `destroy_incomplete` envelope verbatim.
    pub fn from_mob_destroy(err: &MobMcpDestroyError) -> Self {
        match err {
            MobMcpDestroyError::Incomplete { report } => Self::destroy_incomplete(report),
            MobMcpDestroyError::Mob(inner) => Self::from_mob(inner),
        }
    }

    /// Preserve mob-family authorization details for the composite
    /// system-context verb while retaining the legacy invalid-params mapping
    /// for genuine session-control failures.
    pub fn from_mob_append_system_context(err: &MobAppendSystemContextError) -> Self {
        match err {
            MobAppendSystemContextError::Mob(inner) => Self::from_mob(inner),
            MobAppendSystemContextError::Session(inner) => Self::invalid_params(inner.to_string()),
        }
    }
}

/// Return the agent-side `mob_*` dispatcher tool inventory.
///
/// This mirrors `MobMcpDispatcher` and is intended for in-session/agent tool
/// composition helpers. Public host surfaces should use `public_tools_list()`
/// instead.
pub fn tools_list() -> Vec<serde_json::Value> {
    let dispatcher = MobMcpDispatcher::new(MobMcpState::new_in_memory());
    dispatcher
        .tools()
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema
            })
        })
        .collect()
}

/// Dispatch an agent-side `mob_*` tool call against `MobMcpDispatcher`.
///
/// This is the internal dispatcher helper for in-session agent tools. Public
/// host MCP surfaces should use `handle_public_tools_call()` instead.
pub async fn handle_tools_call(
    state: &Arc<MobMcpState>,
    name: &str,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, McpToolError> {
    let dispatcher = MobMcpDispatcher::new(state.clone());
    let raw = serde_json::value::RawValue::from_string(arguments.to_string()).map_err(|e| {
        McpToolError {
            code: -32602,
            message: format!("invalid arguments: {e}"),
            data: None,
        }
    })?;
    let result = dispatcher
        .dispatch(ToolCallView {
            id: "mcp-tool-call",
            name,
            args: &raw,
        })
        .await
        .map_err(|e| McpToolError {
            code: -32000,
            message: e.to_string(),
            data: e.structured_data(),
        })?;
    let text = result.result.text_content();
    serde_json::from_str(&text).map_err(|e| McpToolError {
        code: -32603,
        message: format!("invalid tool result payload: {e}"),
        data: Some(json!({"raw": text})),
    })
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::collapsible_if,
    clippy::panic
)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::InteractionId;
    use meerkat_core::PlainEventSource;
    use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
    use meerkat_core::comms::{
        CommsCommand, CommsTrustMutation, CommsTrustMutationResult,
        GeneratedCommsTrustAuthoritySourceKind, PeerId, SendError, SendReceipt,
        TrustedPeerDescriptor,
    };
    use meerkat_core::event::AgentEvent;
    use meerkat_core::event_injector::{
        EventInjector, EventInjectorError, InteractionSubscription, SubscribableInjector,
    };
    use meerkat_core::interaction::PeerInputCandidate;
    use meerkat_core::service::InitialTurnPolicy;
    use meerkat_core::service::SessionService;
    use meerkat_core::service::{
        CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionSummary,
        SessionUsage, SessionView, StartTurnRequest,
    };
    use meerkat_core::types::{RunResult, SessionId, Usage};
    use meerkat_core::{
        PeerMeta, Provider, Session, SessionMetadata, SessionTooling, ToolCategoryOverride,
    };
    use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::SystemTime;
    use tokio::sync::Notify;
    use tokio::time::{Duration, Instant, sleep};

    const ED25519_PUBLIC_KEY_7: &str = "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";

    fn external_peer_target(public_key: &str) -> WireActionPeerTarget {
        serde_json::from_value(json!({
            "external_binding": {
                "name": "external-worker",
                "address": "inproc://external-worker",
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": public_key
                }
            }
        }))
        .expect("external binding target should deserialize")
    }

    #[tokio::test]
    async fn test_local_comms_runtime_trust_requires_mutation_authority() {
        let runtime = LocalCommsRuntime::new("local");
        let mut pubkey = [0u8; 32];
        pubkey[0] = 42;
        let peer_id = PeerId::from_ed25519_pubkey(&pubkey).to_string();
        let peer = TrustedPeerDescriptor::unsigned_with_pubkey(
            "peer".to_string(),
            peer_id.clone(),
            pubkey,
            "inproc://peer",
        )
        .expect("valid peer descriptor");

        let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&peer);
        let projection_authority = std::sync::Arc::new(std::sync::Mutex::new(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineAuthority::new(),
        ));
        {
            let mut authority = projection_authority
                .lock()
                .expect("projection authority lock should not be poisoned");
            authority
                .apply_signal(
                    meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
                )
                .expect("Initialize signal");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                    session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(
                        "mob-mcp-local-comms-test",
                    ),
                },
            )
            .expect("RegisterSession input");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::PublishLocalEndpoint {
                    endpoint: meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::new(
                        "local",
                        runtime.peer_id.to_string(),
                        runtime.address.clone(),
                        runtime.public_key_bytes,
                    ),
                },
            )
            .expect("PublishLocalEndpoint input");
        }
        let wiring_transition = {
            let mut authority = projection_authority
                .lock()
                .expect("projection authority lock should not be poisoned");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
                    epoch: 1,
                    endpoints: BTreeSet::from([endpoint.clone()]),
                },
            )
            .expect("ApplyMobPeerOverlay input")
        };
        let wiring_obligation =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &wiring_transition,
                meerkat_runtime::protocol_comms_trust_reconcile::PeerProjectionFreshnessAuthority::from_authority(
                    std::sync::Arc::clone(&projection_authority),
                ),
            )
            .pop()
            .expect("generated wiring obligation");
        let add_authority =
            meerkat_runtime::protocol_comms_trust_reconcile::authority_for_endpoint(
                &wiring_obligation,
                &endpoint,
            )
            .expect("generated wiring obligation covers peer");
        meerkat_runtime::RuntimePeerCommsHandle::install_generated_on(
            Arc::new(meerkat_runtime::HandleDslAuthority::from_shared(
                Arc::clone(&projection_authority),
            )),
            &runtime,
        )
        .expect("install generated peer-comms owner");

        let added = runtime
            .apply_trust_mutation(CommsTrustMutation::AddTrustedPeer {
                peer: peer.clone(),
                authority: add_authority,
            })
            .await
            .expect("authorized add succeeds");
        assert_eq!(added, CommsTrustMutationResult::Added { created: true });
        assert!(runtime.trusted.read().await.contains_key(&peer_id));

        let unwiring_transition = {
            let mut authority = projection_authority
                .lock()
                .expect("projection authority lock should not be poisoned");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
                    epoch: 2,
                    endpoints: BTreeSet::new(),
                },
            )
            .expect("ApplyMobPeerOverlay remove input")
        };
        let unwiring_obligation =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &unwiring_transition,
                meerkat_runtime::protocol_comms_trust_reconcile::PeerProjectionFreshnessAuthority::from_authority(
                    std::sync::Arc::clone(&projection_authority),
                ),
            )
            .pop()
            .expect("generated unwiring obligation");

        let removed = runtime
            .apply_trust_mutation(CommsTrustMutation::RemoveTrustedPeer {
                peer_id: peer_id.clone(),
                authority:
                    meerkat_runtime::protocol_comms_trust_reconcile::removal_authority_for_peer_id(
                        &unwiring_obligation,
                        &peer_id,
                    )
                    .expect("generated unwiring obligation covers peer"),
            })
            .await
            .expect("authorized remove succeeds");
        assert_eq!(removed, CommsTrustMutationResult::Removed { removed: true });
        assert!(!runtime.trusted.read().await.contains_key(&peer_id));
    }

    #[tokio::test]
    async fn test_local_comms_runtime_generated_source_rejects_descriptor_rewrite() {
        let runtime = LocalCommsRuntime::new("local");
        let mut pubkey = [0u8; 32];
        pubkey[0] = 45;
        let peer_id = PeerId::from_ed25519_pubkey(&pubkey).to_string();
        let peer = TrustedPeerDescriptor::unsigned_with_pubkey(
            "peer".to_string(),
            peer_id.clone(),
            pubkey,
            "inproc://peer",
        )
        .expect("valid peer descriptor");
        let rewritten = TrustedPeerDescriptor::unsigned_with_pubkey(
            "peer-renamed".to_string(),
            peer_id.clone(),
            pubkey,
            "inproc://peer-renamed",
        )
        .expect("valid peer descriptor");

        let source_kind = GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection;
        let created = runtime
            .add_generated_trust_source(peer.clone(), source_kind, false)
            .await
            .expect("initial generated trust row");
        assert!(created);
        let repeated = runtime
            .add_generated_trust_source(peer, source_kind, false)
            .await
            .expect("same generated trust row should be idempotent");
        assert!(!repeated);

        let conflict = runtime
            .add_generated_trust_source(rewritten, source_kind, false)
            .await
            .expect_err("same generated source must not rewrite descriptor material");
        assert!(
            matches!(conflict, SendError::Validation(ref message) if message.contains("already owns different trust material")),
            "unexpected conflict error: {conflict:?}"
        );
        let descriptors = runtime.trusted_descriptors.read().await;
        let descriptor = descriptors
            .get(&peer_id)
            .and_then(|by_source| by_source.get(&source_kind))
            .expect("generated descriptor remains installed");
        assert_eq!(descriptor.name.as_str(), "peer");
        assert_eq!(descriptor.address.to_string(), "inproc://peer");
    }

    #[tokio::test]
    async fn test_local_comms_runtime_source_snapshot_excludes_private_trust() {
        let runtime = LocalCommsRuntime::new("local");
        let source_kind = GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection;
        let public_key = [46u8; 32];
        let private_key = [47u8; 32];
        let public_peer = TrustedPeerDescriptor::unsigned_with_pubkey(
            "public-peer".to_string(),
            PeerId::from_ed25519_pubkey(&public_key).to_string(),
            public_key,
            "inproc://public-peer",
        )
        .expect("valid public peer descriptor");
        let private_peer = TrustedPeerDescriptor::unsigned_with_pubkey(
            "private-peer".to_string(),
            PeerId::from_ed25519_pubkey(&private_key).to_string(),
            private_key,
            "inproc://private-peer",
        )
        .expect("valid private peer descriptor");

        runtime
            .add_generated_trust_source(public_peer.clone(), source_kind, false)
            .await
            .expect("install public generated trust row");
        runtime
            .add_generated_trust_source(private_peer, source_kind, true)
            .await
            .expect("install private generated trust row");

        let snapshot = runtime
            .trusted_peer_projection_snapshot_for_source(source_kind)
            .await
            .expect("read public source projection");
        assert_eq!(snapshot, vec![public_peer]);
    }

    #[tokio::test]
    async fn test_local_comms_runtime_source_removal_is_cancellation_safe() {
        let runtime = Arc::new(LocalCommsRuntime::new("local"));
        let source_kind = GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection;
        let public_key = [48u8; 32];
        let peer = TrustedPeerDescriptor::unsigned_with_pubkey(
            "public-peer".to_string(),
            PeerId::from_ed25519_pubkey(&public_key).to_string(),
            public_key,
            "inproc://public-peer",
        )
        .expect("valid public peer descriptor");
        let peer_id = peer.peer_id.as_str().to_string();
        runtime
            .add_generated_trust_source(peer.clone(), source_kind, false)
            .await
            .expect("install generated trust row");

        let descriptor_guard = runtime.trusted_descriptors.write().await;
        let removal = {
            let runtime = Arc::clone(&runtime);
            let peer_id = peer_id.clone();
            tokio::spawn(async move {
                runtime
                    .remove_generated_trust_source(&peer_id, source_kind)
                    .await
            })
        };

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if tokio::time::timeout(Duration::from_millis(10), runtime.trusted.read())
                .await
                .is_err()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "removal should acquire the membership guard before waiting for descriptors"
            );
            tokio::task::yield_now().await;
        }
        removal.abort();
        let _ = removal.await;
        drop(descriptor_guard);

        assert!(
            runtime.trusted.read().await.contains_key(&peer_id),
            "cancellation before all trust guards are acquired must leave membership intact"
        );
        assert!(
            !runtime
                .add_generated_trust_source(peer.clone(), source_kind, false)
                .await
                .expect("the retained row should remain idempotent"),
            "cancellation must not leave an orphan descriptor that rejects the original row"
        );
        assert_eq!(
            runtime
                .trusted_peer_projection_snapshot_for_source(source_kind)
                .await
                .expect("read public source projection"),
            vec![peer]
        );
    }

    #[test]
    fn mob_spawn_member_args_accept_canonical_external_runtime_binding() {
        let args = serde_json::from_value::<SpawnManyMeerkatsArgs>(json!({
            "mob_id": "mob",
            "specs": [{
                "profile": "worker",
                "agent_identity": "w-ext",
                "binding": {
                    "kind": "external",
                    "address": "inproc://external-worker",
                    "identity": {
                        "kind": "ed25519_public_key",
                        "public_key": ED25519_PUBLIC_KEY_7
                    }
                }
            }]
        }))
        .expect("canonical external runtime binding should deserialize");

        let binding = args
            .specs
            .into_iter()
            .next()
            .and_then(|spec| spec.binding)
            .expect("binding present");
        let resolved = runtime_binding_from_wire(binding)
            .expect("canonical external runtime binding resolves");
        let meerkat_mob::RuntimeBinding::External {
            peer_id, pubkey, ..
        } = resolved
        else {
            panic!("expected external runtime binding");
        };
        let expected_pubkey = [7u8; 32];
        assert_eq!(
            peer_id,
            meerkat_core::comms::PeerId::from_ed25519_pubkey(&expected_pubkey).to_string()
        );
        assert_eq!(pubkey, expected_pubkey);
    }

    #[test]
    fn mob_spawn_member_args_reject_raw_external_runtime_binding_atoms() {
        let err = serde_json::from_value::<SpawnManyMeerkatsArgs>(json!({
            "mob_id": "mob",
            "specs": [{
                "profile": "worker",
                "agent_identity": "w-ext",
                "binding": {
                    "kind": "external",
                    "peer_id": meerkat_core::comms::PeerId::from_ed25519_pubkey(&[7u8; 32]).to_string(),
                    "address": "inproc://external-worker",
                    "pubkey": vec![7u8; 32]
                }
            }]
        }))
        .expect_err("raw peer_id/pubkey external runtime binding shape must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("peer_id") || msg.contains("identity"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn wire_action_args_rejects_raw_external_peer_atoms() {
        let err = serde_json::from_value::<WireActionPeerTarget>(json!({
            "external": {
                "name": "external-worker",
                "peer_id": meerkat_core::comms::PeerId::from_ed25519_pubkey(&[7u8; 32]).to_string(),
                "address": "inproc://external-worker",
                "pubkey": vec![7u8; 32]
            }
        }))
        .expect_err("raw peer_id/pubkey external peer shape must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("external")
                || msg.contains("external_binding")
                || msg.contains("did not match"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn wire_action_args_rejects_missing_external_binding_pubkey_material() {
        let err = serde_json::from_value::<WireActionPeerTarget>(json!({
            "external_binding": {
                "name": "external-worker",
                "address": "inproc://external-worker",
                "identity": {
                    "kind": "ed25519_public_key"
                }
            }
        }))
        .expect_err("missing external binding pubkey material must fail closed");

        let msg = err.to_string();
        assert!(
            msg.contains("public_key") || msg.contains("identity") || msg.contains("did not match"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn wire_action_args_rejects_ambiguous_peer_target_shape() {
        let err = serde_json::from_value::<WireActionPeerTarget>(json!({
            "local": "worker-a",
            "external_binding": {
                "name": "external-worker",
                "address": "inproc://external-worker",
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": ED25519_PUBLIC_KEY_7
                }
            }
        }))
        .expect_err("legacy wire action must not accept multiple peer target shapes");

        let msg = err.to_string();
        assert!(
            msg.contains("did not match")
                || msg.contains("unknown field")
                || msg.contains("external_binding"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn wire_action_args_rejects_external_handle_for_wire_action() {
        let err = WireActionArgs {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            peer: serde_json::from_value(json!({
                "external": { "name": "external-worker" }
            }))
            .expect("external handle should deserialize"),
            action: WireMobWireAction::Wire,
        }
        .resolve()
        .expect_err("wire action must require external_binding");

        assert!(
            err.contains("external_binding"),
            "expected external_binding validation error, got: {err}"
        );
    }

    #[test]
    fn wire_action_args_passes_external_binding_to_mob_authority() {
        let resolved = WireActionArgs {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            peer: external_peer_target(ED25519_PUBLIC_KEY_7),
            action: WireMobWireAction::Wire,
        }
        .resolve()
        .expect("canonical external peer binding should resolve as request");

        let (_mob_id, _local, target, _action) = resolved;
        let meerkat_mob::PeerTarget::ExternalBinding(binding) = target else {
            panic!("canonical external peer should remain mob-resolved external binding");
        };
        assert_eq!(binding.name, "external-worker");
        assert_eq!(binding.address, "inproc://external-worker");
    }

    #[test]
    fn wire_action_args_unwire_accepts_external_name_handle() {
        let resolved = WireActionArgs {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            peer: serde_json::from_value(json!({
                "external": { "name": "external-worker" }
            }))
            .expect("external handle should deserialize"),
            action: WireMobWireAction::Unwire,
        }
        .resolve()
        .expect("external handle should be valid for unwire");

        let (_mob_id, _local, target, _action) = resolved;
        let meerkat_mob::PeerTarget::ExternalName(peer_name) = target else {
            panic!("unwire external should use the external peer handle");
        };
        assert_eq!(peer_name.as_str(), "external-worker");
    }

    struct MockComms {
        name: String,
        peer_id: meerkat_core::comms::PeerId,
        public_key_bytes: [u8; 32],
        address: String,
        key: String,
        trusted: RwLock<HashMap<String, BTreeSet<GeneratedCommsTrustAuthoritySourceKind>>>,
        trusted_descriptors: RwLock<
            HashMap<String, HashMap<GeneratedCommsTrustAuthoritySourceKind, TrustedPeerDescriptor>>,
        >,
        private_trusted: RwLock<HashMap<String, BTreeSet<GeneratedCommsTrustAuthoritySourceKind>>>,
        mob_machine_trust_owner: RwLock<Option<Arc<dyn std::any::Any + Send + Sync>>>,
        notify: Arc<Notify>,
    }

    struct MockInjector;

    impl EventInjector for MockInjector {
        fn inject(
            &self,
            _body: ContentInput,
            _source: PlainEventSource,
            _handling_mode: HandlingMode,
            _render_metadata: Option<RenderMetadata>,
        ) -> Result<(), EventInjectorError> {
            Ok(())
        }
    }

    impl SubscribableInjector for MockInjector {
        fn inject_with_subscription(
            &self,
            body: ContentInput,
            source: PlainEventSource,
            handling_mode: HandlingMode,
            render_metadata: Option<RenderMetadata>,
        ) -> Result<InteractionSubscription, EventInjectorError> {
            self.inject(body, source, handling_mode, render_metadata)?;
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            let interaction_id = InteractionId(uuid::Uuid::new_v4());
            let interaction_id_for_task = interaction_id;
            tokio::spawn(async move {
                let _ = tx
                    .send(AgentEvent::InteractionComplete {
                        interaction_id: interaction_id_for_task,
                        result: "ok".to_string(),
                        structured_output: None,
                    })
                    .await;
            });
            Ok(InteractionSubscription {
                id: interaction_id,
                events: rx,
            })
        }

        fn inject_with_interaction_id(
            &self,
            _interaction_id: InteractionId,
            body: ContentInput,
            source: PlainEventSource,
            handling_mode: HandlingMode,
            render_metadata: Option<RenderMetadata>,
        ) -> Result<(), EventInjectorError> {
            self.inject(body, source, handling_mode, render_metadata)
        }
    }

    impl MockComms {
        fn new(name: &str) -> Self {
            let mut public_key_bytes = [0u8; 32];
            for (index, byte) in name.bytes().enumerate() {
                let slot = index % public_key_bytes.len();
                public_key_bytes[slot] = public_key_bytes[slot]
                    .wrapping_add(byte)
                    .wrapping_add(index as u8);
            }
            if public_key_bytes == [0u8; 32] {
                public_key_bytes[0] = 1;
            }
            let peer_id = meerkat_core::comms::PeerId::from_ed25519_pubkey(&public_key_bytes);
            Self {
                name: name.to_string(),
                peer_id,
                public_key_bytes,
                address: format!("inproc://{name}"),
                key: super::encode_ed25519_public_key(&public_key_bytes),
                trusted: RwLock::new(HashMap::new()),
                trusted_descriptors: RwLock::new(HashMap::new()),
                private_trusted: RwLock::new(HashMap::new()),
                mob_machine_trust_owner: RwLock::new(None),
                notify: Arc::new(Notify::new()),
            }
        }

        async fn validate_mob_trust_authority_owner(
            &self,
            authority: &meerkat_core::comms::CommsTrustMutationAuthority,
        ) -> Result<(), SendError> {
            if !authority.is_mob_machine_source() {
                return Ok(());
            }
            let expected = self.mob_machine_trust_owner.read().await;
            authority
                .validate_raw_source_owner_token(expected.as_ref())
                .map_err(SendError::Validation)
        }

        async fn add_generated_trust_source(
            &self,
            peer: TrustedPeerDescriptor,
            source_kind: GeneratedCommsTrustAuthoritySourceKind,
            private: bool,
        ) -> Result<bool, SendError> {
            let peer_id = peer.peer_id.as_str().to_string();
            let mut trusted = self.trusted.write().await;
            let mut descriptors = self.trusted_descriptors.write().await;
            let mut private_trusted = self.private_trusted.write().await;
            let source_exists = trusted
                .get(&peer_id)
                .is_some_and(|sources| sources.contains(&source_kind));
            let existing_descriptor = descriptors
                .get(&peer_id)
                .and_then(|by_source| by_source.get(&source_kind));
            let private_source_exists = private_trusted
                .get(&peer_id)
                .is_some_and(|sources| sources.contains(&source_kind));
            match (source_exists, existing_descriptor) {
                (true, Some(existing)) if existing == &peer && private_source_exists == private => {
                    return Ok(false);
                }
                (false, None) if !private_source_exists => {}
                _ => {
                    return Err(SendError::Validation(format!(
                        "generated trust source {source_kind:?} for {peer_id} already owns different trust material"
                    )));
                }
            }
            descriptors
                .entry(peer_id.clone())
                .or_default()
                .insert(source_kind, peer);
            let created = trusted
                .entry(peer_id.clone())
                .or_default()
                .insert(source_kind);
            if private {
                private_trusted
                    .entry(peer_id)
                    .or_default()
                    .insert(source_kind);
            }
            Ok(created)
        }

        async fn remove_generated_trust_source(
            &self,
            peer_id: &str,
            source_kind: GeneratedCommsTrustAuthoritySourceKind,
        ) -> bool {
            let mut trusted = self.trusted.write().await;
            let mut descriptors = self.trusted_descriptors.write().await;
            let mut private_trusted = self.private_trusted.write().await;
            if !remove_trust_source(&mut trusted, peer_id, source_kind) {
                return false;
            }
            if let Some(by_source) = descriptors.get_mut(peer_id) {
                by_source.remove(&source_kind);
                if by_source.is_empty() {
                    descriptors.remove(peer_id);
                }
            }
            if let Some(sources) = private_trusted.get_mut(peer_id) {
                sources.remove(&source_kind);
                if sources.is_empty() {
                    private_trusted.remove(peer_id);
                }
            }
            true
        }
    }

    #[async_trait]
    impl CoreCommsRuntime for MockComms {
        fn peer_id(&self) -> Option<meerkat_core::comms::PeerId> {
            Some(self.peer_id)
        }

        fn public_key(&self) -> Option<String> {
            Some(self.key.clone())
        }

        fn public_key_bytes(&self) -> Option<[u8; 32]> {
            Some(self.public_key_bytes)
        }

        fn comms_name(&self) -> Option<String> {
            Some(self.name.clone())
        }

        fn advertised_address(&self) -> Option<String> {
            Some(self.address.clone())
        }

        async fn apply_trust_mutation(
            &self,
            mutation: CommsTrustMutation,
        ) -> Result<CommsTrustMutationResult, SendError> {
            match mutation {
                CommsTrustMutation::AddTrustedPeer { peer, authority } => {
                    self.validate_mob_trust_authority_owner(&authority).await?;
                    authority
                        .validate_public_add(self.peer_id(), &peer)
                        .map_err(SendError::Validation)?;
                    meerkat_core::comms::TrustedPeerDescriptor::validate_pubkey_for_peer_id(
                        peer.peer_id,
                        &peer.pubkey,
                    )
                    .map_err(SendError::Validation)?;
                    let created = self
                        .add_generated_trust_source(peer, authority.trust_row_owner_kind(), false)
                        .await?;
                    Ok(CommsTrustMutationResult::Added { created })
                }
                CommsTrustMutation::RemoveTrustedPeer { peer_id, authority } => {
                    self.validate_mob_trust_authority_owner(&authority).await?;
                    let parsed_peer_id = PeerId::parse(&peer_id)
                        .map_err(|err| SendError::Validation(err.to_string()))?;
                    authority
                        .validate_public_remove(self.peer_id(), parsed_peer_id)
                        .map_err(SendError::Validation)?;
                    let removed = self
                        .remove_generated_trust_source(&peer_id, authority.trust_row_owner_kind())
                        .await;
                    Ok(CommsTrustMutationResult::Removed { removed })
                }
                CommsTrustMutation::AddPrivateTrustedPeer { peer, authority } => {
                    self.validate_mob_trust_authority_owner(&authority).await?;
                    authority
                        .validate_private_add(self.peer_id(), &peer)
                        .map_err(SendError::Validation)?;
                    meerkat_core::comms::TrustedPeerDescriptor::validate_pubkey_for_peer_id(
                        peer.peer_id,
                        &peer.pubkey,
                    )
                    .map_err(SendError::Validation)?;
                    let created = self
                        .add_generated_trust_source(peer, authority.trust_row_owner_kind(), true)
                        .await?;
                    Ok(CommsTrustMutationResult::Added { created })
                }
                CommsTrustMutation::RemovePrivateTrustedPeer { peer_id, authority } => {
                    self.validate_mob_trust_authority_owner(&authority).await?;
                    let parsed_peer_id = PeerId::parse(&peer_id)
                        .map_err(|err| SendError::Validation(err.to_string()))?;
                    authority
                        .validate_private_remove(self.peer_id(), parsed_peer_id)
                        .map_err(SendError::Validation)?;
                    let removed = self
                        .remove_generated_trust_source(&peer_id, authority.trust_row_owner_kind())
                        .await;
                    Ok(CommsTrustMutationResult::Removed { removed })
                }
            }
        }

        async fn trusted_peer_projection_snapshot_for_source(
            &self,
            source_kind: GeneratedCommsTrustAuthoritySourceKind,
        ) -> Result<Vec<TrustedPeerDescriptor>, CommsCapabilityError> {
            let descriptors = self.trusted_descriptors.read().await;
            let private_trusted = self.private_trusted.read().await;
            let mut peers = descriptors
                .iter()
                .filter_map(|(peer_id, by_source)| {
                    if private_trusted
                        .get(peer_id)
                        .is_some_and(|sources| sources.contains(&source_kind))
                    {
                        return None;
                    }
                    by_source.get(&source_kind).cloned()
                })
                .collect::<Vec<_>>();
            peers.sort_by(|left, right| left.peer_id.as_str().cmp(&right.peer_id.as_str()));
            Ok(peers)
        }

        async fn install_generated_mob_trust_owner(
            &self,
            owner: Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<(), SendError> {
            let mut expected = self.mob_machine_trust_owner.write().await;
            if let Some(existing) = expected.as_ref() {
                if Arc::ptr_eq(existing, &owner) {
                    return Ok(());
                }
                return Err(SendError::Validation(
                    "target runtime is already bound to a different generated MobMachine trust owner"
                        .to_string(),
                ));
            }
            *expected = Some(owner);
            Ok(())
        }

        async fn validate_recovered_generated_mob_trust_owner(
            &self,
            owner: Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<(), SendError> {
            let expected = self.mob_machine_trust_owner.read().await;
            if let Some(existing) = expected.as_ref()
                && !Arc::ptr_eq(existing, &owner)
            {
                return Err(SendError::Validation(
                    "target runtime is already bound to a different generated MobMachine trust owner"
                        .to_string(),
                ));
            }
            Ok(())
        }

        async fn install_recovered_generated_mob_trust_owner(
            &self,
            owner: Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<(), SendError> {
            let mut expected = self.mob_machine_trust_owner.write().await;
            if let Some(existing) = expected.as_ref() {
                if Arc::ptr_eq(existing, &owner) {
                    return Ok(());
                }
                return Err(SendError::Validation(
                    "target runtime is already bound to a different generated MobMachine trust owner"
                        .to_string(),
                ));
            }
            *expected = Some(owner);
            Ok(())
        }

        async fn add_private_trusted_peer(
            &self,
            _peer: meerkat_core::comms::TrustedPeerDescriptor,
        ) -> Result<(), SendError> {
            Err(SendError::Unsupported(
                "add_private_trusted_peer requires apply_trust_mutation authority".to_string(),
            ))
        }

        async fn send(&self, _cmd: CommsCommand) -> Result<SendReceipt, SendError> {
            Ok(SendReceipt::InputAccepted {
                interaction_id: meerkat_core::interaction::InteractionId(uuid::Uuid::nil()),
                stream_reserved: false,
            })
        }

        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            self.notify.clone()
        }

        async fn drain_peer_input_candidates(&self) -> Vec<PeerInputCandidate> {
            Vec::new()
        }
    }

    struct MockSessionActor {
        comms: Arc<MockComms>,
        witness: meerkat_session::LiveSessionActorWitness,
    }

    struct MockSessionSvc {
        sessions: RwLock<HashMap<SessionId, MockSessionActor>>,
        actor_registry: meerkat_session::LiveSessionActorRegistry,
        persisted_sessions: RwLock<HashMap<SessionId, Session>>,
        archive_failures: RwLock<HashMap<SessionId, String>>,
        keep_alive_notifiers: RwLock<HashMap<SessionId, Arc<Notify>>>,
        last_start_turn: RwLock<Option<(SessionId, String)>>,
        counter: AtomicU64,
        start_turn_delay_ms: AtomicU64,
        start_turn_calls: AtomicU64,
        /// Counting-wrapper guard state: full-document reads through
        /// `load_persisted_session`.
        persisted_full_loads: AtomicU64,
        /// Counting-wrapper guard state: metadata-only reads through
        /// `load_persisted_session_metadata`.
        persisted_metadata_loads: AtomicU64,
        runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    }

    impl MockSessionSvc {
        fn new() -> Self {
            Self {
                sessions: RwLock::new(HashMap::new()),
                actor_registry: meerkat_session::LiveSessionActorRegistry::default(),
                persisted_sessions: RwLock::new(HashMap::new()),
                archive_failures: RwLock::new(HashMap::new()),
                keep_alive_notifiers: RwLock::new(HashMap::new()),
                last_start_turn: RwLock::new(None),
                counter: AtomicU64::new(0),
                start_turn_delay_ms: AtomicU64::new(0),
                start_turn_calls: AtomicU64::new(0),
                persisted_full_loads: AtomicU64::new(0),
                persisted_metadata_loads: AtomicU64::new(0),
                runtime_adapter: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
            }
        }

        async fn cold_restart(&self) -> Self {
            let actor_registry = meerkat_session::LiveSessionActorRegistry::default();
            let sessions = self
                .sessions
                .read()
                .await
                .iter()
                .map(|(session_id, actor)| {
                    let witness_slot = meerkat_session::LiveSessionActorWitnessSlot::default();
                    let witness =
                        register_live_actor(&actor_registry, &witness_slot, session_id.clone())
                            .expect("cold-restart fixture actor should register");
                    (
                        session_id.clone(),
                        MockSessionActor {
                            comms: Arc::new(MockComms::new(&actor.comms.name)),
                            witness,
                        },
                    )
                })
                .collect();
            let persisted_sessions = self.persisted_sessions.read().await.clone();
            let archive_failures = self.archive_failures.read().await.clone();
            let keep_alive_notifiers = self
                .keep_alive_notifiers
                .read()
                .await
                .keys()
                .cloned()
                .map(|session_id| (session_id, Arc::new(Notify::new())))
                .collect();
            Self {
                sessions: RwLock::new(sessions),
                actor_registry,
                persisted_sessions: RwLock::new(persisted_sessions),
                archive_failures: RwLock::new(archive_failures),
                keep_alive_notifiers: RwLock::new(keep_alive_notifiers),
                last_start_turn: RwLock::new(None),
                counter: AtomicU64::new(self.counter.load(Ordering::Relaxed)),
                start_turn_delay_ms: AtomicU64::new(
                    self.start_turn_delay_ms.load(Ordering::Relaxed),
                ),
                start_turn_calls: AtomicU64::new(0),
                persisted_full_loads: AtomicU64::new(0),
                persisted_metadata_loads: AtomicU64::new(0),
                runtime_adapter: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
            }
        }

        async fn create_session_with_actor_slot(
            &self,
            req: CreateSessionRequest,
            actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
        ) -> Result<RunResult, SessionError> {
            let build = req.build;
            let mut persisted_session = build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let sid = persisted_session.id().clone();
            let n = self.counter.fetch_add(1, Ordering::Relaxed);
            let is_keep_alive = build
                .as_ref()
                .map(|build| build.keep_alive)
                .unwrap_or(false);
            let actor_materialization_permit =
                begin_live_actor_materialization(build.as_ref().and_then(|build| {
                    match &build.runtime_build_mode {
                        meerkat_core::RuntimeBuildMode::SessionOwned(bindings) => Some(bindings),
                        meerkat_core::RuntimeBuildMode::StandaloneEphemeral => None,
                    }
                }))?;
            let name = build
                .as_ref()
                .and_then(|build| build.comms_name.clone())
                .unwrap_or_else(|| format!("s-{n}"));
            if persisted_session.session_metadata().is_none() {
                persisted_session
                    .set_session_metadata(SessionMetadata {
                        schema_version: meerkat_core::session_metadata_schema_version(),
                        model: req.model.clone(),
                        max_tokens: req.max_tokens.unwrap_or(4096),
                        structured_output_retries: build
                            .as_ref()
                            .and_then(|options| options.structured_output_retries)
                            .unwrap_or_else(
                                meerkat_core::config::default_structured_output_retries,
                            ),
                        provider: build
                            .as_ref()
                            .and_then(|options| options.provider)
                            .unwrap_or(Provider::Anthropic),
                        self_hosted_server_id: build
                            .as_ref()
                            .and_then(|options| options.self_hosted_server_id.clone()),
                        provider_params: build
                            .as_ref()
                            .and_then(|options| options.provider_params.clone()),
                        tooling: SessionTooling {
                            builtins: build
                                .as_ref()
                                .map(|options| options.override_builtins)
                                .unwrap_or_default(),
                            shell: build
                                .as_ref()
                                .map(|options| options.override_shell)
                                .unwrap_or_default(),
                            comms: build
                                .as_ref()
                                .map(|options| options.override_comms)
                                .unwrap_or_default(),
                            mob: build
                                .as_ref()
                                .map(|options| options.override_mob)
                                .unwrap_or_default(),
                            memory: build
                                .as_ref()
                                .map(|options| options.override_memory)
                                .unwrap_or_default(),
                            schedule: build
                                .as_ref()
                                .map(|options| options.override_schedule)
                                .unwrap_or_default(),
                            workgraph: build
                                .as_ref()
                                .map(|options| options.override_workgraph)
                                .unwrap_or_default(),
                            image_generation: build
                                .as_ref()
                                .map(|options| options.override_image_generation)
                                .unwrap_or_default(),
                            web_search: build
                                .as_ref()
                                .map(|options| options.override_web_search)
                                .unwrap_or_default(),
                            tool_access_policy: build
                                .as_ref()
                                .and_then(|options| options.tool_access_policy.clone()),
                            active_skills: build
                                .as_ref()
                                .and_then(|options| options.preload_skills.clone()),
                        },
                        keep_alive: is_keep_alive,
                        comms_name: build
                            .as_ref()
                            .and_then(|options| options.comms_name.clone()),
                        peer_meta: build.as_ref().and_then(|options| options.peer_meta.clone()),
                        realm_id: build.as_ref().and_then(|options| options.realm_id.clone()),
                        instance_id: build
                            .as_ref()
                            .and_then(|options| options.instance_id.clone()),
                        backend: build.as_ref().and_then(|options| {
                            options.backend.map(|kind| kind.as_str().to_string())
                        }),
                        config_generation: build
                            .as_ref()
                            .and_then(|options| options.config_generation),
                        auth_binding: build
                            .as_ref()
                            .and_then(|options| options.auth_binding.clone()),
                        mob_member_binding: build
                            .as_ref()
                            .and_then(|options| options.mob_member_binding.clone()),
                    })
                    .map_err(|error| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("failed to seed mock persisted session metadata: {error}"),
                        ))
                    })?;
            }
            let comms = Arc::new(MockComms::new(&name));
            let actor_witness = {
                let mut sessions = self.sessions.write().await;
                if sessions.contains_key(&sid) {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "live session actor is already registered: {sid}"
                        )),
                    ));
                }
                let witness =
                    register_live_actor(&self.actor_registry, actor_witness_slot, sid.clone())?;
                sessions.insert(
                    sid.clone(),
                    MockSessionActor {
                        comms,
                        witness: witness.clone(),
                    },
                );
                witness
            };
            commit_live_actor_materialization_or_discard(
                self,
                actor_materialization_permit,
                &actor_witness,
            )
            .await?;
            self.persisted_sessions
                .write()
                .await
                .insert(sid.clone(), persisted_session);
            if is_keep_alive {
                self.keep_alive_notifiers
                    .write()
                    .await
                    .insert(sid.clone(), Arc::new(Notify::new()));
            }
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: sid,
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_turn_delay_ms(&self, delay_ms: u64) {
            self.start_turn_delay_ms.store(delay_ms, Ordering::Relaxed);
        }

        fn start_turn_call_count(&self) -> u64 {
            self.start_turn_calls.load(Ordering::Relaxed)
        }

        async fn last_start_turn(&self) -> Option<(SessionId, String)> {
            self.last_start_turn.read().await.clone()
        }

        async fn insert_persisted_session(&self, session: Session) {
            self.persisted_sessions
                .write()
                .await
                .insert(session.id().clone(), session);
        }

        async fn fail_archive(&self, id: SessionId, reason: impl Into<String>) {
            self.archive_failures
                .write()
                .await
                .insert(id, reason.into());
        }

        async fn clear_archive_failure(&self, id: &SessionId) {
            self.archive_failures.write().await.remove(id);
        }

        async fn session_exists(&self, id: &SessionId) -> bool {
            self.sessions.read().await.contains_key(id)
                || self.persisted_sessions.read().await.contains_key(id)
        }

        async fn retire_with_machine_archive_authority(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            if !self.session_exists(session_id).await
                && !self.runtime_adapter.contains_session(session_id).await
            {
                return Ok(());
            }

            let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
            match meerkat_runtime::RuntimeControlPlane::retire(&*self.runtime_adapter, &runtime_id)
                .await
            {
                Ok(_) => Ok(()),
                Err(meerkat_runtime::RuntimeControlPlaneError::NotFound(_)) => {
                    self.runtime_adapter
                        .register_session(session_id.clone())
                        .await
                        .expect("register session");
                    meerkat_runtime::RuntimeControlPlane::retire(
                        &*self.runtime_adapter,
                        &runtime_id,
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("machine archive retire failed after registration: {error}"),
                        ))
                    })
                }
                Err(error) => Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "machine archive retire failed: {error}"
                    )),
                )),
            }
        }
    }

    #[async_trait]
    impl SessionService for MockSessionSvc {
        async fn create_session(
            &self,
            req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            let actor_witness_slot = meerkat_session::LiveSessionActorWitnessSlot::default();
            self.create_session_with_actor_slot(req, &actor_witness_slot)
                .await
        }

        async fn start_turn(
            &self,
            id: &SessionId,
            req: StartTurnRequest,
        ) -> Result<RunResult, SessionError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            self.start_turn_calls.fetch_add(1, Ordering::Relaxed);
            *self.last_start_turn.write().await = Some((id.clone(), req.prompt.text_content()));
            let delay_ms = self.start_turn_delay_ms.load(Ordering::Relaxed);
            if delay_ms > 0 {
                sleep(Duration::from_millis(delay_ms)).await;
            }
            // Block only for keep-alive sessions (notifier registered at create time).
            if let Some(notifier) = self.keep_alive_notifiers.read().await.get(id).cloned() {
                notifier.notified().await;
            }
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
            if let Some(notifier) = self.keep_alive_notifiers.read().await.get(id).cloned() {
                notifier.notify_waiters();
            }
            Ok(())
        }

        async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
            Ok(self.sessions.read().await.contains_key(id))
        }

        async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
            if !self.sessions.read().await.contains_key(id) {
                if self.persisted_sessions.read().await.contains_key(id) {
                    return Ok(SessionView {
                        state: SessionInfo {
                            session_id: id.clone(),
                            created_at: SystemTime::now(),
                            updated_at: SystemTime::now(),
                            message_count: 0,
                            is_active: false,
                            model: "claude-sonnet-4-5".to_string(),
                            provider: Provider::Anthropic,
                            last_assistant_text: None,
                            labels: Default::default(),
                        },
                        billing: SessionUsage {
                            total_tokens: 0,
                            usage: Usage::default(),
                        },
                    });
                }
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(SessionView {
                state: SessionInfo {
                    session_id: id.clone(),
                    created_at: SystemTime::now(),
                    updated_at: SystemTime::now(),
                    message_count: 0,
                    is_active: false,
                    model: "claude-sonnet-4-5".to_string(),
                    provider: Provider::Anthropic,
                    last_assistant_text: None,
                    labels: Default::default(),
                },
                billing: SessionUsage {
                    total_tokens: 0,
                    usage: Usage::default(),
                },
            })
        }

        async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
            Ok(self
                .sessions
                .read()
                .await
                .keys()
                .map(|id| SessionSummary {
                    session_id: id.clone(),
                    created_at: SystemTime::now(),
                    updated_at: SystemTime::now(),
                    message_count: 0,
                    total_tokens: 0,
                    is_active: false,
                    labels: Default::default(),
                })
                .collect())
        }

        async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
            if let Some(reason) = self.archive_failures.read().await.get(id).cloned() {
                return Err(SessionError::Unsupported(reason));
            }
            let removed_live = {
                let mut sessions = self.sessions.write().await;
                let had_live_actor = sessions.contains_key(id);
                let removed_registry_actor = self.actor_registry.remove_current(id);
                if had_live_actor && !removed_registry_actor {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "live actor registry omitted current session {id} during archive"
                        )),
                    ));
                }
                sessions.remove(id).is_some()
            };
            let removed_persisted = self.persisted_sessions.write().await.remove(id).is_some();
            if let Some(notifier) = self.keep_alive_notifiers.write().await.remove(id) {
                notifier.notify_waiters();
            }
            if removed_live || removed_persisted {
                Ok(())
            } else {
                Err(SessionError::NotFound { id: id.clone() })
            }
        }
    }

    #[async_trait]
    impl SessionServiceCommsExt for MockSessionSvc {
        async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
            self.sessions
                .read()
                .await
                .get(session_id)
                .map(|actor| actor.comms.clone() as Arc<dyn CoreCommsRuntime>)
        }

        async fn event_injector(
            &self,
            session_id: &SessionId,
        ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
            if !self.sessions.read().await.contains_key(session_id) {
                return None;
            }
            Some(Arc::new(MockInjector))
        }

        async fn interaction_event_injector(
            &self,
            session_id: &SessionId,
        ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
            if !self.sessions.read().await.contains_key(session_id) {
                return None;
            }
            Some(Arc::new(MockInjector))
        }
    }

    #[async_trait]
    impl SessionServiceControlExt for MockSessionSvc {
        async fn append_system_context(
            &self,
            id: &SessionId,
            _req: AppendSystemContextRequest,
        ) -> Result<AppendSystemContextResult, SessionControlError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() }.into());
            }
            Ok(AppendSystemContextResult {
                status: AppendSystemContextStatus::Staged,
            })
        }
    }

    #[async_trait]
    impl SessionServiceHistoryExt for MockSessionSvc {
        async fn read_history(
            &self,
            id: &SessionId,
            query: SessionHistoryQuery,
        ) -> Result<SessionHistoryPage, SessionError> {
            if self.sessions.read().await.contains_key(id)
                || self.persisted_sessions.read().await.contains_key(id)
            {
                return Ok(SessionHistoryPage::from_messages(id.clone(), &[], query));
            }
            Err(SessionError::NotFound { id: id.clone() })
        }
    }

    #[async_trait]
    impl MobSessionService for MockSessionSvc {
        async fn create_session_under_runtime_turn_boundary(
            &self,
            req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            <Self as SessionService>::create_session(self, req).await
        }

        async fn create_session_with_actor_witness_under_runtime_turn_boundary(
            &self,
            req: CreateSessionRequest,
            actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
        ) -> Result<RunResult, SessionError> {
            self.create_session_with_actor_slot(req, actor_witness_slot)
                .await
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.archive_with_mob_lifecycle_authority(session_id).await
        }

        async fn discard_live_session_under_runtime_turn_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.discard_live_session(session_id).await
        }

        async fn discard_live_session_actor_under_runtime_turn_boundary(
            &self,
            witness: &meerkat_session::LiveSessionActorWitness,
        ) -> Result<bool, SessionError> {
            let removed = {
                let mut sessions = self.sessions.write().await;
                let Some(actor) = sessions.get(witness.session_id()) else {
                    return Ok(false);
                };
                if !actor.witness.eq(witness) {
                    return Ok(false);
                }
                if !self.actor_registry.compare_remove(witness) {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "live actor registry rejected current witness for {}",
                            witness.session_id()
                        )),
                    ));
                }
                sessions.remove(witness.session_id()).is_some()
            };
            if removed {
                if let Some(notifier) = self
                    .keep_alive_notifiers
                    .write()
                    .await
                    .remove(witness.session_id())
                {
                    notifier.notify_waiters();
                }
            }
            Ok(removed)
        }

        fn supports_persistent_sessions(&self) -> bool {
            true
        }

        async fn live_session_actor_registered(
            &self,
            session_id: &SessionId,
        ) -> Result<bool, SessionError> {
            Ok(self.actor_registry.contains(session_id))
        }

        fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
            Some(self.runtime_adapter.clone())
        }

        async fn archive_with_mob_lifecycle_authority(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.retire_with_machine_archive_authority(session_id)
                .await?;
            SessionService::archive(self, session_id).await
        }

        async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
            {
                let mut sessions = self.sessions.write().await;
                let had_live_actor = sessions.contains_key(session_id);
                let removed_registry_actor = self.actor_registry.remove_current(session_id);
                if had_live_actor && !removed_registry_actor {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "live actor registry omitted current session {session_id} during discard"
                        )),
                    ));
                }
                sessions.remove(session_id);
            }
            if let Some(notifier) = self.keep_alive_notifiers.write().await.remove(session_id) {
                notifier.notify_waiters();
            }
            Ok(())
        }

        async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
            true
        }

        async fn load_persisted_session(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<Session>, SessionError> {
            self.persisted_full_loads.fetch_add(1, Ordering::Relaxed);
            Ok(self
                .persisted_sessions
                .read()
                .await
                .get(session_id)
                .cloned())
        }

        async fn load_persisted_session_metadata(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
            self.persisted_metadata_loads
                .fetch_add(1, Ordering::Relaxed);
            let Some(session) = self
                .persisted_sessions
                .read()
                .await
                .get(session_id)
                .cloned()
            else {
                return Ok(None);
            };
            meerkat_core::PersistedSessionMetadataView::try_from_session(&session)
                .map(Some)
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "session {session_id} durable metadata failed typed restore: {err}"
                    )))
                })
        }

        async fn apply_runtime_turn(
            &self,
            session_id: &SessionId,
            run_id: meerkat_core::RunId,
            req: StartTurnRequest,
            boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
            contributing_input_ids: Vec<meerkat_core::InputId>,
        ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
            let run_result = <Self as SessionService>::start_turn(self, session_id, req).await?;
            Ok(
                meerkat_core::lifecycle::core_executor::CoreApplyOutput::with_run_result(
                    meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft {
                        run_id,
                        boundary,
                        contributing_input_ids,
                        conversation_digest: None,
                        message_count: 0,
                    },
                    None,
                    run_result,
                ),
            )
        }
    }

    #[tokio::test]
    async fn local_session_service_persists_appended_context() {
        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        let result = service
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                        "Remember the customer preference.".to_string(),
                    ),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-1".to_string()),
                    source_kind: meerkat_core::session::SystemContextSource::Normal,
                    peer_response_terminal: None,
                },
            )
            .await
            .expect("append context");
        assert_eq!(result.status, AppendSystemContextStatus::Staged);
        let pending = service.pending_context.read().await;
        assert_eq!(pending.get(&session_id).map(std::vec::Vec::len), Some(1));
    }

    #[tokio::test]
    async fn local_session_service_consumes_staged_context_on_next_turn() -> Result<(), String> {
        use futures::StreamExt;

        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        service
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                        "Remember the customer preference.".to_string(),
                    ),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-1".to_string()),
                    source_kind: meerkat_core::session::SystemContextSource::Normal,
                    peer_response_terminal: None,
                },
            )
            .await
            .expect("append context");

        let mut stream =
            meerkat_mob::MobSessionService::subscribe_session_events(&service, &session_id)
                .await
                .expect("subscribe events");
        service
            .start_turn(
                &session_id,
                StartTurnRequest {
                    injected_context: Vec::new(),
                    prompt: "hello".to_string().into(),
                    system_prompt: None,
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                        HandlingMode::Queue,
                        None,
                        Vec::new(),
                        None,
                    ),
                },
            )
            .await
            .expect("start turn");

        let first = stream.next().await.expect("first event");
        match first.payload {
            AgentEvent::RunStarted { input, .. } => {
                let prompt = input
                    .content()
                    .expect("content-bearing run input")
                    .text_content();
                assert!(prompt.contains("Remember the customer preference."));
                assert!(prompt.contains("hello"));
            }
            other => return Err(format!("expected RunStarted, got {other:?}")),
        }

        let pending = service.pending_context.read().await;
        assert_eq!(pending.get(&session_id).map(std::vec::Vec::len), Some(0));
        Ok(())
    }

    #[tokio::test]
    async fn local_session_service_preserves_multimodal_prompt_when_staging_context()
    -> Result<(), String> {
        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        service
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                        "Remember the picture.".to_string(),
                    ),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-image".to_string()),
                    source_kind: meerkat_core::session::SystemContextSource::Normal,
                    peer_response_terminal: None,
                },
            )
            .await
            .expect("append context");

        let prompt = meerkat_core::types::ContentInput::Blocks(vec![
            meerkat_core::types::ContentBlock::Text {
                text: "Look at this.".to_string(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "abc123".into(),
            },
        ]);

        let staged_context = {
            let mut pending = service.pending_context.write().await;
            let entry = pending.entry(session_id.clone()).or_default();
            std::mem::take(entry)
        };

        let effective_prompt = if staged_context.is_empty() {
            prompt.clone()
        } else {
            let staged_sections = staged_context
                .iter()
                .map(|append| match append.source.as_deref() {
                    Some(source) => format!("[SYSTEM CONTEXT:{source}] {}", append.text()),
                    None => format!("[SYSTEM CONTEXT] {}", append.text()),
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut blocks = vec![meerkat_core::types::ContentBlock::Text {
                text: format!("{staged_sections}\n\n"),
            }];
            blocks.extend(prompt.into_blocks());
            meerkat_core::types::ContentInput::Blocks(blocks)
        };

        match effective_prompt {
            meerkat_core::types::ContentInput::Blocks(blocks) => {
                assert_eq!(blocks.len(), 3);
                assert!(matches!(
                    blocks.get(1),
                    Some(meerkat_core::types::ContentBlock::Text { text }) if text == "Look at this."
                ));
                assert!(matches!(
                    blocks.get(2),
                    Some(meerkat_core::types::ContentBlock::Image { media_type, .. }) if media_type == "image/png"
                ));
            }
            other => return Err(format!("expected multimodal blocks, got {other:?}")),
        }
        Ok(())
    }

    #[tokio::test]
    async fn local_session_service_archive_drops_staged_context() {
        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        service
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                        "Remember the customer preference.".to_string(),
                    ),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-archive".to_string()),
                    source_kind: meerkat_core::session::SystemContextSource::Normal,
                    peer_response_terminal: None,
                },
            )
            .await
            .expect("append context");

        service.archive(&session_id).await.expect("archive session");

        let pending = service.pending_context.read().await;
        assert!(
            !pending.contains_key(&session_id),
            "archive must drop unapplied staged context"
        );
    }

    #[tokio::test]
    async fn local_session_service_stream_emits_events_on_turn() {
        use futures::StreamExt;

        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        let mut stream =
            meerkat_mob::MobSessionService::subscribe_session_events(&service, &session_id)
                .await
                .expect("subscribe events");
        let turn = service
            .start_turn(
                &session_id,
                StartTurnRequest {
                    injected_context: Vec::new(),
                    prompt: "hello".to_string().into(),
                    system_prompt: None,
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                        HandlingMode::Queue,
                        None,
                        Vec::new(),
                        None,
                    ),
                },
            )
            .await;
        assert!(turn.is_ok());

        let first = stream.next().await.expect("first event");
        assert!(matches!(first.payload, AgentEvent::RunStarted { .. }));
        let second = stream.next().await.expect("second event");
        assert!(matches!(second.payload, AgentEvent::TurnStarted { .. }));
    }

    fn mk_call<'a>(name: &'a str, args: &'a serde_json::value::RawValue) -> ToolCallView<'a> {
        ToolCallView {
            id: "t1",
            name,
            args,
        }
    }

    async fn call_tool(
        d: &MobMcpDispatcher,
        name: &str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
        let out = d.dispatch(mk_call(name, &raw)).await.expect("tool call");
        serde_json::from_str(&out.result.text_content()).expect("tool json")
    }

    async fn call_tool_err(d: &MobMcpDispatcher, name: &str, args: serde_json::Value) -> ToolError {
        let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
        d.dispatch(mk_call(name, &raw))
            .await
            .expect_err("tool call should fail")
    }

    #[test]
    fn test_map_destroy_err_preserves_incomplete_error_data() {
        let raw = serde_json::value::RawValue::from_string("{}".to_string()).expect("raw args");
        let call = mk_call("mob_lifecycle", &raw);
        let mut report = meerkat_mob::MobDestroyReport::default();
        report.errors.push("worker: archive failed".to_string());

        let error = map_destroy_err(call, MobMcpDestroyError::Incomplete { report });
        let data = error
            .structured_data()
            .expect("incomplete destroy should include structured data");

        assert_eq!(
            data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );
        assert_eq!(
            data.get("retryable").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert!(data.get("destroy_report").is_some());
    }

    #[tokio::test]
    async fn test_dispatcher_exposes_expected_tools() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);
        let tools = d.tools();
        let tool_names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(
            tool_names,
            vec![
                "mob_create",
                "mob_list",
                "mob_lifecycle",
                "mob_events",
                "mob_run_flow",
                "mob_flow_status",
                "mob_cancel_flow",
                "mob_spawn_member",
                "mob_retire_member",
                "mob_list_members",
                "mob_wire",
                "mob_respawn",
                "mob_force_cancel",
                "mob_member_status",
                "mob_wait_kickoff",
                "mob_wait_ready",
            ]
        );
    }

    #[tokio::test]
    async fn test_mob_lifecycle_rejects_unknown_action_at_contract_boundary() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        call_tool(
            &d,
            "mob_create",
            json!({"definition":{"id":"typed_agent_lifecycle_rejects_unknown","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
        )
        .await;

        let error = call_tool_err(
            &d,
            "mob_lifecycle",
            json!({"mob_id": "typed_agent_lifecycle_rejects_unknown", "action": "explode"}),
        )
        .await;
        let ToolError::InvalidArguments { reason, .. } = error else {
            panic!("unknown lifecycle action must be InvalidArguments, got: {error:?}");
        };
        assert!(
            reason.contains("unknown variant") && !reason.contains("unknown lifecycle action"),
            "unexpected error: {reason}"
        );

        let status = call_tool(
            &d,
            "mob_list",
            json!({"mob_id": "typed_agent_lifecycle_rejects_unknown"}),
        )
        .await;
        assert_eq!(status["status"], "Running");

        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": "typed_agent_lifecycle_rejects_unknown", "action": "destroy"}),
        )
        .await;
    }

    #[tokio::test]
    async fn test_mob_lifecycle_accepts_typed_contract_params() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        call_tool(
            &d,
            "mob_create",
            json!({"definition":{"id":"typed_agent_lifecycle_complete","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
        )
        .await;

        let params = MobLifecycleParams {
            mob_id: "typed_agent_lifecycle_complete".to_string(),
            action: WireMobLifecycleAction::Complete,
        };
        let payload = serde_json::to_value(&params).expect("typed lifecycle params serialize");
        let result = call_tool(&d, "mob_lifecycle", payload).await;

        assert_eq!(result["ok"], true);

        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": "typed_agent_lifecycle_complete", "action": "destroy"}),
        )
        .await;
    }

    #[test]
    fn test_mob_wire_schema_uses_external_binding_without_raw_peer_atoms() {
        let tools = tools_list();
        let schema = tools
            .iter()
            .find(|tool| tool["name"] == "mob_wire")
            .and_then(|tool| tool.get("inputSchema"))
            .expect("mob_wire schema present");
        let schema_text = serde_json::to_string(schema).expect("schema should encode");

        assert!(schema_text.contains("external_binding"));
        assert!(
            !schema_text.contains("\"peer_id\"") && !schema_text.contains("\"pubkey\""),
            "mob_wire schema must not expose raw comms identity atoms: {schema_text}"
        );
    }

    #[tokio::test]
    async fn test_owns_persisted_session_requires_actual_roster_membership() {
        let svc = Arc::new(MockSessionSvc::new());
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = svc.clone();
        let state = Arc::new(MobMcpState::new(
            session_service,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let dispatcher = MobMcpDispatcher::new(Arc::clone(&state));

        call_tool(
            &dispatcher,
            "mob_create",
            json!({
                "definition": {
                    "id": "team",
                    "orchestrator": {"profile": "lead"},
                    "profiles": {
                        "lead": {
                            "model": "claude-opus-4-8",
                            "tools": {"comms": true, "mob": true},
                            "external_addressable": true
                        }
                    },
                    "wiring": {"auto_wire_orchestrator": false, "role_wiring": []},
                    "skills": {},
                    "backend": {"default": "session"}
                }
            }),
        )
        .await;

        let mut spoofed = Session::new();
        let spoofed_id = spoofed.id().clone();
        let _ = spoofed.set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: SessionTooling {
                comms: ToolCategoryOverride::Enable,
                ..SessionTooling::default()
            },
            keep_alive: false,
            // Spoof: a session that merely carries the comms_name shape but no
            // typed durable binding must NOT be owned. Identity is the typed
            // mob_member_binding, not the routing-name string.
            comms_name: Some("team/reviewer/alice".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        });
        svc.insert_persisted_session(spoofed).await;

        assert!(
            !state.owns_persisted_bridge_session(&spoofed_id).await,
            "persisted session routing must verify real mob membership via the typed binding, not the comms_name shape"
        );
    }

    #[tokio::test]
    async fn test_owns_persisted_bridge_session_accepts_mob_marked_session_without_live_handle() {
        let svc = Arc::new(MockSessionSvc::new());
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = svc.clone();
        let state = Arc::new(MobMcpState::new(
            session_service,
            meerkat_mob::MobControlPrincipal::Owner,
        ));

        let mut persisted = Session::new();
        let persisted_id = persisted.id().clone();
        let _ = persisted.set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: SessionTooling {
                comms: ToolCategoryOverride::Enable,
                ..SessionTooling::default()
            },
            keep_alive: false,
            // Transport routing name + discovery metadata (kept as-is); the
            // realm now uses the canonical dot form via the shared helper.
            comms_name: Some("team/reviewer/alice".to_string()),
            peer_meta: Some(
                PeerMeta::default()
                    .with_label("mob_id", "team")
                    .with_label("role", "reviewer")
                    .with_label("meerkat_id", "alice"),
            ),
            realm_id: Some(meerkat_core::RealmId::parse("mob.team").unwrap()),
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            // Typed durable identity — this is what ownership routing reads.
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: "team".to_string(),
                role: "reviewer".to_string(),
                member: "alice".to_string(),
            }),
        });
        svc.insert_persisted_session(persisted).await;

        assert!(
            state.owns_persisted_bridge_session(&persisted_id).await,
            "persisted mob members must still route through mob ownership after restart even before a live handle is rehydrated"
        );
    }

    /// Counting-wrapper guard (mobkit ask-24 clause 3): the persisted
    /// ownership probe reads the metadata seam ONLY — exactly one
    /// metadata-view load and ZERO full session-document loads. Regresses if
    /// `owns_persisted_bridge_session` (or anything on its path) reaches for
    /// `load_persisted_session` again.
    #[tokio::test]
    async fn test_owns_persisted_bridge_session_reads_metadata_seam_without_full_load() {
        let svc = Arc::new(MockSessionSvc::new());
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = svc.clone();
        let state = Arc::new(MobMcpState::new(
            session_service,
            meerkat_mob::MobControlPrincipal::Owner,
        ));

        let mut persisted = Session::new();
        let persisted_id = persisted.id().clone();
        let _ = persisted.set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: SessionTooling {
                comms: ToolCategoryOverride::Enable,
                ..SessionTooling::default()
            },
            keep_alive: false,
            comms_name: Some("team/reviewer/alice".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: "team".to_string(),
                role: "reviewer".to_string(),
                member: "alice".to_string(),
            }),
        });
        svc.insert_persisted_session(persisted).await;

        assert!(
            state.owns_persisted_bridge_session(&persisted_id).await,
            "the metadata seam must resolve ownership for a bound persisted session"
        );
        assert_eq!(
            svc.persisted_full_loads.load(Ordering::Relaxed),
            0,
            "the ownership probe must not materialize the full session document"
        );
        assert_eq!(
            svc.persisted_metadata_loads.load(Ordering::Relaxed),
            1,
            "the ownership probe reads exactly one metadata view"
        );
    }

    #[tokio::test]
    async fn test_retire_member_by_bridge_session_id_falls_back_to_archiving_persisted_member() {
        let svc = Arc::new(MockSessionSvc::new());
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = svc.clone();
        let state = Arc::new(MobMcpState::new(
            session_service,
            meerkat_mob::MobControlPrincipal::Owner,
        ));

        let mut persisted = Session::new();
        let persisted_id = persisted.id().clone();
        let _ = persisted.set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: SessionTooling {
                comms: ToolCategoryOverride::Enable,
                ..SessionTooling::default()
            },
            keep_alive: false,
            comms_name: Some("team/reviewer/alice".to_string()),
            peer_meta: Some(
                PeerMeta::default()
                    .with_label("mob_id", "team")
                    .with_label("role", "reviewer")
                    .with_label("meerkat_id", "alice"),
            ),
            realm_id: Some(meerkat_core::RealmId::parse("mob.team").unwrap()),
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            // Ownership routing reads the typed binding directly.
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: "team".to_string(),
                role: "reviewer".to_string(),
                member: "alice".to_string(),
            }),
        });
        svc.insert_persisted_session(persisted).await;

        state
            .retire_member_by_bridge_session_id(&persisted_id)
            .await
            .expect("persisted mob sessions should archive cleanly even without a live handle");
        assert!(
            svc.load_persisted_session(&persisted_id)
                .await
                .expect("load persisted")
                .is_none(),
            "archive fallback must remove the persisted session snapshot"
        );
    }

    #[tokio::test]
    async fn test_retire_member_unknown_bridge_session_returns_typed_recovery_variant() {
        // A bridge session that lives in no live roster, is not service-reported,
        // and is not persisted must surface the TYPED recovery class
        // (BridgeSessionNotInLiveAuthority) rather than an Internal string the
        // consumer has to re-parse by message prefix.
        let svc = Arc::new(MockSessionSvc::new());
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = svc.clone();
        let state = Arc::new(MobMcpState::new(
            session_service,
            meerkat_mob::MobControlPrincipal::Owner,
        ));

        let unknown = SessionId::new();
        let err = state
            .retire_member_by_bridge_session_id(&unknown)
            .await
            .expect_err("unknown bridge session must not retire successfully");
        match err {
            MobError::BridgeSessionNotInLiveAuthority { bridge_session_id } => {
                assert_eq!(bridge_session_id, unknown.to_string());
            }
            other => panic!("expected typed BridgeSessionNotInLiveAuthority, got {other:?}"),
        }
    }

    fn flow_enabled_definition() -> serde_json::Value {
        json!({
            "id": "flow-mob",
            "orchestrator": {
                "profile": "lead"
            },
            "profiles": {
                "lead": {
                    "model": "claude-opus-4-8",
                    "external_addressable": true,
                    "peer_description": "Lead",
                    "tools": {
                        "comms": true,
                        "mob": true
                    }
                },
                "worker": {
                    "model": "claude-sonnet-4-5",
                    "external_addressable": false,
                    "peer_description": "Worker",
                    "tools": {
                        "comms": true,
                        "mob": true
                    }
                }
            },
            "wiring": {
                "auto_wire_orchestrator": false,
                "role_wiring": []
            },
            "backend": {
                "default": "session"
            },
            "flows": {
                "demo": {
                    "description": "demo flow",
                    "steps": {
                        "start": {
                            "role": "worker",
                            "message": "run demo",
                            // Explicit: the cancel test depends on the mock's
                            // non-JSON "ok" turn output remaining a parse
                            // fault; the schema-aware omitted default would
                            // resolve to text and complete the step.
                            "output_format": "json",
                            "timeout_ms": 1000
                        }
                    }
                }
            }
        })
    }

    #[tokio::test]
    async fn test_multi_mob_isolation() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let a = call_tool(&d, "mob_create", json!({"definition":{"id":"mob_a","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await["mob_id"]
            .as_str()
            .unwrap()
            .to_string();
        let b = call_tool(&d, "mob_create", json!({"definition":{"id":"mob_b","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": a, "specs":[{"profile":"worker", "agent_identity":"wa", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": b, "specs":[{"profile":"worker", "agent_identity":"wb", "runtime_mode":"turn_driven"}]}),
        )
        .await;

        let la = call_tool(&d, "mob_list_members", json!({"mob_id": a})).await;
        let lb = call_tool(&d, "mob_list_members", json!({"mob_id": b})).await;
        assert_eq!(la["members"].as_array().unwrap().len(), 1); // wa
        assert_eq!(lb["members"].as_array().unwrap().len(), 1); // wb

        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": a, "action":"destroy"}),
        )
        .await;
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": b, "action":"destroy"}),
        )
        .await;
    }

    #[tokio::test]
    async fn test_mcp_e2e_flow_and_destroy_removes_mob() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let mob_id = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-8","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await["mob_id"]
            .as_str()
            .unwrap()
            .to_string();
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"lead", "agent_identity":"lead", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "agent_identity":"w1", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "agent_identity":"w2", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_wire",
            json!({"mob_id": mob_id, "agent_identity":"w1", "peer":{"local":"w2"}, "action":"wire"}),
        )
        .await;
        let listed = call_tool(&d, "mob_list_members", json!({"mob_id": mob_id})).await;
        assert_eq!(
            listed["members"].as_array().map(std::vec::Vec::len),
            Some(3)
        );
        call_tool(
            &d,
            "mob_wire",
            json!({"mob_id": mob_id, "agent_identity":"w1", "peer":{"local":"w2"}, "action":"unwire"}),
        )
        .await;
        call_tool(
            &d,
            "mob_retire_member",
            json!({"mob_id": mob_id, "agent_identity":"w2"}),
        )
        .await;
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"complete"}),
        )
        .await;
        let events = call_tool(
            &d,
            "mob_events",
            json!({"mob_id": mob_id, "after_cursor":0, "limit":50}),
        )
        .await;
        let events = events["events"].as_array().cloned().unwrap_or_default();
        assert!(
            events.iter().any(|e| e["kind"]["type"] == "member_spawned"),
            "expected structural events to include member_spawned"
        );
        assert!(
            events.iter().any(|e| e["kind"]["type"] == "mob_completed"),
            "expected structural events to include mob_completed"
        );
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"destroy"}),
        )
        .await;
        let listed = call_tool(&d, "mob_list", json!({})).await;
        assert!(listed["mobs"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_mob_list_observes_without_waiting_for_in_flight_member_turn() {
        let svc = Arc::new(MockSessionSvc::new());
        svc.set_turn_delay_ms(5_000);
        let state = Arc::new(MobMcpState::new(
            svc.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(Arc::clone(&state));

        let mob_id = state
            .mob_create_definition(explicit_definition("self-observation-mob"))
            .await
            .expect("create mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");

        let baseline_start_turn_calls = svc.start_turn_call_count();
        let turn_handle = state.handle_for(&mob_id).await.expect("handle");
        let in_flight_turn = tokio::spawn(async move {
            turn_handle
                .member(&AgentIdentity::from("worker-1"))
                .await?
                .internal_turn("observe the mob")
                .await
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while svc.start_turn_call_count() <= baseline_start_turn_calls {
            assert!(
                Instant::now() < deadline,
                "member turn should reach the delayed session service"
            );
            sleep(Duration::from_millis(10)).await;
        }

        let listed = tokio::time::timeout(
            Duration::from_millis(100),
            call_tool(&d, "mob_list", json!({})),
        )
        .await
        .expect("mob_list must not wait behind the in-flight member turn");
        assert_eq!(listed["mobs"].as_array().unwrap().len(), 1);
        assert_eq!(listed["mobs"][0]["status"], "Running");

        let profiles = tokio::time::timeout(Duration::from_millis(100), state.realm_profile_list())
            .await
            .expect("profile observation should also remain available during the turn")
            .expect("profile list should succeed");
        drop(profiles);

        in_flight_turn.abort();
    }

    #[tokio::test]
    async fn test_mcp_stop_resume_round_trip() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let mob_id = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-8","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await["mob_id"]
            .as_str()
            .unwrap()
            .to_string();
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"lead", "agent_identity":"lead", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "agent_identity":"w1", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "agent_identity":"w2", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_wire",
            json!({"mob_id": mob_id, "agent_identity":"w1", "peer":{"local":"w2"}, "action":"wire"}),
        )
        .await;
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"stop"}),
        )
        .await;
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"resume"}),
        )
        .await;
        let members = call_tool(&d, "mob_list_members", json!({"mob_id": mob_id})).await;
        assert_eq!(members["members"].as_array().unwrap().len(), 3); // lead + 2 workers
        let status = call_tool(&d, "mob_list", json!({"mob_id": mob_id})).await;
        assert_eq!(status["status"], "Running");

        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"destroy"}),
        )
        .await;
    }

    #[tokio::test]
    async fn test_mcp_flow_tools_dispatch_run_status_cancel() {
        let svc = Arc::new(MockSessionSvc::new());
        svc.set_turn_delay_ms(60_000);
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(
            &d,
            "mob_create",
            json!({
                "definition": flow_enabled_definition()
            }),
        )
        .await;
        let mob_id = created["mob_id"]
            .as_str()
            .expect("mob_id should be returned")
            .to_string();

        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "agent_identity":"w1"}]}),
        )
        .await;

        let started = call_tool(
            &d,
            "mob_run_flow",
            json!({
                "mob_id": mob_id,
                "flow_id": "demo",
                "params": {"ticket": "ABC-123"}
            }),
        )
        .await;
        let run_id = started["run_id"]
            .as_str()
            .expect("run_id should be returned")
            .to_string();

        let status = call_tool(
            &d,
            "mob_flow_status",
            json!({
                "mob_id": mob_id,
                "run_id": run_id
            }),
        )
        .await;
        assert_eq!(status["run"]["run_id"], run_id);
        assert_eq!(status["run"]["flow_id"], "demo");

        let canceled = call_tool(
            &d,
            "mob_cancel_flow",
            json!({
                "mob_id": mob_id,
                "run_id": run_id
            }),
        )
        .await;
        assert_eq!(canceled["ok"], true);

        let deadline = Instant::now() + Duration::from_secs(8);
        let mut terminal_status = None;
        while Instant::now() < deadline {
            let polled = call_tool(
                &d,
                "mob_flow_status",
                json!({
                    "mob_id": mob_id.clone(),
                    "run_id": run_id.clone()
                }),
            )
            .await;
            if let Some(status) = polled
                .get("run")
                .and_then(|run| run.get("status"))
                .and_then(|status| status.as_str())
            {
                if matches!(status, "canceled" | "completed" | "failed") {
                    terminal_status = Some(status.to_string());
                    break;
                }
            }
            sleep(Duration::from_millis(25)).await;
        }
        assert!(
            matches!(terminal_status.as_deref(), Some("canceled" | "failed")),
            "mob_cancel_flow should converge to canceled, or failed if terminal failure won the race first; got {terminal_status:?}"
        );
    }

    #[tokio::test]
    async fn test_mcp_flow_status_rejects_invalid_run_id() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let error = call_tool_err(
            &d,
            "mob_flow_status",
            json!({
                "mob_id": "does-not-matter",
                "run_id": "not-a-uuid"
            }),
        )
        .await;
        assert!(
            matches!(error, ToolError::InvalidArguments { .. }),
            "invalid run_id should map to invalid arguments"
        );
    }

    #[tokio::test]
    #[ignore = "requires live comms peer after external binding validation was added"]
    async fn test_mob_spawn_backend_arg_returns_backend_member_ref() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(
            &d,
            "mob_create",
            json!({
                "definition": {
                    "id": "ext-mob",
                    "orchestrator": {"profile": "lead"},
                    "profiles": {
                        "lead": {
                            "model": "claude-opus-4-8",
                            "tools": {"comms": true},
                            "external_addressable": true
                        },
                        "worker": {
                            "model": "claude-sonnet-4-5",
                            "tools": {"comms": true},
                            "external_addressable": false
                        }
                    },
                    "wiring": {"auto_wire_orchestrator": false, "role_wiring": []},
                    "skills": {},
                    "backend": {
                        "default": "session",
                        "external": {"address_base": "https://backend.example.invalid/mesh"}
                    }
                }
            }),
        )
        .await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let spawned = call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [{
                    "profile": "worker",
                    "agent_identity": "w-ext",
                    "binding": {
                        "kind": "external",
                        "address": "inproc://test-w-ext",
                        "identity": {
                            "kind": "ed25519_public_key",
                            "public_key": ED25519_PUBLIC_KEY_7
                        }
                    }
                }]
            }),
        )
        .await;
        let row = &spawned["results"].as_array().expect("typed spawn results")[0];
        assert_eq!(row["status"], "spawned");
        assert_eq!(row["result"]["agent_identity"], "w-ext");
        assert!(row["result"]["member_ref"].is_string());
        assert!(row.get("ok").is_none());
    }

    #[tokio::test]
    async fn test_mob_spawn_runtime_mode_defaults_and_override() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-8","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [{"profile": "lead", "agent_identity": "lead-default"}]
            }),
        )
        .await;
        call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [{"profile": "worker", "agent_identity": "worker-turn", "runtime_mode": "turn_driven"}]
            }),
        )
        .await;

        let listed = call_tool(&d, "mob_list_members", json!({"mob_id": mob_id})).await;
        let members = listed["members"].as_array().cloned().unwrap_or_default();
        let lead_mode = members
            .iter()
            .find(|m| m["agent_identity"] == "lead-default")
            .and_then(|m| m["runtime_mode"].as_str());
        let worker_mode = members
            .iter()
            .find(|m| m["agent_identity"] == "worker-turn")
            .and_then(|m| m["runtime_mode"].as_str());

        assert_eq!(lead_mode, Some("autonomous_host"));
        assert_eq!(worker_mode, Some("turn_driven"));
    }

    #[tokio::test]
    async fn test_mob_spawn_many_dispatches_batch() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let spawned = call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [
                    {"profile":"worker","agent_identity":"w-many-a"},
                    {"profile":"worker","agent_identity":"w-many-b"}
                ]
            }),
        )
        .await;
        let results = spawned["results"].as_array().expect("results array");
        assert_eq!(results.len(), 2, "expected two batch rows");
        assert!(
            results
                .iter()
                .all(|row| row["status"] == json!("spawned")
                    && row["result"]["member_ref"].is_string()),
            "all batch spawn rows should succeed"
        );

        let listed = call_tool(&d, "mob_list_members", json!({"mob_id": mob_id})).await;
        let ids = listed["members"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| m["agent_identity"].as_str().map(ToString::to_string))
            .collect::<std::collections::BTreeSet<_>>();
        assert!(ids.contains("w-many-a"));
        assert!(ids.contains("w-many-b"));
    }

    #[tokio::test]
    async fn test_mob_spawn_many_dispatches_typed_failure_cause() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let spawned = call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [
                    {"profile":"missing","agent_identity":"w-missing"}
                ]
            }),
        )
        .await;
        let row = &spawned["results"].as_array().expect("results array")[0];
        assert_eq!(row["status"], "failed");
        assert_eq!(row["result"]["cause"], "profile_not_found");
        assert!(
            row["result"]["message"].as_str().is_some_and(|msg| {
                msg.contains("profile not found") && msg.contains("missing")
            })
        );
        assert!(row.get("ok").is_none());
        assert!(row.get("error").is_none());
    }

    #[tokio::test]
    async fn test_mob_wait_kickoff_returns_member_snapshots() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-8","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();
        call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [
                    {"profile":"lead","agent_identity":"lead-kickoff","runtime_mode":"turn_driven"},
                    {"profile":"worker","agent_identity":"worker-kickoff","runtime_mode":"turn_driven"}
                ]
            }),
        )
        .await;

        let waited = call_tool(
            &d,
            "mob_wait_kickoff",
            json!({
                "mob_id": mob_id,
                "member_ids": ["lead-kickoff", "worker-kickoff"],
                "timeout_ms": 2000
            }),
        )
        .await;
        let members = waited["members"].as_array().expect("members array");
        assert_eq!(members.len(), 2);
        assert_eq!(members[0]["agent_identity"], "lead-kickoff");
        assert_eq!(members[1]["agent_identity"], "worker-kickoff");
    }

    #[tokio::test]
    async fn test_bound_mob_wait_ready_returns_detached_operation() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(Arc::clone(&state));

        let created = call_tool(
            &d,
            "mob_create",
            json!({"definition":{"id":"test_ready_detached","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
        )
        .await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let registry: Arc<dyn OpsLifecycleRegistry> =
            Arc::new(meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry::new());
        let dispatcher = Arc::new(MobMcpDispatcher::new(state));
        let bound = AgentToolDispatcher::bind_ops_lifecycle(
            dispatcher,
            Arc::clone(&registry),
            SessionId::new(),
        )
        .expect("dispatcher should bind ops lifecycle")
        .into_dispatcher();

        let args = json!({
            "mob_id": mob_id,
            "timeout_ms": 60_000
        });
        let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
        let outcome = bound
            .dispatch(mk_call("mob_wait_ready", &raw))
            .await
            .expect("bound wait_ready dispatch should return detached op");
        let payload: serde_json::Value =
            serde_json::from_str(&outcome.result.text_content()).expect("tool json");

        assert_eq!(payload["status"], "waiting");
        assert_eq!(payload["wait_policy"], "detached");
        assert!(payload.get("members").is_none());
        assert_eq!(outcome.async_ops.len(), 1);
        assert_eq!(
            outcome.async_ops[0].wait_policy,
            meerkat_core::ops::WaitPolicy::Detached
        );
        assert_eq!(
            payload["operation_id"].as_str(),
            Some(outcome.async_ops[0].operation_id.to_string().as_str())
        );
        assert!(
            registry
                .snapshot(&outcome.async_ops[0].operation_id)
                .expect("registry snapshot should succeed")
                .is_some(),
            "detached wait operation should be registered before dispatch returns"
        );
    }

    #[tokio::test]
    async fn test_mob_wait_kickoff_completes_after_initial_turn() {
        // The kickoff barrier waits for the initial autonomous turn to complete.
        // Use turn_driven members to avoid the keep_alive mock blocking.
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-8","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();
        call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [
                    {"profile":"lead","agent_identity":"kickoff-lead","runtime_mode":"turn_driven"},
                    {"profile":"worker","agent_identity":"kickoff-worker","runtime_mode":"turn_driven"}
                ]
            }),
        )
        .await;

        // Turn-driven members don't run an autonomous kickoff turn,
        // so the barrier returns immediately.
        let waited = call_tool(
            &d,
            "mob_wait_kickoff",
            json!({
                "mob_id": mob_id,
                "member_ids": ["kickoff-lead", "kickoff-worker"],
                "timeout_ms": 2000
            }),
        )
        .await;
        let members = waited["members"].as_array().expect("members array");
        assert_eq!(members.len(), 2);
        assert_eq!(members[0]["agent_identity"], "kickoff-lead");
        assert_eq!(members[1]["agent_identity"], "kickoff-worker");
    }

    #[tokio::test]
    async fn test_mob_create_rejects_duplicate_mob_id() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"dup_mob","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();
        let error = call_tool_err(&d, "mob_create", json!({"definition":{"id":"dup_mob","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        assert!(
            matches!(error, ToolError::ExecutionFailed { .. }),
            "duplicate mob creation should fail deterministically"
        );

        let listed = call_tool(&d, "mob_list", json!({})).await;
        let ids: Vec<String> = listed["mobs"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|m| m["mob_id"].as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            ids,
            vec![mob_id],
            "duplicate create must not replace active mob"
        );
    }

    #[tokio::test]
    async fn test_mobpack_duplicate_create_requires_same_verified_identity() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let definition = explicit_definition("dup-pack-mob");
        let first_identity =
            meerkat_mob::MobDefinitionSourceIdentity::mobpack("a".repeat(64), Vec::new());
        let second_identity =
            meerkat_mob::MobDefinitionSourceIdentity::mobpack("b".repeat(64), Vec::new());

        let mob_id = state
            .mob_create_from_mobpack(definition.clone(), BTreeMap::new(), first_identity.clone())
            .await
            .expect("initial mobpack create");
        assert_eq!(mob_id.as_str(), "dup-pack-mob");

        let same = state
            .mob_create_from_mobpack(definition.clone(), BTreeMap::new(), first_identity)
            .await
            .expect("same verified mobpack identity should be idempotent");
        assert_eq!(same, mob_id);

        let err = state
            .mob_create_from_mobpack(definition, BTreeMap::new(), second_identity)
            .await
            .expect_err("mismatched verified mobpack digest must fail closed");
        assert!(
            err.to_string().contains("verified pack identity"),
            "expected verified identity mismatch error, got {err}"
        );
    }

    #[tokio::test]
    async fn test_mob_create_rejects_missing_definition() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        // definition field is required at the serde level — empty args should
        // fail at parse_args() time with InvalidArguments.
        let error = call_tool_err(&d, "mob_create", json!({})).await;
        assert!(
            matches!(error, ToolError::InvalidArguments { .. }),
            "mob_create with missing definition must return InvalidArguments, got: {error:?}"
        );
    }

    #[tokio::test]
    async fn test_mob_create_rejects_invalid_definition() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        // A definition with no profiles triggers DiagnosticCode::EmptyProfiles
        // in validate_definition() and must surface as ExecutionFailed.
        let error = call_tool_err(
            &d,
            "mob_create",
            json!({"definition": {"id": "bad_mob", "profiles": {}}}),
        )
        .await;
        assert!(
            matches!(error, ToolError::ExecutionFailed { .. }),
            "mob_create with empty profiles must return ExecutionFailed, got: {error:?}"
        );
    }

    #[tokio::test]
    async fn test_mob_create_rejects_internal_profile_tool_bundles() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let d = MobMcpDispatcher::new(state);

        let error = call_tool_err(
            &d,
            "mob_create",
            json!({
                "definition": {
                    "id": "bad_mob",
                    "profiles": {
                        "worker": {
                            "model": "claude-sonnet-4-6",
                            "tools": {
                                "rust_bundles": ["internal-only"]
                            }
                        }
                    }
                }
            }),
        )
        .await;
        assert!(
            matches!(error, ToolError::InvalidArguments { .. }),
            "mob_create with internal rust bundle fields must return InvalidArguments, got: {error:?}"
        );
    }

    // ── Implicit mob methods ──────────────────────────────────────────

    #[tokio::test]
    async fn test_find_implicit_mob_returns_none_when_no_implicit_mob() {
        let state = MobMcpState::new_in_memory();
        assert!(
            state
                .find_implicit_mob_for_bridge_session("nonexistent")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_get_or_create_implicit_mob_creates_and_reuses() {
        let state = MobMcpState::new_in_memory();
        let session_id = SessionId::new();
        let sid = session_id.to_string();

        let mob_id_1 = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();
        let mob_id_2 = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();
        assert_eq!(mob_id_1, mob_id_2, "second call must return same mob_id");

        let found = state.find_implicit_mob_for_bridge_session(&sid).await;
        assert_eq!(found, Some(mob_id_1));
    }

    #[tokio::test]
    async fn test_get_or_create_implicit_mob_distinct_sessions() {
        let state = MobMcpState::new_in_memory();
        let sid_a = SessionId::new().to_string();
        let sid_b = SessionId::new().to_string();

        let mob_a = state
            .get_or_create_implicit_mob_for_bridge_session(&sid_a, "claude-sonnet-4-5")
            .await
            .unwrap();
        let mob_b = state
            .get_or_create_implicit_mob_for_bridge_session(&sid_b, "claude-sonnet-4-5")
            .await
            .unwrap();
        assert_ne!(mob_a, mob_b, "different sessions must get different mobs");
    }

    #[tokio::test]
    async fn test_ensure_implicit_mob_for_model_reconciles_stale_model_in_state() {
        let state = MobMcpState::new_in_memory();
        let sid = SessionId::new().to_string();

        let old_mob_id = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .expect("create initial implicit mob");
        let old_handle = state
            .handle_for(&old_mob_id)
            .await
            .expect("initial implicit mob handle");
        assert_eq!(
            old_handle
                .definition()
                .profiles
                .get(&ProfileName::from("delegate"))
                .expect("delegate profile")
                .as_inline()
                .unwrap()
                .model,
            "claude-sonnet-4-5"
        );

        let (new_mob_id, created) = state
            .ensure_implicit_mob_for_model(&sid, "gpt-5.4", Some(&old_mob_id))
            .await
            .expect("reconcile implicit mob");

        assert!(created, "model mismatch should force a fresh implicit mob");
        assert_eq!(
            state.find_implicit_mob_for_bridge_session(&sid).await,
            Some(new_mob_id.clone()),
            "session should now point at the reconciled implicit mob"
        );
        assert_eq!(
            new_mob_id, old_mob_id,
            "implicit mob IDs are canonical per session even when the runtime refreshes their model"
        );
        let new_handle = state
            .handle_for(&new_mob_id)
            .await
            .expect("reconciled implicit mob handle");
        assert_eq!(
            new_handle
                .definition()
                .profiles
                .get(&ProfileName::from("delegate"))
                .expect("delegate profile")
                .as_inline()
                .unwrap()
                .model,
            "gpt-5.4"
        );
    }

    #[tokio::test]
    async fn test_is_implicit_mob_true_for_implicit_false_for_explicit() {
        let state = MobMcpState::new_in_memory();

        // Create an implicit mob
        let sid = SessionId::new().to_string();
        let implicit_id = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();

        // Create an explicit mob via mob_create_definition
        let explicit_id = state
            .mob_create_definition(explicit_definition("explicit-mob"))
            .await
            .unwrap();

        assert!(state.is_implicit_mob(&implicit_id).await);
        assert!(!state.is_implicit_mob(&explicit_id).await);
        assert!(!state.is_implicit_mob(&MobId::from("nonexistent")).await);
    }

    fn explicit_definition(mob_id: &str) -> MobDefinition {
        let mut explicit_profiles = BTreeMap::new();
        explicit_profiles.insert(
            ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::profile::Profile {
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: meerkat_mob::profile::ToolConfig {
                    comms: true,
                    ..meerkat_mob::profile::ToolConfig::default()
                },
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            })),
        );
        let mut definition = MobDefinition::explicit(MobId::from(mob_id));
        definition.profiles = explicit_profiles;
        definition
    }

    fn sample_realm_profile(model: &str) -> meerkat_mob::Profile {
        meerkat_mob::profile::Profile {
            model: model.to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: Vec::new(),
            tools: meerkat_mob::profile::ToolConfig::default(),
            peer_description: "realm worker".to_string(),
            external_addressable: false,
            backend: None,
            runtime_mode: meerkat_mob::MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        }
    }

    #[tokio::test]
    async fn realm_profile_child_mobs_hydrate_seeded_skill_sources() {
        let svc = Arc::new(MockSessionSvc::new());
        let store = Arc::new(meerkat_mob::InMemoryRealmProfileStore::new())
            as Arc<dyn meerkat_mob::RealmProfileStore>;
        let mut profile = sample_realm_profile("gpt-5.5");
        profile.skills = vec!["ob3-investigation-worker".to_string()];
        store
            .create("investigation-worker", &profile)
            .await
            .expect("realm profile seeded");

        let mut sources = BTreeMap::new();
        sources.insert(
            "ob3-investigation-worker".to_string(),
            SkillSource::Inline {
                content: "investigation worker rules".to_string(),
            },
        );

        let state = MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
            .with_realm_profile_store(Some(store))
            .with_realm_skill_sources(sources);
        let mut definition = MobDefinition::explicit(MobId::from("child-mob"));
        definition.profiles.insert(
            ProfileName::from("investigation-worker"),
            meerkat_mob::ProfileBinding::RealmRef {
                realm_profile: "investigation-worker".to_string(),
            },
        );

        state
            .hydrate_definition_skill_sources(&mut definition)
            .await
            .expect("hydration succeeds");

        let SkillSource::Inline { content } = definition
            .skills
            .get("ob3-investigation-worker")
            .expect("seeded source copied")
        else {
            panic!("expected inline seeded skill source");
        };
        assert_eq!(content, "investigation worker rules");
    }

    #[tokio::test]
    async fn realm_ref_mob_create_spawns_against_shared_profile_store() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner);
        let mut profile = sample_realm_profile("gpt-5.5");
        profile.tools.comms = true;
        state
            .realm_profile_create("investigation-worker", &profile)
            .await
            .expect("realm profile seeded");
        state
            .realm_profile_create("person-worker", &profile)
            .await
            .expect("person profile seeded");

        let mut definition = MobDefinition::explicit(MobId::from("nest-1779406812080"));
        definition.wiring.auto_wire_orchestrator = true;
        definition.profiles.insert(
            ProfileName::from("investigation-worker"),
            meerkat_mob::ProfileBinding::RealmRef {
                realm_profile: "investigation-worker".to_string(),
            },
        );
        definition.profiles.insert(
            ProfileName::from("person-worker"),
            meerkat_mob::ProfileBinding::RealmRef {
                realm_profile: "person-worker".to_string(),
            },
        );

        let mob_id = state
            .mob_create_definition(definition)
            .await
            .expect("realm-ref mob create should succeed");
        let mut spec = SpawnMemberSpec::new(
            ProfileName::from("investigation-worker"),
            AgentIdentity::from("investigation-worker-nest-1779406812080"),
        );
        spec.auto_wire_parent = true;

        let result = state
            .mob_spawn_spec(&mob_id, spec)
            .await
            .expect("realm-ref profile should resolve from shared state store");
        assert_eq!(
            result.agent_identity,
            AgentIdentity::from("investigation-worker-nest-1779406812080")
        );
    }

    #[tokio::test]
    async fn test_persistent_root_restores_explicit_mob_member_status() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );

        let mob_id = state
            .mob_create_definition(explicit_definition("restored-explicit"))
            .await
            .expect("create explicit mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");

        let restored = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        let status = restored
            .mob_member_status(&mob_id, &AgentIdentity::from("worker-1"))
            .await
            .expect("restore member status");
        assert_eq!(status.status, meerkat_mob::MobMemberStatus::Active);
        // Verify the member has a live bridge-session binding via the handle.
        let handle = restored.handle_for(&mob_id).await.expect("mob handle");
        assert!(
            handle
                .resolve_bridge_session_id(&AgentIdentity::from("worker-1"))
                .await
                .is_some(),
            "restored member should still have a live bridge-session binding"
        );

        let mobs = restored.mob_list().await.expect("restore mob list");
        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0].0, mob_id);
    }

    #[tokio::test]
    async fn current_session_schedule_survives_member_respawn_and_runtime_restart() {
        use meerkat::surface::SurfaceScheduleMobHost as _;

        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let runtime_root = root.path().join("runtime");
        let schedule_path = root.path().join("schedules.db");
        let identity = AgentIdentity::from("worker-1");
        let mut definition = explicit_definition("scheduled-identity-e2e");
        if let Some(meerkat_mob::ProfileBinding::Inline(profile)) =
            definition.profiles.get_mut(&ProfileName::from("worker"))
        {
            profile.external_addressable = true;
        }

        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .with_persistent_storage_root(Some(runtime_root.clone())),
        );
        let mob_id = state
            .mob_create_definition(definition)
            .await
            .expect("create persistent mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                identity.clone(),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn original worker");
        let original_bridge_session = state
            .mob_resolve_bridge_session_id(&mob_id, &identity)
            .await
            .expect("resolve original bridge session")
            .expect("original bridge session exists");

        let schedule_store = Arc::new(
            meerkat_store::SqliteScheduleStore::open(&schedule_path).expect("open schedule store"),
        );
        let schedule_service = meerkat_schedule::ScheduleService::new(schedule_store.clone());
        let dispatcher = meerkat_schedule::CurrentSessionScheduleToolDispatcher::new_with_resolver(
            Arc::new(meerkat_schedule::ScheduleToolDispatcher::new(
                schedule_service.clone(),
            )),
            original_bridge_session.clone(),
            Arc::new(
                meerkat::surface::MobMemberCurrentSessionScheduleResolver::new(
                    meerkat_core::MobMemberBinding {
                        mob_id: mob_id.to_string(),
                        role: "worker".to_string(),
                        member: identity.to_string(),
                    },
                ),
            ),
        );
        let schedule_args = json!({
            "name": "durable identity schedule",
            "description": "created by an identity-backed member through current_session",
            "trigger": {
                "type": "interval",
                "start_at_utc": chrono::Utc::now(),
                "every_seconds": 60
            },
            "target": {
                "target_kind": "session",
                "type": "current_session",
                "action": {
                    "type": "prompt",
                    "prompt": "scheduled identity survives restart"
                }
            },
            "misfire_policy": { "type": "skip" },
            "overlap_policy": "skip_if_running",
            "missing_target_policy": "mark_misfired",
            "planning_horizon_occurrences": 1
        });
        let raw_schedule_args =
            serde_json::value::RawValue::from_string(schedule_args.to_string()).expect("raw args");
        let created = dispatcher
            .dispatch(ToolCallView {
                id: "schedule-1",
                name: "meerkat_schedule_create",
                args: raw_schedule_args.as_ref(),
            })
            .await
            .expect("schedule create through current_session wrapper");
        let created: serde_json::Value =
            serde_json::from_str(&created.result.text_content()).expect("created schedule json");
        let schedule_id = meerkat_schedule::ScheduleId::parse(
            created["schedule_id"]
                .as_str()
                .expect("created schedule id"),
        )
        .expect("valid schedule id");
        let persisted = schedule_service
            .get(&schedule_id)
            .await
            .expect("load created schedule");
        let meerkat::TargetBinding::Identity(binding) = &persisted.target else {
            panic!("current_session from mob member must persist as identity target");
        };
        assert!(
            !binding
                .identity()
                .contains(&original_bridge_session.to_string()),
            "durable identity must not include the transient bridge session"
        );
        assert!(
            !binding.identity().contains("worker\""),
            "durable identity must not include the old profile/role"
        );

        state
            .mob_respawn(&mob_id, identity.clone(), None)
            .await
            .expect("respawn worker");
        let respawned_bridge_session = state
            .mob_resolve_bridge_session_id(&mob_id, &identity)
            .await
            .expect("resolve respawned bridge session")
            .expect("respawned bridge session exists");
        assert_ne!(
            respawned_bridge_session, original_bridge_session,
            "respawn should regenerate the bridge session"
        );

        let old_handle = state.handle_for(&mob_id).await.expect("old mob handle");
        old_handle
            .crash_stop_preserving_durable_work_for_test()
            .await
            .expect("crash-stop old mob actor before reopening durable storage");
        let closed_error = tokio::time::timeout(Duration::from_secs(1), old_handle.stop())
            .await
            .expect("old actor command channel closes after crash stop")
            .expect_err("crash-stopped actor must reject later commands");
        assert!(matches!(closed_error, MobError::ActorCommandChannelClosed));
        drop(old_handle);
        drop(state);
        // A process restart rebuilds both the surface sidecars and their
        // machine-owned executor attachments. Retain only the mock session
        // facts needed to stand those actors back up; reusing the old machine
        // here would be a warm surface replacement over a live attachment,
        // which correctly requires its original exact sidecar.
        let svc = Arc::new(svc.cold_restart().await);
        let restored_state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .with_persistent_storage_root(Some(runtime_root)),
        );
        let restored_bridge_session = restored_state
            .mob_resolve_bridge_session_id(&mob_id, &identity)
            .await
            .expect("resolve restored bridge session")
            .expect("restored bridge session exists");
        assert_eq!(
            restored_bridge_session, respawned_bridge_session,
            "runtime restart should restore the current materialized member binding"
        );

        let restored_schedule_store = Arc::new(
            meerkat_store::SqliteScheduleStore::open(&schedule_path)
                .expect("reopen schedule store"),
        );
        let restored_schedule_service =
            meerkat_schedule::ScheduleService::new(restored_schedule_store);
        let restored_schedule = restored_schedule_service
            .get(&schedule_id)
            .await
            .expect("load schedule after restart");
        let occurrence = meerkat_schedule::Occurrence::planned_from_schedule(
            &restored_schedule,
            meerkat_schedule::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("plan restored occurrence");
        let meerkat::TargetBinding::Identity(restored_binding) = &restored_schedule.target else {
            panic!("restored schedule must keep identity target");
        };
        let host = MobMcpScheduleHost::new(restored_state);
        let probe = host
            .probe_identity_target(restored_binding)
            .await
            .expect("probe restored identity target")
            .expect("mob identity should be handled by mob host");
        assert!(matches!(probe, meerkat::TargetProbeOutcome::Ready));

        let before_turns = svc.start_turn_call_count();
        let dispatch = host
            .deliver_identity_target(&occurrence, restored_binding)
            .await
            .expect("deliver restored identity target")
            .expect("mob identity should deliver through restored mob host");
        let terminal = dispatch.completion.await.expect("delivery completion");
        assert_eq!(
            terminal.phase,
            meerkat::OccurrencePhase::Completed,
            "scheduled identity delivery failed: {terminal:?}"
        );
        assert_eq!(dispatch.correlation_id.as_deref(), Some("worker-1"));
        let delivery_deadline = Instant::now() + Duration::from_secs(2);
        while svc.start_turn_call_count() <= before_turns {
            assert!(
                Instant::now() < delivery_deadline,
                "accepted scheduled delivery should reach the restored member session"
            );
            sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            svc.start_turn_call_count(),
            before_turns + 1,
            "scheduled prompt should reach the restored current member session"
        );
        let (delivered_session, delivered_prompt) = svc
            .last_start_turn()
            .await
            .expect("scheduled delivery should start a turn");
        assert_eq!(
            delivered_session, restored_bridge_session,
            "scheduled prompt must target the restored current materialized session"
        );
        assert_ne!(
            delivered_session, original_bridge_session,
            "scheduled prompt must not target the original transient bridge session"
        );
        assert_eq!(delivered_prompt, "scheduled identity survives restart");
    }

    #[tokio::test]
    async fn test_mob_destroy_removes_persistent_store_file() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );

        let mob_id = state
            .mob_create_definition(explicit_definition("destroyed-explicit"))
            .await
            .expect("create explicit mob");
        let storage_root = MobMcpState::persistent_mob_root(root.path());
        let storage_path = MobMcpState::persistent_storage_path(&storage_root, &mob_id);
        assert!(
            tokio::fs::metadata(&storage_path).await.is_ok(),
            "persistent mob db should exist after create"
        );

        state.mob_destroy(&mob_id).await.expect("destroy mob");
        assert!(
            tokio::fs::metadata(&storage_path).await.is_err(),
            "destroy should remove persistent mob db"
        );

        let restored = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        assert!(
            restored
                .mob_list()
                .await
                .expect("list restored mobs")
                .is_empty(),
            "destroyed persistent mobs must not reappear after restart"
        );
    }

    #[tokio::test]
    async fn test_mob_list_surfaces_persistent_restore_failure() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let mob_root = MobMcpState::persistent_mob_root(root.path());
        tokio::fs::create_dir_all(&mob_root)
            .await
            .expect("create mob root");
        tokio::fs::write(mob_root.join("broken.db"), b"not a sqlite database")
            .await
            .expect("write invalid mob store");

        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        let err = state
            .mob_list()
            .await
            .expect_err("restore failure must not be reported as an empty successful list");

        assert!(
            err.to_string().contains("broken.db") || err.to_string().contains("database"),
            "restore failure should preserve the typed storage error, got {err}"
        );
    }

    #[tokio::test]
    async fn test_incomplete_mob_destroy_retains_storage_and_retry_anchor() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );

        let mob_id = state
            .mob_create_definition(explicit_definition("partial-destroy-explicit"))
            .await
            .expect("create explicit mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");
        let handle = state.handle_for(&mob_id).await.expect("mob handle");
        let bridge_session_id = handle
            .resolve_bridge_session_id(&AgentIdentity::from("worker-1"))
            .await
            .expect("worker bridge session");
        drop(handle);
        svc.fail_archive(
            bridge_session_id.clone(),
            "forced archive failure for partial destroy test",
        )
        .await;

        let storage_root = MobMcpState::persistent_mob_root(root.path());
        let storage_path = MobMcpState::persistent_storage_path(&storage_root, &mob_id);
        assert!(
            tokio::fs::metadata(&storage_path).await.is_ok(),
            "persistent mob db should exist before destroy"
        );

        let public_payload = json!({
            "mob_id": mob_id.to_string(),
            "action": "destroy",
        });
        let err = crate::public_mcp::handle_public_tools_call(
            &state,
            "meerkat_mob_lifecycle",
            &public_payload,
        )
        .await
        .expect_err("incomplete destroy must fail closed");
        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::InternalError.jsonrpc_code()
        );
        let data = err.data.expect("incomplete destroy should include data");
        assert_eq!(
            data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );
        assert_eq!(
            data.get("retryable").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert!(
            data.get("destroy_report")
                .and_then(|report| report.get("errors"))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|errors| !errors.is_empty()),
            "partial destroy error data should include the destroy report errors: {data}"
        );
        assert!(
            tokio::fs::metadata(&storage_path).await.is_ok(),
            "incomplete destroy must retain persistent mob db"
        );
        assert!(
            svc.session_exists(&bridge_session_id).await,
            "failed ArchiveSession cleanup must leave the bridge session available for retry"
        );
        let members = state
            .mob_list_members(&mob_id)
            .await
            .expect("list members after incomplete destroy");
        assert!(
            members
                .iter()
                .any(|member| member.agent_identity == "worker-1"),
            "incomplete destroy must retain the failed member as retry work"
        );
        let mobs = state.mob_list().await.expect("list retained mobs");
        assert!(
            mobs.iter().any(|(id, _)| id == &mob_id),
            "incomplete destroy must retain in-memory retry anchor"
        );

        svc.clear_archive_failure(&bridge_session_id).await;
        let retry_report = state
            .mob_destroy(&mob_id)
            .await
            .expect("retry should complete cleanup once archive succeeds");
        assert!(retry_report.metadata_scrubbed);
        assert!(retry_report.events_cleared);
        assert!(retry_report.namespace_cleaned);
        assert!(
            !svc.session_exists(&bridge_session_id).await,
            "complete retry must actually archive the bridge session before storage removal"
        );
        assert!(
            tokio::fs::metadata(&storage_path).await.is_err(),
            "complete retry should remove persistent mob db"
        );
        assert!(
            state
                .mob_list()
                .await
                .expect("list mobs after retry")
                .is_empty(),
            "complete retry should remove retry anchor"
        );
    }

    #[tokio::test]
    async fn test_default_constructor_exposes_realm_profile_crud_with_in_memory_store() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc,
            meerkat_mob::MobControlPrincipal::Owner,
        ));

        let created = state
            .realm_profile_create("worker", &sample_realm_profile("claude-sonnet-4-6"))
            .await
            .expect("default constructor should provide realm profile store");
        assert_eq!(created.name, "worker");

        let fetched = state
            .realm_profile_get("worker")
            .await
            .expect("get realm profile")
            .expect("stored profile");
        assert_eq!(fetched.profile.model, "claude-sonnet-4-6");
    }

    #[tokio::test]
    async fn realm_profile_crud_is_owner_only_before_store_access() {
        let principal =
            meerkat_core::auth::PrincipalId::new("realm-profile-viewer").expect("principal id");
        let state = MobMcpState::new_in_memory_as(MobControlPrincipal::External(principal));
        let profile = sample_realm_profile("claude-sonnet-4-6");

        let create = state
            .realm_profile_create("worker", &profile)
            .await
            .expect_err("realm-wide profile mutation requires Owner");
        assert_required_scope(&create, ControlScope::SendCommand);
        let get = state
            .realm_profile_get("worker")
            .await
            .expect_err("realm-wide profile read requires Owner");
        assert_required_scope(&get, ControlScope::List);
        let list = state
            .realm_profile_list()
            .await
            .expect_err("realm-wide profile enumeration requires Owner");
        assert_required_scope(&list, ControlScope::List);
        let update = state
            .realm_profile_update("worker", &profile, 1)
            .await
            .expect_err("realm-wide profile update requires Owner");
        assert_required_scope(&update, ControlScope::SendCommand);
        let delete = state
            .realm_profile_delete("worker", 1)
            .await
            .expect_err("realm-wide profile delete requires Owner");
        assert_required_scope(&delete, ControlScope::SendCommand);
    }

    #[tokio::test]
    async fn test_persistent_root_upgrades_default_realm_profile_store_to_durable_sqlite() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");

        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        state
            .realm_profile_create("worker", &sample_realm_profile("claude-opus-4-8"))
            .await
            .expect("create persistent realm profile");

        let restored = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        let fetched = restored
            .realm_profile_get("worker")
            .await
            .expect("get restored realm profile")
            .expect("restored profile should exist");
        assert_eq!(fetched.profile.model, "claude-opus-4-8");
    }

    #[tokio::test]
    async fn test_destroy_bridge_session_mobs_cleans_up() {
        let state = MobMcpState::new_in_memory();
        let sid = SessionId::new().to_string();

        let _mob_id = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();
        assert!(
            state
                .find_implicit_mob_for_bridge_session(&sid)
                .await
                .is_some()
        );

        state.destroy_bridge_session_mobs(&sid).await.unwrap();
        assert!(
            state
                .find_implicit_mob_for_bridge_session(&sid)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_destroy_bridge_session_mobs_fails_closed_on_incomplete_destroy() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let sid = SessionId::new().to_string();

        let definition = explicit_definition("bridge-session-partial-destroy");
        let mob_id = state
            .mob_create_definition_with_owner_bridge_session(
                definition,
                SessionId::parse(&sid).expect("session id"),
                true,
                false,
            )
            .await
            .expect("create bridge-session-scoped mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");
        let bridge_session_id = state
            .handle_for(&mob_id)
            .await
            .expect("mob handle")
            .resolve_bridge_session_id(&AgentIdentity::from("worker-1"))
            .await
            .expect("worker bridge session");
        svc.fail_archive(
            bridge_session_id.clone(),
            "forced bridge-session cleanup archive failure",
        )
        .await;

        let err = state
            .destroy_bridge_session_mobs(&sid)
            .await
            .expect_err("partial bridge-session cleanup must fail closed");
        assert!(
            matches!(err, MobMcpDestroyError::Incomplete { .. }),
            "expected typed incomplete cleanup error, got {err:?}"
        );
        assert!(
            state.handle_for(&mob_id).await.is_ok(),
            "incomplete bridge-session cleanup must retain the mob retry anchor"
        );

        svc.clear_archive_failure(&bridge_session_id).await;
        state
            .destroy_bridge_session_mobs(&sid)
            .await
            .expect("retry should clean bridge-session mob");
        assert!(
            state.handle_for(&mob_id).await.is_err(),
            "successful retry should remove bridge-session mob"
        );
    }

    #[tokio::test]
    async fn test_archive_session_with_mob_cleanup_surfaces_incomplete_and_retries_success() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let owner_session_id = SessionId::new();
        svc.insert_persisted_session(Session::with_id(owner_session_id.clone()))
            .await;

        let definition = explicit_definition("archive-helper-partial-destroy");
        let mob_id = state
            .mob_create_definition_with_owner_bridge_session(
                definition,
                owner_session_id.clone(),
                true,
                false,
            )
            .await
            .expect("create archive-helper-owned mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");
        let bridge_session_id = state
            .handle_for(&mob_id)
            .await
            .expect("mob handle")
            .resolve_bridge_session_id(&AgentIdentity::from("worker-1"))
            .await
            .expect("worker bridge session");
        svc.fail_archive(
            bridge_session_id.clone(),
            "forced archive-helper cleanup archive failure",
        )
        .await;

        let err = crate::agent_tools::archive_session_with_mob_cleanup(
            svc.clone(),
            state.clone(),
            &owner_session_id,
        )
        .await
        .expect_err("archive helper must fail closed on incomplete mob cleanup");
        let SessionError::FailedWithData { data, .. } = err else {
            panic!("expected typed incomplete session error, got {err:?}");
        };
        assert_eq!(
            data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );
        assert_eq!(
            data.get("retryable").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert!(
            state.handle_for(&mob_id).await.is_ok(),
            "incomplete archive helper cleanup must retain the mob retry anchor"
        );
        assert!(
            !svc.session_exists(&owner_session_id).await,
            "first archive attempt should have removed the owner session before mob cleanup failed"
        );
        assert!(
            svc.session_exists(&bridge_session_id).await,
            "failed member archive must retain the bridge session for retry"
        );

        svc.clear_archive_failure(&bridge_session_id).await;
        crate::agent_tools::archive_session_with_mob_cleanup(
            svc.clone(),
            state.clone(),
            &owner_session_id,
        )
        .await
        .expect("retry should report success after retained mob cleanup completes");
        assert!(
            state.handle_for(&mob_id).await.is_err(),
            "successful archive helper retry should remove the mob retry anchor"
        );
        assert!(
            !svc.session_exists(&bridge_session_id).await,
            "successful archive helper retry must archive the worker bridge session"
        );
    }

    #[tokio::test]
    async fn test_archive_session_with_mob_cleanup_runs_member_retire_then_child_cleanup() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let parent_mob_id = state
            .mob_create_definition(explicit_definition("archive-helper-live-parent"))
            .await
            .expect("create parent mob");
        let parent_identity = AgentIdentity::from("worker-1");
        state
            .mob_spawn(
                &parent_mob_id,
                ProfileName::from("worker"),
                parent_identity.clone(),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn parent worker");
        let member_session_id = state
            .handle_for(&parent_mob_id)
            .await
            .expect("parent mob handle")
            .resolve_bridge_session_id(&parent_identity)
            .await
            .expect("parent member bridge session");

        let child_definition = explicit_definition("archive-helper-live-member-child");
        let child_mob_id = state
            .mob_create_definition_with_owner_bridge_session(
                child_definition,
                member_session_id.clone(),
                true,
                false,
            )
            .await
            .expect("create child mob owned by parent member session");
        let child_identity = AgentIdentity::from("child-worker-1");
        state
            .mob_spawn(
                &child_mob_id,
                ProfileName::from("worker"),
                child_identity.clone(),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn child worker");
        let child_bridge_session_id = state
            .handle_for(&child_mob_id)
            .await
            .expect("child mob handle")
            .resolve_bridge_session_id(&child_identity)
            .await
            .expect("child worker bridge session");
        svc.fail_archive(
            child_bridge_session_id.clone(),
            "forced archive-helper live-member child cleanup failure",
        )
        .await;

        let err = crate::agent_tools::archive_session_with_mob_cleanup(
            svc.clone(),
            state.clone(),
            &member_session_id,
        )
        .await
        .expect_err("archive helper must fail closed on child cleanup after member retire");
        let SessionError::FailedWithData { data, .. } = err else {
            panic!("expected typed incomplete child cleanup error, got {err:?}");
        };
        assert_eq!(
            data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );
        assert!(
            !svc.session_exists(&member_session_id).await,
            "successful parent retire must archive the mob member bridge session before child cleanup"
        );
        assert!(
            state.handle_for(&child_mob_id).await.is_ok(),
            "incomplete child cleanup must retain the child mob retry anchor"
        );

        svc.clear_archive_failure(&child_bridge_session_id).await;
        crate::agent_tools::archive_session_with_mob_cleanup(
            svc.clone(),
            state.clone(),
            &member_session_id,
        )
        .await
        .expect("retry should complete retained child cleanup after parent member retire");
        assert!(
            state.handle_for(&child_mob_id).await.is_err(),
            "successful retry must remove the child mob retry anchor"
        );
        assert!(
            !svc.session_exists(&child_bridge_session_id).await,
            "successful retry must archive the child worker bridge session"
        );
    }

    #[tokio::test]
    async fn test_destroy_bridge_session_mobs_uses_generated_cleanup_authority() {
        let state = MobMcpState::new_in_memory();
        let sid = SessionId::new().to_string();

        let manual = explicit_definition("manual-owner-index");
        let manual_id = state
            .mob_create_definition(manual)
            .await
            .expect("create manual unowned mob");

        let bridge_session_scoped = explicit_definition("bridge-session-scoped-owner");
        let bridge_session_scoped_id = state
            .mob_create_definition_with_owner_bridge_session(
                bridge_session_scoped,
                SessionId::parse(&sid).expect("session id"),
                true,
                false,
            )
            .await
            .expect("create bridge-session-scoped mob");

        state.destroy_bridge_session_mobs(&sid).await.unwrap();

        assert!(
            state.handle_for(&manual_id).await.is_ok(),
            "manual unowned mob must not be eligible for owner-session cleanup"
        );
        assert!(
            state.handle_for(&bridge_session_scoped_id).await.is_err(),
            "generated DestroyOnOwnerArchive authority must remain the cleanup truth"
        );
    }

    #[tokio::test]
    async fn test_destroy_bridge_session_mobs_noop_when_none() {
        let state = MobMcpState::new_in_memory();
        // Should succeed even when no implicit mob exists
        state
            .destroy_bridge_session_mobs("nonexistent")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_concurrent_get_or_create_produces_single_mob() {
        let state = Arc::new(MobMcpState::new_in_memory());
        let sid = SessionId::new().to_string();
        let n = 20;

        let mut handles = Vec::new();
        for _ in 0..n {
            let s = state.clone();
            let sid = sid.clone();
            handles.push(tokio::spawn(async move {
                s.get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
                    .await
                    .unwrap()
            }));
        }

        let mut mob_ids = Vec::new();
        for h in handles {
            mob_ids.push(h.await.unwrap());
        }

        // All must return the same mob_id
        let first = &mob_ids[0];
        for id in &mob_ids {
            assert_eq!(id, first, "concurrent calls must return same mob_id");
        }

        // Only one mob should exist in the registry
        let mobs = state.mob_list().await.expect("list implicit mobs");
        let implicit_count = mobs.len();
        assert_eq!(implicit_count, 1, "only one implicit mob should exist");
    }

    #[tokio::test]
    async fn test_scavenge_orphaned_bridge_session_scoped_mobs() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        ));

        // Create a session and its implicit mob
        let result = svc
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: ContentInput::from("test"),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .unwrap();
        let sid = result.session_id.to_string();
        let mob_id = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();

        // Also create an unowned explicit mob that should survive scavenging.
        let manual = explicit_definition("manual-owner-index");
        let manual_id = state
            .mob_create_definition(manual)
            .await
            .expect("create manual unowned mob");

        // Session exists — scavenge should find nothing.
        let scavenged = state.scavenge_orphaned_bridge_session_scoped_mobs().await;
        assert!(scavenged.is_empty(), "no orphans while session exists");

        // Archive the session — now the mob is orphaned
        svc.archive(&result.session_id).await.unwrap();

        // Scavenge should find and destroy the bridge-session-scoped orphan, but not
        // the manual owner-indexed mob.
        let scavenged = state.scavenge_orphaned_bridge_session_scoped_mobs().await;
        assert_eq!(scavenged, vec![mob_id.clone()]);

        // Mob should be gone
        assert!(
            state
                .find_implicit_mob_for_bridge_session(&sid)
                .await
                .is_none()
        );
        assert!(
            state.handle_for(&manual_id).await.is_ok(),
            "manual unowned mob must survive orphan scavenging"
        );
    }

    #[tokio::test]
    async fn test_scavenge_orphaned_bridge_session_scoped_mobs_honors_bridge_owner_index() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            svc.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        ));

        let result = svc
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: ContentInput::from("test"),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .unwrap();
        let sid = result.session_id.to_string();

        let bridge_owned = explicit_definition("bridge-owned-orphan");
        let bridge_owned_id = state
            .mob_create_definition_with_owner_bridge_session(
                bridge_owned,
                SessionId::parse(&sid).expect("session id"),
                true,
                false,
            )
            .await
            .expect("create bridge-owned bridge-session-scoped mob");

        let scavenged = state.scavenge_orphaned_bridge_session_scoped_mobs().await;
        assert!(
            scavenged.is_empty(),
            "session exists, so bridge-owned mob must not scavenge"
        );

        svc.archive(&result.session_id).await.unwrap();

        let scavenged = state.scavenge_orphaned_bridge_session_scoped_mobs().await;
        assert_eq!(scavenged, vec![bridge_owned_id.clone()]);
        assert!(
            state.handle_for(&bridge_owned_id).await.is_err(),
            "bridge-owned bridge-session-scoped mob must be scavenged once the bridge session disappears"
        );
    }

    #[tokio::test]
    async fn test_implicit_mob_uses_external_owner_wiring() {
        let state = MobMcpState::new_in_memory();
        let sid = SessionId::new().to_string();

        let mob_id = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();
        let handle = state.handle_for(&mob_id).await.unwrap();
        assert!(
            !handle.definition().wiring.auto_wire_orchestrator,
            "implicit delegate mobs wire the owner as an external peer, not as a local orchestrator"
        );
        assert!(
            handle.definition().orchestrator.is_none(),
            "implicit delegate mobs must not invent a local orchestrator member"
        );
        assert_eq!(
            handle
                .owner_bridge_session_lifecycle_authority()
                .map(|authority| authority.bridge_session_id.to_string()),
            Some(sid),
            "implicit mob must have correct owner_bridge_session_id"
        );
    }

    #[tokio::test]
    async fn test_local_session_service_provides_runtime_adapter() {
        let svc = LocalSessionService::new();
        assert!(
            <LocalSessionService as MobSessionService>::runtime_adapter(&svc).is_some(),
            "LocalSessionService must provide adapter for AutonomousHost"
        );
    }

    #[tokio::test]
    async fn test_local_session_service_apply_runtime_turn_returns_start_turn_terminal_result() {
        let svc = LocalSessionService::new();
        let req = CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: "test".to_string().into(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        };
        let created = <LocalSessionService as SessionService>::create_session(&svc, req)
            .await
            .expect("create session");
        let result = <LocalSessionService as MobSessionService>::apply_runtime_turn(
            &svc,
            &created.session_id,
            meerkat_core::RunId::new(),
            StartTurnRequest {
                injected_context: Vec::new(),
                prompt: "test".into(),
                system_prompt: None,
                event_tx: None,
                runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
            },
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
            vec![],
        )
        .await;
        assert!(
            result.is_ok(),
            "apply_runtime_turn must work for AutonomousHost: {result:?}"
        );
        let output = result.expect("apply_runtime_turn");
        match output.terminal {
            Some(meerkat_core::lifecycle::core_executor::CoreApplyTerminal::RunResult(
                run_result,
            )) => {
                assert_eq!(run_result.text, "ok");
                assert_eq!(run_result.session_id, created.session_id);
            }
            other => panic!("expected start_turn terminal run result, got {other:?}"),
        }
    }

    /// T-B5 (§17.4, DEC-P7B-9): typed MCP constructors — ScopeDenied
    /// carries -32025 + the bare typed `{required, presented}` data,
    /// StaleCursor carries the watermark, and an unmapped `MobError` is
    /// byte-identical to the legacy `invalid_params` fallback.
    #[test]
    fn mcp_tool_error_from_mob_typed_and_fallback() {
        let denied = MobError::ScopeDenied(ScopeDenial {
            required: ControlScope::AdminHost,
            presented: std::collections::BTreeSet::from([ControlScope::List]),
        });
        let err = McpToolError::from_mob(&denied);
        assert_eq!(err.code, -32025, "ScopeDenied carries the stable code");
        assert_eq!(err.message, denied.to_string());
        assert_eq!(
            err.data,
            Some(json!({ "required": "admin_host", "presented": ["list"] })),
            "data is the BARE phase-5 detail shape"
        );

        let stale = MobError::StaleEventCursor {
            after_cursor: 12,
            latest_cursor: 5,
        };
        let err = McpToolError::from_mob(&stale);
        assert_eq!(err.code, -32027, "StaleCursor carries the stable code");
        let data = err.data.expect("stale cursor data");
        assert_eq!(data["watermark"], 5, "reply carries the current watermark");
        assert_eq!(data["requested"], 12);

        let unmapped = MobError::MobNotFound(MobId::from("missing"));
        let err = McpToolError::from_mob(&unmapped);
        let legacy = McpToolError::invalid_params(unmapped.to_string());
        assert_eq!(err.code, legacy.code, "fallback keeps -32602");
        assert_eq!(
            err.message, legacy.message,
            "fallback message is byte-identical"
        );
        assert_eq!(
            err.data, None,
            "fallback carries no data, exactly like legacy"
        );

        let append_denied = MobAppendSystemContextError::Mob(MobError::ScopeDenied(ScopeDenial {
            required: ControlScope::SendCommand,
            presented: BTreeSet::new(),
        }));
        let err = McpToolError::from_mob_append_system_context(&append_denied);
        assert_eq!(err.code, -32025);
        assert_eq!(
            err.data,
            Some(json!({ "required": "send_command", "presented": [] }))
        );

        let append_session =
            MobAppendSystemContextError::Session(SessionControlError::InvalidRequest {
                message: "bad append".to_string(),
            });
        let err = McpToolError::from_mob_append_system_context(&append_session);
        assert_eq!(err.code, -32602);
        assert!(err.data.is_none());
    }

    /// T-B3 (destroy half): `MobMcpDestroyError::wire_detail` delegates for
    /// `Mob(inner)`; the `Incomplete` arm keeps its dedicated
    /// `destroy_incomplete` envelope through `from_mob_destroy`.
    #[test]
    fn destroy_wrapper_delegates_wire_detail_and_keeps_incomplete_envelope() {
        let delegated = MobMcpDestroyError::Mob(MobError::BridgeRequestTimedOut {
            request_envelope_id: "env-3".to_string(),
            timeout_ms: 30_000,
        });
        assert!(
            matches!(
                delegated.wire_detail(),
                Some(meerkat_contracts::wire::WireMobErrorDetail::HostUnavailable(_))
            ),
            "Mob(inner) must delegate the console projection"
        );
        let err = McpToolError::from_mob_destroy(&delegated);
        assert_eq!(err.code, -32026);

        let mut report = meerkat_mob::MobDestroyReport::default();
        report.errors.push("worker: archive failed".to_string());
        let incomplete = MobMcpDestroyError::Incomplete { report };
        assert!(
            incomplete.wire_detail().is_none(),
            "Incomplete keeps its dedicated envelope, not a console code"
        );
        let err = McpToolError::from_mob_destroy(&incomplete);
        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::InternalError.jsonrpc_code()
        );
        assert_eq!(
            err.data
                .as_ref()
                .and_then(|data| data.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete"),
            "destroy_incomplete data envelope preserved verbatim"
        );
    }

    /// T-B8 (local half; the placed-member half rides the cross-host
    /// fixtures in meerkat-mob): the ONE envelope-assembly point emits
    /// `placement: None` + `ControllingHostVerified` for a locally-served
    /// page, with the shared page body shape.
    #[tokio::test]
    async fn member_history_result_provenance_and_placement_local() {
        let state = MobMcpState::new_in_memory();
        let mob_id = state
            .mob_create_definition(explicit_definition("history-provenance-mob"))
            .await
            .expect("create mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn local member");

        let result = state
            .mob_member_history(&mob_id, AgentIdentity::from("worker-1"), None, None)
            .await
            .expect("local member history");
        assert_eq!(
            result.placement, None,
            "a locally-served page carries no placement"
        );
        assert_eq!(
            result.provenance,
            meerkat_contracts::wire::WireProjectionProvenance::ControllingHostVerified,
            "controlling-host pages are ControllingHostVerified"
        );
        // Shared page-body invariants (WireMemberHistoryPageBody::try_from_history_page).
        assert_eq!(result.page.from_index, 0);
        assert_eq!(
            result.page.messages.len() as u64,
            result.page.message_count,
            "a complete single page serves the whole transcript"
        );
        assert!(result.page.complete);
        assert_eq!(result.page.next_index, None);
    }

    /// T-B13: scope matrix over the phase-7 wrappers. A List-only named
    /// principal reads the two chokepoint-(b) projections but the
    /// AdminHost-gated revoke and the ReadHistory-gated member history
    /// deny with the typed `{required, presented}` pair (actor-gated).
    #[tokio::test]
    async fn phase7_wrapper_scope_matrix() {
        use meerkat_contracts::wire::{WireControlScope, WireMobErrorDetail};

        let owner = MobMcpState::new_in_memory();
        let mob_id = owner
            .mob_create_definition(explicit_definition("phase7-scope-matrix"))
            .await
            .expect("create mob");
        owner
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn member");
        let viewer_id = meerkat_core::auth::PrincipalId::new("viewer").expect("principal id");
        owner
            .mob_grant_scopes(
                &mob_id,
                viewer_id.clone(),
                std::collections::BTreeSet::from([ControlScope::List]),
                None,
            )
            .await
            .expect("owner grants List");

        // The named-principal console shares the same live handle
        // (ADJ-P5-10 deterministic lane); handle_for rebinds it to the
        // viewer principal at chokepoint (b).
        let viewer =
            MobMcpState::new_in_memory_as(MobControlPrincipal::External(viewer_id.clone()));
        viewer
            .mob_insert_handle(
                mob_id.clone(),
                owner.handle_for(&mob_id).await.expect("owner handle"),
            )
            .await;

        // List admits the two watch-read projections.
        viewer
            .mob_route_installs(&mob_id)
            .await
            .expect("List admits the route-install projection");
        viewer
            .mob_hosts(&mob_id)
            .await
            .expect("List admits the hosts projection");

        // AdminHost-gated host ceremony denies with the typed pair.
        let denied = viewer
            .mob_revoke_host(&mob_id, "host-peer-1")
            .await
            .expect_err("List-only principal cannot drive host ceremony");
        match denied.wire_detail() {
            Some(WireMobErrorDetail::ScopeDenied(detail)) => {
                assert_eq!(detail.required, WireControlScope::AdminHost);
                assert_eq!(detail.presented, vec![WireControlScope::List]);
            }
            other => panic!("expected typed AdminHost denial, got {other:?}"),
        }

        // ReadHistory gate on member_history, end-to-end (actor-gated).
        let denied = viewer
            .mob_member_history(&mob_id, AgentIdentity::from("worker-1"), None, None)
            .await
            .expect_err("List-only principal cannot read history");
        match denied.wire_detail() {
            Some(WireMobErrorDetail::ScopeDenied(detail)) => {
                assert_eq!(detail.required, WireControlScope::ReadHistory);
                assert_eq!(detail.presented, vec![WireControlScope::List]);
            }
            other => panic!("expected typed ReadHistory denial, got {other:?}"),
        }
    }

    fn assert_required_scope(error: &MobError, required: ControlScope) {
        match error {
            MobError::ScopeDenied(denial) => {
                assert_eq!(denial.required, required);
                assert!(denial.presented.is_empty());
            }
            other => panic!("expected ScopeDenied({required:?}), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn composite_surfaces_deny_before_raw_projection_or_fallback() {
        let owner = MobMcpState::new_in_memory();
        let mob_id = owner
            .mob_create_definition(explicit_definition("composite-scope-precedence"))
            .await
            .expect("create mob");
        owner
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("existing"),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn retained member fixture");

        let viewer = MobMcpState::new_in_memory_as(MobControlPrincipal::External(
            meerkat_core::auth::PrincipalId::new("composite-viewer").expect("principal"),
        ));
        viewer
            .mob_insert_handle(
                mob_id.clone(),
                owner.handle_for(&mob_id).await.expect("owner handle"),
            )
            .await;

        let append = viewer
            .mob_append_system_context(
                &mob_id,
                &AgentIdentity::from("missing"),
                AppendSystemContextRequest {
                    content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text("x"),
                    source: None,
                    idempotency_key: None,
                    source_kind: meerkat_core::session::SystemContextSource::Normal,
                    peer_response_terminal: None,
                },
            )
            .await
            .expect_err("authorization precedes member-session lookup");
        match append {
            MobAppendSystemContextError::Mob(error) => {
                assert_required_scope(&error, ControlScope::SendCommand);
            }
            other => panic!("expected mob-family denial, got {other:?}"),
        }

        assert_required_scope(
            &viewer
                .mob_events_strict(&mob_id, u64::MAX, 1)
                .await
                .expect_err("authorization precedes strict cursor validation"),
            ControlScope::SubscribeEvents,
        );
        assert_required_scope(
            &viewer
                .mob_ensure_member(&mob_id, SpawnMemberSpec::new("worker", "existing"))
                .await
                .expect_err("authorization precedes retain projection"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_reconcile(
                    &mob_id,
                    vec![SpawnMemberSpec::new("worker", "existing")],
                    meerkat_mob::runtime::ReconcileOptions::default(),
                )
                .await
                .expect_err("authorization precedes no-op reconcile"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_list_members_matching(&mob_id, meerkat_mob::runtime::MemberFilter::default())
                .await
                .expect_err("authorization precedes watch filtering"),
            ControlScope::List,
        );
        assert_required_scope(
            &viewer
                .mob_list_flows(&mob_id)
                .await
                .expect_err("authorization precedes definition projection"),
            ControlScope::List,
        );
        assert_required_scope(
            &viewer
                .mob_spawn_many(&mob_id, Vec::new())
                .await
                .expect_err("empty batch still authorizes"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_member_send(
                    &mob_id,
                    AgentIdentity::from("missing"),
                    ContentInput::from("hello"),
                    HandlingMode::Queue,
                    None,
                )
                .await
                .expect_err("authorization precedes missing-member lookup"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_ingress_interaction(
                    &mob_id,
                    SpawnMemberSpec::new("worker", "ingress-member"),
                    ContentInput::from("hello"),
                    HandlingMode::Queue,
                    None,
                )
                .await
                .expect_err("authorization precedes ingress cursor and ensure projections"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_turn_start_target(&mob_id, &AgentIdentity::from("missing"))
                .await
                .expect_err("authorization precedes turn target resolution"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_member_status(&mob_id, &AgentIdentity::from("existing"))
                .await
                .expect_err("authorization precedes member-status shortcuts"),
            ControlScope::List,
        );
        assert_required_scope(
            &viewer
                .mob_list_runs(&mob_id, None)
                .await
                .expect_err("authorization precedes run-store projection"),
            ControlScope::List,
        );
        assert_required_scope(
            &viewer
                .mob_spawn_helper(
                    &mob_id,
                    AgentIdentity::from("denied-helper"),
                    "should not spawn".to_string(),
                    meerkat_mob::HelperOptions::default(),
                )
                .await
                .expect_err("authorization precedes helper mechanics"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_cancel_work(&mob_id, meerkat_mob::WorkRef::new())
                .await
                .expect_err("authorization precedes unsupported capability result"),
            ControlScope::Cancel,
        );

        let implicit_id = owner
            .get_or_create_implicit_mob_for_bridge_session(
                &SessionId::new().to_string(),
                "claude-sonnet-4-5",
            )
            .await
            .expect("create implicit mob");
        viewer
            .mob_insert_handle(
                implicit_id.clone(),
                owner
                    .handle_for(&implicit_id)
                    .await
                    .expect("implicit owner handle"),
            )
            .await;
        let destroy = viewer
            .mob_destroy(&implicit_id)
            .await
            .expect_err("authorization precedes implicit-mob guard");
        match destroy {
            MobMcpDestroyError::Mob(error) => assert_required_scope(&error, ControlScope::Retire),
            other => panic!("expected mob-family destroy denial, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn composite_operations_do_not_require_unrelated_read_or_retire_scopes() {
        let owner = MobMcpState::new_in_memory();
        let mob_id = owner
            .mob_create_definition(explicit_definition("composite-scope-grants"))
            .await
            .expect("create mob");
        owner
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn member");
        let principal =
            meerkat_core::auth::PrincipalId::new("composite-operator").expect("principal");
        owner
            .mob_grant_scopes(
                &mob_id,
                principal.clone(),
                BTreeSet::from([
                    ControlScope::SendCommand,
                    ControlScope::SubscribeEvents,
                    ControlScope::Cancel,
                ]),
                None,
            )
            .await
            .expect("grant operation scopes without List or Retire");
        let operator =
            MobMcpState::new_in_memory_as(MobControlPrincipal::External(principal.clone()));
        operator
            .mob_insert_handle(
                mob_id.clone(),
                owner.handle_for(&mob_id).await.expect("owner handle"),
            )
            .await;

        operator
            .mob_member_binding_for_send_command(&mob_id, &AgentIdentity::from("worker-1"))
            .await
            .expect("write-target resolution must not require List");
        operator
            .mob_member_binding_for_cancel(&mob_id, &AgentIdentity::from("worker-1"))
            .await
            .expect("cancel-target resolution must not require List");
        operator
            .mob_ensure_member(
                &mob_id,
                SpawnMemberSpec::new("worker", "worker-1")
                    .with_runtime_mode(MobRuntimeMode::TurnDriven),
            )
            .await
            .expect("retained ensure requires SendCommand, not List");
        assert!(
            operator
                .mob_spawn_many(&mob_id, Vec::new())
                .await
                .expect("authorized empty batch")
                .is_empty()
        );
        assert!(matches!(
            operator
                .mob_cancel_work(&mob_id, meerkat_mob::WorkRef::new())
                .await,
            Err(MobError::WorkCancellationUnsupported(_))
        ));

        let reconcile_error = operator
            .mob_reconcile(
                &mob_id,
                vec![
                    SpawnMemberSpec::new("worker", "worker-1")
                        .with_runtime_mode(MobRuntimeMode::TurnDriven),
                ],
                meerkat_mob::runtime::ReconcileOptions { retire_stale: true },
            )
            .await
            .expect_err("retire-stale reconcile additionally requires Retire");
        match reconcile_error {
            MobError::ScopeDenied(denial) => {
                assert_eq!(denial.required, ControlScope::Retire);
                assert_eq!(
                    denial.presented,
                    BTreeSet::from([
                        ControlScope::SendCommand,
                        ControlScope::SubscribeEvents,
                        ControlScope::Cancel,
                    ])
                );
            }
            other => panic!("expected Retire denial, got {other:?}"),
        }

        let flow_error = operator
            .mob_run_flow(&mob_id, FlowId::from("missing-flow"), json!({}))
            .await
            .expect_err("fixture flow is absent after SendCommand admission");
        assert!(
            !matches!(flow_error, MobError::ScopeDenied(_)),
            "flow preview must not impose List on a SendCommand principal"
        );

        operator
            .mob_spawn_helper(
                &mob_id,
                AgentIdentity::from("helper-1"),
                "small helper task".to_string(),
                meerkat_mob::HelperOptions::default(),
            )
            .await
            .expect("helper mechanics must not require List or Retire after SendCommand admission");

        let empty_wait_mob = owner
            .mob_create_definition(explicit_definition("subscribe-only-wait"))
            .await
            .expect("create empty wait mob");
        owner
            .mob_grant_scopes(
                &empty_wait_mob,
                principal.clone(),
                BTreeSet::from([ControlScope::SubscribeEvents]),
                None,
            )
            .await
            .expect("grant SubscribeEvents");
        operator
            .mob_insert_handle(
                empty_wait_mob.clone(),
                owner
                    .handle_for(&empty_wait_mob)
                    .await
                    .expect("empty wait handle"),
            )
            .await;
        assert!(
            operator
                .mob_wait_ready(&empty_wait_mob, None, Some(10))
                .await
                .expect("SubscribeEvents-only wait must not require List")
                .is_empty()
        );
    }
}
