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
    WireMobLifecycleAction,
};
use meerkat_core::AppendSystemContextStatus;
use meerkat_core::ScopedAgentEvent;
use meerkat_core::agent::{AgentToolDispatcher, CommsRuntime as CoreCommsRuntime};
use meerkat_core::comms::{
    CommsCommand, PeerId, PeerName, SendError, SendReceipt, TrustedPeerDescriptor,
};
use meerkat_core::error::ToolError;
use meerkat_core::interaction::{InteractionId, PeerInputCandidate};
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
use meerkat_mob::{
    AgentIdentity, FlowId, MobBackendKind, MobBuilder, MobDefinition, MobError, MobHandle, MobId,
    MobRuntimeMode, MobSessionService, MobState, MobStorage, ProfileName, RunId, SpawnMemberSpec,
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet, btree_map::Entry};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
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

type DefaultLlmClientProvider = Arc<dyn Fn() -> Option<Arc<dyn LlmClient>> + Send + Sync + 'static>;

#[derive(Debug, Clone, serde::Serialize)]
pub struct KickoffMemberSnapshot {
    pub agent_identity: AgentIdentity,
    #[serde(flatten)]
    pub snapshot: meerkat_mob::MobMemberSnapshot,
}

fn persisted_mob_binding(session: &meerkat_core::Session) -> Option<meerkat_mob::MobId> {
    let metadata = session.session_metadata()?;
    let comms_name = metadata.comms_name.as_deref()?;
    let mut parts = comms_name.split('/');
    let mob_id = parts.next().filter(|part| !part.is_empty())?;
    let profile = parts.next().filter(|part| !part.is_empty())?;
    let meerkat_id = parts.next().filter(|part| !part.is_empty())?;
    if parts.next().is_some() {
        return None;
    }
    if metadata.realm_id.as_deref() != Some(&format!("mob:{mob_id}")) {
        return None;
    }
    let peer_meta = metadata.peer_meta.as_ref()?;
    if peer_meta.labels.get("mob_id").map(String::as_str) != Some(mob_id)
        || peer_meta.labels.get("role").map(String::as_str) != Some(profile)
        || peer_meta.labels.get("meerkat_id").map(String::as_str) != Some(meerkat_id)
    {
        return None;
    }
    Some(meerkat_mob::MobId::from(mob_id))
}

/// In-memory MCP state for multiple mobs.
pub struct MobMcpState {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    realtime_rpc_tcp_addr: StdMutex<Option<String>>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
    default_llm_client_provider: Option<DefaultLlmClientProvider>,
    external_tools_provider: Option<meerkat_mob::ExternalToolsProvider>,
    persistent_storage_root: Option<PathBuf>,
    mobs: RwLock<BTreeMap<MobId, ManagedMob>>,
    /// Per-session locks for single-flight implicit mob creation.
    implicit_mob_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    restore_lock: Mutex<bool>,
    /// Shared realm-scoped profile store for cross-mob profile CRUD.
    realm_profile_store: Option<Arc<dyn meerkat_mob::RealmProfileStore>>,
    /// Whether the current realm profile store was explicitly supplied by the caller.
    ///
    /// Default constructor wiring installs an in-memory store so profile CRUD is
    /// available on ephemeral surfaces. Persistent roots may upgrade that
    /// default to SQLite, but must not override an explicit caller-owned store.
    realm_profile_store_explicit: bool,
}

impl MobMcpState {
    pub fn new(session_service: Arc<dyn MobSessionService>) -> Self {
        let runtime_adapter = session_service.runtime_adapter();
        Self::new_with_runtime_adapter(session_service, runtime_adapter)
    }

    pub fn new_with_runtime_adapter(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    ) -> Self {
        Self {
            session_service,
            runtime_adapter,
            realtime_rpc_tcp_addr: StdMutex::new(None),
            default_llm_client: None,
            default_llm_client_provider: None,
            external_tools_provider: None,
            persistent_storage_root: None,
            mobs: RwLock::new(BTreeMap::new()),
            implicit_mob_locks: Mutex::new(HashMap::new()),
            restore_lock: Mutex::new(false),
            realm_profile_store: Some(Arc::new(meerkat_mob::InMemoryRealmProfileStore::new())),
            realm_profile_store_explicit: false,
        }
    }

    /// Set the shared realm profile store for cross-mob profile CRUD.
    pub fn with_realm_profile_store(
        mut self,
        store: Option<Arc<dyn meerkat_mob::RealmProfileStore>>,
    ) -> Self {
        self.realm_profile_store = store;
        self.realm_profile_store_explicit = true;
        self
    }

    /// Returns a reference to the realm profile store, if configured.
    pub fn realm_profile_store(&self) -> Option<&Arc<dyn meerkat_mob::RealmProfileStore>> {
        self.realm_profile_store.as_ref()
    }

    pub fn with_persistent_storage_root(mut self, runtime_root: Option<PathBuf>) -> Self {
        self.persistent_storage_root = runtime_root.map(|root| {
            let mob_root = Self::persistent_mob_root(&root);
            // Auto-create realm profile store when persistent storage is available.
            #[cfg(not(target_arch = "wasm32"))]
            if !self.realm_profile_store_explicit {
                let db_path = mob_root.join("realm_profiles.db");
                match meerkat_mob::SqliteRealmProfileStore::open(&db_path) {
                    Ok(store) => {
                        self.realm_profile_store =
                            Some(Arc::new(store) as Arc<dyn meerkat_mob::RealmProfileStore>);
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

    pub fn with_realtime_rpc_tcp_addr(self, addr: Option<String>) -> Self {
        *self
            .realtime_rpc_tcp_addr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = addr;
        self
    }

    pub fn set_realtime_rpc_tcp_addr(&self, addr: Option<String>) {
        *self
            .realtime_rpc_tcp_addr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = addr;
    }

    pub fn realtime_rpc_tcp_addr(&self) -> Option<String> {
        self.realtime_rpc_tcp_addr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
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
            .with_default_external_tools_provider(self.external_tools_provider.clone());
        if let Some(adapter) = &self.runtime_adapter {
            builder = builder.with_runtime_adapter(adapter.clone());
        }
        if let Some(client) = self.default_llm_client_snapshot() {
            builder = builder.with_default_llm_client(client);
        }
        builder
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
            let storage = MobStorage::persistent(&path)?;
            return Ok((storage, Some(path)));
        }

        Ok((MobStorage::in_memory(), None))
    }

    async fn maybe_remove_storage_file(path: Option<&Path>) {
        #[cfg(target_arch = "wasm32")]
        let _ = path;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = path
            && let Err(error) = tokio::fs::remove_file(path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), error = %error, "failed to remove mob storage file");
        }
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

                let storage = MobStorage::persistent(&path)?;
                if storage.events.replay_all().await?.is_empty() {
                    Self::maybe_remove_storage_file(Some(path.as_path())).await;
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
        let mob_id = definition.id.clone();
        self.ensure_restored().await?;
        if self.mobs.read().await.contains_key(&mob_id) {
            return Err(MobError::Internal(format!("mob already exists: {mob_id}")));
        }
        let (storage, storage_path) = self.storage_for_new_mob(&mob_id).await?;
        let handle = self
            .configure_builder(MobBuilder::new(definition.clone(), storage))
            .create()
            .await?;
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
                Self::maybe_remove_storage_file(storage_path.as_deref()).await;
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

    pub async fn mob_list(&self) -> Vec<(MobId, MobState)> {
        if !self.ensure_restored_best_effort("mob_list").await {
            return Vec::new();
        }
        let snapshot: Vec<(MobId, meerkat_mob::MobHandle)> = self
            .mobs
            .read()
            .await
            .iter()
            .map(|(id, managed)| (id.clone(), managed.handle.clone()))
            .collect();
        let mut out = Vec::with_capacity(snapshot.len());
        for (id, handle) in snapshot {
            match handle.status().await {
                Ok(state) => out.push((id, state)),
                Err(err) => {
                    tracing::warn!(mob_id = %id, error = %err, "mob_list: skipping mob whose status read failed");
                }
            }
        }
        out
    }

    pub async fn mob_status(&self, mob_id: &MobId) -> Result<MobState, MobError> {
        let handle = self.handle_for(mob_id).await?;
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
    ) -> Result<Option<meerkat_mob::MobDestroyReport>, MobError> {
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
    /// Returns the structured [`meerkat_mob::MobDestroyReport`] so RPC/MCP
    /// surfaces can project every cleanup result (force-destroyed members,
    /// orphaned remote members, deadline exceeded, partial errors) — the
    /// report was previously dropped on the floor, leaving mobkit and the
    /// RPC client unable to tell partial failures from clean destroys.
    pub async fn mob_destroy(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobDestroyReport, MobError> {
        if self.is_implicit_mob(mob_id).await {
            return Err(MobError::Internal(
                "Cannot destroy implicit delegation mob directly. \
                 It is cleaned up automatically when the owning session is archived."
                    .to_string(),
            ));
        }
        self.mob_destroy_unchecked(mob_id).await
    }

    /// Destroy a mob without the implicit-mob guard.
    ///
    /// Used by session cleanup paths and canonical implicit-mob reconciliation.
    pub(crate) async fn mob_destroy_unchecked(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobDestroyReport, MobError> {
        self.ensure_restored().await?;
        self.mob_destroy_unchecked_loaded(mob_id).await
    }

    async fn mob_destroy_unchecked_loaded(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobDestroyReport, MobError> {
        let managed = {
            let mut mobs = self.mobs.write().await;
            mobs.remove(mob_id)
                .ok_or_else(|| MobError::Internal(format!("mob not found: {mob_id}")))?
        };

        match managed.handle.destroy().await {
            Ok(report) => {
                Self::maybe_remove_storage_file(managed.storage_path.as_deref()).await;
                Ok(report)
            }
            Err(meerkat_mob::MobDestroyError::Incomplete { report }) => {
                // Partial cleanup is a successful destroy with non-empty
                // errors. Finding A9: callers shouldn't have to match on an
                // Err variant to read the report — every consumer either
                // ignored the report or did the match dance. Keep the
                // storage file cleanup as-is (destroy attempt reached the
                // reporting stage, so the storage side is considered
                // finalised) and surface the report with its errors
                // populated.
                Self::maybe_remove_storage_file(managed.storage_path.as_deref()).await;
                Ok(report)
            }
            Err(meerkat_mob::MobDestroyError::Mob(error)) => {
                let mut mobs = self.mobs.write().await;
                match mobs.entry(mob_id.clone()) {
                    Entry::Vacant(entry) => {
                        entry.insert(managed);
                    }
                    Entry::Occupied(_) => {
                        tracing::warn!(
                            mob_id = %mob_id,
                            "mob destroy failed after a replacement mob with the same id was inserted; preserving replacement"
                        );
                    }
                }
                Err(error)
            }
            Err(other) => {
                // MobDestroyError is #[non_exhaustive]; future variants we
                // haven't coded for fall through to a generic internal
                // error so the caller still gets a readable message.
                let mut mobs = self.mobs.write().await;
                match mobs.entry(mob_id.clone()) {
                    Entry::Vacant(entry) => {
                        entry.insert(managed);
                    }
                    Entry::Occupied(_) => {
                        tracing::warn!(
                            mob_id = %mob_id,
                            "mob destroy failed after a replacement mob with the same id was inserted; preserving replacement"
                        );
                    }
                }
                Err(MobError::Internal(format!("mob destroy failed: {other}")))
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
    ) -> Result<meerkat_mob::SpawnResult, MobError> {
        let mut spec = SpawnMemberSpec::new(profile, identity);
        spec.runtime_mode = runtime_mode;
        spec.backend = backend;
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
    ) -> Result<Vec<Result<meerkat_mob::SpawnResult, MobError>>, MobError> {
        Ok(self.handle_for(mob_id).await?.spawn_many(specs).await)
    }

    pub async fn mob_retire(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.retire(identity).await
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
            if self.owns_persisted_bridge_session(bridge_session_id).await {
                return self
                    .session_service()
                    .archive(bridge_session_id)
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to archive persisted mob-owned bridge session '{bridge_session_id}': {error}"
                        ))
                    });
            }
            return Err(MobError::Internal(format!(
                "bridge session not found in any live mob authority: {bridge_session_id}"
            )));
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
    pub async fn owns_persisted_bridge_session(&self, bridge_session_id: &SessionId) -> bool {
        let Some(session) = self
            .session_service()
            .load_persisted_session(bridge_session_id)
            .await
            .ok()
            .flatten()
        else {
            return false;
        };

        let Some(mob_id) = persisted_mob_binding(&session) else {
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
        self.handle_for(mob_id)
            .await?
            .list_members_including_retiring()
            .await
            .pipe(Ok)
    }

    pub async fn mob_append_system_context(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        req: AppendSystemContextRequest,
    ) -> Result<(SessionId, AppendSystemContextResult), SessionControlError> {
        let handle =
            self.handle_for(mob_id)
                .await
                .map_err(|error| SessionControlError::InvalidRequest {
                    message: error.to_string(),
                })?;
        let bridge_session_id = handle
            .resolve_bridge_session_id(identity)
            .await
            .ok_or_else(|| SessionControlError::InvalidRequest {
                message: format!("member has no session: {identity}"),
            })?;
        let result = self
            .session_service()
            .append_system_context(&bridge_session_id, req)
            .await?;
        Ok((bridge_session_id, result))
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
        self.handle_for(mob_id)
            .await?
            .member(&identity)
            .await?
            .send_with_render_metadata(content, handling_mode, render_metadata)
            .await
    }

    pub async fn mob_events(
        &self,
        mob_id: &MobId,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<meerkat_mob::MobEvent>, MobError> {
        self.handle_for(mob_id)
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
        self.handle_for(mob_id)
            .await?
            .events()
            .poll_strict(after_cursor, limit)
            .await
    }

    pub async fn mob_latest_event_cursor(&self, mob_id: &MobId) -> Result<u64, MobError> {
        self.handle_for(mob_id)
            .await?
            .events()
            .latest_cursor()
            .await
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

    /// Cancel a previously submitted unit of work. Finding C4.
    pub async fn mob_cancel_work(
        &self,
        mob_id: &MobId,
        work_ref: meerkat_mob::WorkRef,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.cancel_work(work_ref).await
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
        let flows = self.handle_for(mob_id).await?.list_flows();
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
        self.handle_for(mob_id).await?.member_status(identity).await
    }

    pub async fn realtime_validate_session_target(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.session_service.read(session_id).await.map(|_| ())
    }

    pub async fn realtime_session_realtime_attachment_status(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_runtime::RealtimeAttachmentStatus, SessionError> {
        self.realtime_validate_session_target(session_id).await?;
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            SessionError::Unsupported(
                "runtime adapter unavailable for realtime target inspection".to_string(),
            )
        })?;
        adapter
            .realtime_attachment_status(session_id)
            .await
            .map_err(|err| SessionError::Unsupported(err.to_string()))
    }

    /// Wave-c C-9c R4: fully-projected public channel status for MCP
    /// `meerkat_realtime_status`. Reads DSL state (attachment +
    /// reconnect-progress) through the runtime adapter.
    pub async fn realtime_session_realtime_channel_status(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_contracts::RealtimeChannelStatus, SessionError> {
        self.realtime_validate_session_target(session_id).await?;
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            SessionError::Unsupported(
                "runtime adapter unavailable for realtime channel status inspection".to_string(),
            )
        })?;
        adapter
            .realtime_channel_status(session_id)
            .await
            .map_err(|err| SessionError::Unsupported(err.to_string()))
    }

    pub async fn mob_wait_kickoff(
        &self,
        mob_id: &MobId,
        member_ids: Option<Vec<AgentIdentity>>,
        timeout_ms: Option<u64>,
    ) -> Result<Vec<KickoffMemberSnapshot>, MobError> {
        let handle = self.handle_for(mob_id).await?;
        let timeout = timeout_ms.map(Duration::from_millis);
        let snapshots = match member_ids {
            Some(ids) => {
                handle
                    .wait_for_members_kickoff_complete(&ids, timeout)
                    .await?
            }
            None => handle.wait_for_kickoff_complete(timeout).await?,
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
        let handle = self.handle_for(mob_id).await?;
        let timeout = timeout_ms.map(Duration::from_millis);
        let snapshots = match member_ids {
            Some(ids) => handle.wait_for_members_ready(&ids, timeout).await?,
            None => handle.wait_for_ready(timeout).await?,
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
    pub async fn subscribe_mob_events(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobEventRouterHandle, MobError> {
        Ok(self.handle_for(mob_id).await?.subscribe_mob_events().await)
    }

    /// Subscribe to agent-level events for a specific member.
    pub async fn subscribe_agent_events(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<meerkat_core::comms::EventStream, MobError> {
        self.handle_for(mob_id)
            .await?
            .subscribe_agent_events(identity)
            .await
    }

    /// Find the implicit delegation mob for the given bridge session, if one exists.
    ///
    /// Scans the in-memory mob registry for a mob whose definition has
    /// `is_implicit == true` and an owner bridge-session index matching the
    /// given bridge session ID.
    /// Does NOT match explicit mobs that merely share the same owner.
    #[doc(hidden)]
    pub async fn find_implicit_mob_for_bridge_session(
        &self,
        bridge_session_id: &str,
    ) -> Option<MobId> {
        if !self
            .ensure_restored_best_effort("find_implicit_mob_for_bridge_session")
            .await
        {
            return None;
        }
        let mobs = self.mobs.read().await;
        mobs.iter()
            .find(|(_, m)| {
                let def = m.handle.definition();
                def.is_implicit && def.has_owner_bridge_session_index(bridge_session_id)
            })
            .map(|(id, _)| id.clone())
    }

    /// Check whether the given mob is an implicit delegation mob.
    ///
    /// Checks the canonical `is_implicit` field on the mob definition.
    #[doc(hidden)]
    pub async fn is_implicit_mob(&self, mob_id: &MobId) -> bool {
        if !self.ensure_restored_best_effort("is_implicit_mob").await {
            return false;
        }
        let mobs = self.mobs.read().await;
        mobs.get(mob_id)
            .map(|m| m.handle.definition().is_implicit)
            .unwrap_or(false)
    }

    /// Find all mobs indexed to the given owner bridge session
    /// (both implicit and explicit).
    #[doc(hidden)]
    pub async fn find_mobs_for_bridge_session(&self, bridge_session_id: &str) -> Vec<MobId> {
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
                    .definition()
                    .has_owner_bridge_session_index(bridge_session_id)
                {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    async fn find_bridge_session_scoped_mobs(&self, bridge_session_id: &str) -> Vec<MobId> {
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
                    .definition()
                    .is_cleanup_scoped_to_owner_bridge_session(bridge_session_id)
                {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    async fn implicit_mob_matches_session_model(
        &self,
        mob_id: &MobId,
        bridge_session_id: &str,
        model: &str,
    ) -> bool {
        let delegate_profile = ProfileName::from("delegate");
        let mobs = self.mobs.read().await;
        mobs.get(mob_id).is_some_and(|managed| {
            let definition = managed.handle.definition();
            definition.is_implicit
                && definition.has_owner_bridge_session_index(bridge_session_id)
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
            self.mob_destroy_unchecked(&mob_id).await?;
        }

        let mob_id = self
            .mob_create_definition(MobDefinition::implicit(bridge_session_id, model))
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
    ) -> Result<(), MobError> {
        self.ensure_restored().await?;
        let mob_ids = self
            .find_bridge_session_scoped_mobs(bridge_session_id)
            .await;
        if mob_ids.is_empty() {
            return Ok(());
        }
        for mob_id in &mob_ids {
            if let Err(error) = self.mob_destroy_unchecked(mob_id).await {
                tracing::warn!(
                    mob_id = %mob_id,
                    bridge_session_id = %bridge_session_id,
                    error = %error,
                    "failed to destroy bridge-session-scoped mob during cleanup"
                );
            }
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
        let candidates: Vec<(MobId, String)> = {
            let mobs = self.mobs.read().await;
            mobs.iter()
                .filter_map(|(id, m)| {
                    let definition = m.handle.definition();
                    if definition.session_cleanup_policy
                        == meerkat_mob::definition::SessionCleanupPolicy::DestroyOnOwnerArchive
                    {
                        definition
                            .owner_bridge_session_index()
                            .map(|sid| (id.clone(), sid.to_string()))
                    } else {
                        None
                    }
                })
                .collect()
        };

        let mut scavenged = Vec::new();
        for (mob_id, bridge_session_id) in candidates {
            let bridge_session_id = match SessionId::parse(&bridge_session_id) {
                Ok(id) => id,
                Err(_) => continue,
            };
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
    /// Returns `MobError::Internal` if the mob is not found.
    pub async fn handle_for(&self, mob_id: &MobId) -> Result<MobHandle, MobError> {
        self.ensure_restored().await?;
        self.mobs
            .read()
            .await
            .get(mob_id)
            .map(|m| m.handle.clone())
            .ok_or_else(|| MobError::Internal(format!("mob not found: {mob_id}")))
    }

    /// W3-H: resolve a realtime channel target to the concrete bridge
    /// session id. `SessionTarget` returns its parsed session id directly;
    /// `MobMember` resolves `(mob_id, agent_identity)` against the
    /// MobMachine's canonical `member_session_bindings` map via the mob
    /// handle — a single read through the authoritative source (dogma #1).
    ///
    /// Surfaces that receive a `RealtimeChannelTarget` from a caller route
    /// through this resolver to get the concrete session id they need for
    /// downstream runtime-adapter queries. Keeps each surface's MobMember
    /// handling uniform.
    pub async fn resolve_realtime_target_session(
        &self,
        target: &meerkat_contracts::RealtimeChannelTarget,
    ) -> Result<meerkat_core::types::SessionId, MobError> {
        match target {
            meerkat_contracts::RealtimeChannelTarget::SessionTarget { session_id } => {
                meerkat_core::types::SessionId::parse(session_id)
                    .map_err(|err| MobError::Internal(format!("invalid session target: {err}")))
            }
            meerkat_contracts::RealtimeChannelTarget::MobMember {
                mob_id,
                agent_identity,
            } => {
                let mob_id = MobId::from(mob_id.as_str());
                let handle = self.handle_for(&mob_id).await?;
                let identity = AgentIdentity::from(agent_identity.as_str());
                handle
                    .current_realtime_binding(identity.clone())
                    .await?
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "mob {mob_id} has no realtime binding for identity {identity}"
                        ))
                    })
            }
        }
    }

    // ─── Realm profile CRUD ──────────────────────────────────────────

    fn require_realm_profile_store(
        &self,
    ) -> Result<&Arc<dyn meerkat_mob::RealmProfileStore>, meerkat_mob::MobError> {
        self.realm_profile_store.as_ref().ok_or_else(|| {
            meerkat_mob::MobError::Internal("realm profile store not configured".to_string())
        })
    }

    /// Create a new realm profile.
    pub async fn realm_profile_create(
        &self,
        name: &str,
        profile: &meerkat_mob::Profile,
    ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobError> {
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
        let store = self.require_realm_profile_store()?;
        store
            .delete(name, expected_revision)
            .await
            .map_err(|e| meerkat_mob::MobError::Internal(e.to_string()))
    }

    /// Create MCP state backed by an in-memory local session service.
    pub fn new_in_memory() -> Arc<Self> {
        let state = Self::new_with_runtime_adapter(
            Arc::new(LocalSessionService::new()),
            Some(Arc::new(meerkat_runtime::MeerkatMachine::ephemeral())),
        );
        Arc::new(state)
    }
}

struct LocalCommsRuntime {
    peer_id: PeerId,
    public_key_bytes: [u8; 32],
    address: String,
    key: String,
    trusted: RwLock<HashSet<String>>,
    notify: Arc<tokio::sync::Notify>,
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
            peer_id,
            public_key_bytes,
            address: format!("inproc://{name}"),
            key: encode_ed25519_public_key(&public_key_bytes),
            trusted: RwLock::new(HashSet::new()),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }
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

    fn advertised_address(&self) -> Option<String> {
        Some(self.address.clone())
    }

    async fn add_trusted_peer(&self, peer: TrustedPeerDescriptor) -> Result<(), SendError> {
        TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
            .map_err(SendError::Validation)?;
        self.trusted
            .write()
            .await
            .insert(peer.peer_id.as_str().to_string());
        Ok(())
    }

    async fn add_private_trusted_peer(&self, peer: TrustedPeerDescriptor) -> Result<(), SendError> {
        TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
            .map_err(SendError::Validation)?;
        Ok(())
    }

    async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
        Ok(self.trusted.write().await.remove(peer_id))
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

struct LocalSessionService {
    sessions: RwLock<HashMap<SessionId, Arc<LocalCommsRuntime>>>,
    archived_views: RwLock<HashMap<SessionId, SessionView>>,
    pending_context: RwLock<HashMap<SessionId, Vec<AppendSystemContextRequest>>>,
    /// Per-session broadcast channels for event streaming.
    event_txs:
        RwLock<HashMap<SessionId, tokio::sync::broadcast::Sender<EventEnvelope<AgentEvent>>>>,
    counter: std::sync::atomic::AtomicU64,
    archive_delay_ms: std::sync::atomic::AtomicU64,
}

impl LocalSessionService {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            archived_views: RwLock::new(HashMap::new()),
            pending_context: RwLock::new(HashMap::new()),
            event_txs: RwLock::new(HashMap::new()),
            counter: std::sync::atomic::AtomicU64::new(0),
            archive_delay_ms: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn set_archive_delay_ms(&self, delay_ms: u64) {
        self.archive_delay_ms
            .store(delay_ms, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionService for LocalSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let sid = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            .map(|session| session.id().clone())
            .unwrap_or_default();
        let n = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = req
            .build
            .and_then(|b| b.comms_name)
            .unwrap_or_else(|| format!("session-{n}"));
        self.sessions
            .write()
            .await
            .insert(sid.clone(), Arc::new(LocalCommsRuntime::new(&name)));
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
            structured_output: None,
            schema_warnings: None,
            skill_diagnostics: None,
        })
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
                    Some(source) => format!("[SYSTEM CONTEXT:{source}] {}", append.text),
                    None => format!("[SYSTEM CONTEXT] {}", append.text),
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
            let source_id = id.to_string();
            let mut seq = 1u64;
            let _ = event_tx.send(EventEnvelope::new(
                source_id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::RunStarted {
                    session_id: id.clone(),
                    prompt: effective_prompt.clone(),
                },
            ));
            let _ = event_tx.send(EventEnvelope::new(
                source_id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::TurnStarted { turn_number: 1 },
            ));
            let usage = Usage::default();
            let turn_usage = usage.clone();
            let _ = event_tx.send(EventEnvelope::new(
                source_id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::types::StopReason::EndTurn,
                    usage: turn_usage,
                },
            ));
            let _ = event_tx.send(EventEnvelope::new(
                source_id,
                next_seq(&mut seq),
                None,
                AgentEvent::RunCompleted {
                    session_id: id.clone(),
                    result: "ok".to_string(),
                    usage,
                },
            ));
        }
        Ok(RunResult {
            text: "ok".to_string(),
            session_id: id.clone(),
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            structured_output: None,
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
        let archive_delay_ms = self
            .archive_delay_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        if archive_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(archive_delay_ms)).await;
        }
        let removed = self.sessions.write().await.remove(id);
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
            .map(|session| session.clone() as Arc<dyn CoreCommsRuntime>)
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        let sessions = self.sessions.read().await;
        let runtime = sessions.get(session_id)?;
        runtime.event_injector()
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        let sessions = self.sessions.read().await;
        let runtime = sessions.get(session_id)?;
        runtime.interaction_event_injector()
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

    fn runtime_adapter(&self) -> Option<std::sync::Arc<meerkat_runtime::MeerkatMachine>> {
        Some(std::sync::Arc::new(
            meerkat_runtime::MeerkatMachine::ephemeral(),
        ))
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
                meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt {
                    run_id,
                    boundary,
                    contributing_input_ids,
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
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
        Arc::new(Self::new(session_service))
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
                     initial_message, labels (key-value map), context (opaque JSON). {COMMON}"),
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
                                    "context":{"type":"object"}
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
                    "Wait until mob startup readiness (members bound but kickoff not required). Optional: member_ids (subset), timeout_ms. Returns member snapshots. {COMMON}"
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
        Self { state, tools }
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
    serde_json::to_value(schemars::schema_for!(MobLifecycleParams))
        .unwrap_or_else(|_| json!({ "type": "object" }))
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
    action: String,
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
                pubkey: Some(resolved.pubkey),
            })
        }
    }
}

impl WireActionArgs {
    fn resolve(self) -> Result<(String, AgentIdentity, meerkat_mob::PeerTarget, String), String> {
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
                if action == "wire" {
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
                    encode(call, json!({"status": status.as_str()}))
                } else {
                    let mobs = self.state.mob_list().await;
                    encode(
                        call,
                        json!({"mobs": mobs.into_iter().map(|(id, status)| json!({"mob_id": id, "status": status.as_str()})).collect::<Vec<_>>() }),
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
                    .map_err(|e| map_mob_err(call, e))?;
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
                encode(call, json!({"run": run}))
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
                        |result: Result<meerkat_mob::SpawnResult, meerkat_mob::MobError>| {
                            match result {
                                Ok(spawn_result) => {
                                    let identity = spawn_result.agent_identity.to_string();
                                    json!(MobSpawnManyResultEntry::spawned(
                                        identity.clone(),
                                        WireMemberRef::encode(mob_id.as_str(), &identity),
                                    ))
                                }
                                Err(error) => {
                                    json!(MobSpawnManyResultEntry::failed(error.to_string()))
                                }
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
                match action.as_str() {
                    "wire" => self
                        .state
                        .mob_wire(&mob_id, local, target)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                    "unwire" => self
                        .state
                        .mob_unwire(&mob_id, local, target)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                    other => {
                        return Err(ToolError::invalid_arguments(
                            call.name,
                            format!("unknown wire action: {other}"),
                        ));
                    }
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
                            "status": "completed",
                            "receipt": receipt,
                        }),
                    ),
                    Err(meerkat_mob::MobRespawnError::TopologyRestoreFailed {
                        receipt,
                        failed_peer_ids,
                    }) => encode(
                        call,
                        json!({
                            "status": "topology_restore_failed",
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
                let snapshot = self
                    .state
                    .mob_member_status(
                        &MobId::from(args.mob_id),
                        &AgentIdentity::from(args.agent_identity),
                    )
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!(snapshot))
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
                let members = self
                    .state
                    .mob_wait_ready(&MobId::from(args.mob_id), member_ids, args.timeout_ms)
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
            data: None,
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
    use meerkat_core::comms::{CommsCommand, SendError, SendReceipt};
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
    use std::collections::{HashMap, HashSet};
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
        assert_eq!(pubkey, Some(expected_pubkey));
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
            action: "wire".to_string(),
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
            action: "wire".to_string(),
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
            action: "unwire".to_string(),
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
        peer_id: meerkat_core::comms::PeerId,
        public_key_bytes: [u8; 32],
        address: String,
        key: String,
        trusted: RwLock<HashSet<String>>,
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
                    })
                    .await;
            });
            Ok(InteractionSubscription {
                id: interaction_id,
                events: rx,
            })
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
                peer_id,
                public_key_bytes,
                address: format!("inproc://{name}"),
                key: super::encode_ed25519_public_key(&public_key_bytes),
                trusted: RwLock::new(HashSet::new()),
                notify: Arc::new(Notify::new()),
            }
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

        fn advertised_address(&self) -> Option<String> {
            Some(self.address.clone())
        }

        async fn add_trusted_peer(
            &self,
            peer: meerkat_core::comms::TrustedPeerDescriptor,
        ) -> Result<(), SendError> {
            meerkat_core::comms::TrustedPeerDescriptor::validate_pubkey_for_peer_id(
                peer.peer_id,
                &peer.pubkey,
            )
            .map_err(SendError::Validation)?;
            self.trusted
                .write()
                .await
                .insert(peer.peer_id.as_str().to_string());
            Ok(())
        }

        async fn add_private_trusted_peer(
            &self,
            peer: meerkat_core::comms::TrustedPeerDescriptor,
        ) -> Result<(), SendError> {
            meerkat_core::comms::TrustedPeerDescriptor::validate_pubkey_for_peer_id(
                peer.peer_id,
                &peer.pubkey,
            )
            .map_err(SendError::Validation)?;
            Ok(())
        }

        async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
            Ok(self.trusted.write().await.remove(peer_id))
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

    struct MockSessionSvc {
        sessions: RwLock<HashMap<SessionId, Arc<MockComms>>>,
        persisted_sessions: RwLock<HashMap<SessionId, Session>>,
        keep_alive_notifiers: RwLock<HashMap<SessionId, Arc<Notify>>>,
        counter: AtomicU64,
        start_turn_delay_ms: AtomicU64,
        runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    }

    impl MockSessionSvc {
        fn new() -> Self {
            Self {
                sessions: RwLock::new(HashMap::new()),
                persisted_sessions: RwLock::new(HashMap::new()),
                keep_alive_notifiers: RwLock::new(HashMap::new()),
                counter: AtomicU64::new(0),
                start_turn_delay_ms: AtomicU64::new(0),
                runtime_adapter: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
            }
        }

        fn set_turn_delay_ms(&self, delay_ms: u64) {
            self.start_turn_delay_ms.store(delay_ms, Ordering::Relaxed);
        }

        async fn insert_persisted_session(&self, session: Session) {
            self.persisted_sessions
                .write()
                .await
                .insert(session.id().clone(), session);
        }
    }

    #[async_trait]
    impl SessionService for MockSessionSvc {
        async fn create_session(
            &self,
            req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            let sid = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.as_ref())
                .map(|session| session.id().clone())
                .unwrap_or_default();
            let n = self.counter.fetch_add(1, Ordering::Relaxed);
            let is_keep_alive = req.build.as_ref().map(|b| b.keep_alive).unwrap_or(false);
            let name = req
                .build
                .and_then(|b| b.comms_name)
                .unwrap_or_else(|| format!("s-{n}"));
            self.sessions
                .write()
                .await
                .insert(sid.clone(), Arc::new(MockComms::new(&name)));
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
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        async fn start_turn(
            &self,
            id: &SessionId,
            _req: StartTurnRequest,
        ) -> Result<RunResult, SessionError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
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
                structured_output: None,
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
            let removed_live = self.sessions.write().await.remove(id).is_some();
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
                .map(|s| s.clone() as Arc<dyn CoreCommsRuntime>)
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
        fn supports_persistent_sessions(&self) -> bool {
            true
        }

        fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
            Some(self.runtime_adapter.clone())
        }

        async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
            true
        }

        async fn load_persisted_session(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<Session>, SessionError> {
            Ok(self
                .persisted_sessions
                .read()
                .await
                .get(session_id)
                .cloned())
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
                    meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt {
                        run_id,
                        boundary,
                        contributing_input_ids,
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
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
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
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
                    text: "Remember the customer preference.".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-1".to_string()),
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
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
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
                    text: "Remember the customer preference.".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-1".to_string()),
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
                    prompt: "hello".to_string().into(),
                    system_prompt: None,
                    render_metadata: None,
                    handling_mode: HandlingMode::Queue,
                    event_tx: None,
                    skill_references: None,
                    flow_tool_overlay: None,
                    pre_turn_context_appends: Vec::new(),
                    turn_metadata: None,
                },
            )
            .await
            .expect("start turn");

        let first = stream.next().await.expect("first event");
        match first.payload {
            AgentEvent::RunStarted { prompt, .. } => {
                let prompt = prompt.text_content();
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
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
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
                    text: "Remember the picture.".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-image".to_string()),
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
                    Some(source) => format!("[SYSTEM CONTEXT:{source}] {}", append.text),
                    None => format!("[SYSTEM CONTEXT] {}", append.text),
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
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
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
                    text: "Remember the customer preference.".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-archive".to_string()),
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
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
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
                    prompt: "hello".to_string().into(),
                    system_prompt: None,
                    render_metadata: None,
                    handling_mode: HandlingMode::Queue,
                    event_tx: None,
                    skill_references: None,
                    flow_tool_overlay: None,
                    pre_turn_context_appends: Vec::new(),
                    turn_metadata: None,
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

    #[tokio::test]
    async fn test_dispatcher_exposes_expected_tools() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
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
        let state = Arc::new(MobMcpState::new(svc));
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
        let state = Arc::new(MobMcpState::new(svc));
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
        let state = Arc::new(MobMcpState::new(session_service));
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
                            "model": "claude-opus-4-6",
                            "tools": {"comms": true, "mob": true},
                            "external_addressable": true
                        }
                    },
                    "mcp_servers": {},
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
            comms_name: Some("team/reviewer/alice".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            connection_ref: None,
        });
        svc.insert_persisted_session(spoofed).await;

        assert!(
            !state.owns_persisted_bridge_session(&spoofed_id).await,
            "persisted session routing must verify real mob membership instead of trusting comms_name shape"
        );
    }

    #[tokio::test]
    async fn test_owns_persisted_bridge_session_accepts_mob_marked_session_without_live_handle() {
        let svc = Arc::new(MockSessionSvc::new());
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = svc.clone();
        let state = Arc::new(MobMcpState::new(session_service));

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
            realm_id: Some("mob:team".to_string()),
            instance_id: None,
            backend: None,
            config_generation: None,
            connection_ref: None,
        });
        svc.insert_persisted_session(persisted).await;

        assert!(
            state.owns_persisted_bridge_session(&persisted_id).await,
            "persisted mob members must still route through mob ownership after restart even before a live handle is rehydrated"
        );
    }

    #[tokio::test]
    async fn test_retire_member_by_bridge_session_id_falls_back_to_archiving_persisted_member() {
        let svc = Arc::new(MockSessionSvc::new());
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = svc.clone();
        let state = Arc::new(MobMcpState::new(session_service));

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
            realm_id: Some("mob:team".to_string()),
            instance_id: None,
            backend: None,
            config_generation: None,
            connection_ref: None,
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

    fn flow_enabled_definition() -> serde_json::Value {
        json!({
            "id": "flow-mob",
            "orchestrator": {
                "profile": "lead"
            },
            "profiles": {
                "lead": {
                    "model": "claude-opus-4-6",
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
        let state = Arc::new(MobMcpState::new(svc));
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
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let mob_id = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-6","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await["mob_id"]
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
    async fn test_mcp_stop_resume_round_trip() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let mob_id = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-6","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await["mob_id"]
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
        let state = Arc::new(MobMcpState::new(svc));
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
            "mob_cancel_flow should converge to canceled, or failed if terminal failure won the race first"
        );
    }

    #[tokio::test]
    async fn test_mcp_flow_status_rejects_invalid_run_id() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
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
        let state = Arc::new(MobMcpState::new(svc));
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
                            "model": "claude-opus-4-6",
                            "tools": {"comms": true},
                            "external_addressable": true
                        },
                        "worker": {
                            "model": "claude-sonnet-4-5",
                            "tools": {"comms": true},
                            "external_addressable": false
                        }
                    },
                    "mcp_servers": {},
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
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-6","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
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
        let state = Arc::new(MobMcpState::new(svc));
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
    async fn test_mob_wait_kickoff_returns_member_snapshots() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-6","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
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
    async fn test_mob_wait_kickoff_completes_after_initial_turn() {
        // The kickoff barrier waits for the initial autonomous turn to complete.
        // Use turn_driven members to avoid the keep_alive mock blocking.
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-6","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
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
        let state = Arc::new(MobMcpState::new(svc));
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
    async fn test_mob_create_rejects_missing_definition() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
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
        let state = Arc::new(MobMcpState::new(svc));
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
        let state = Arc::new(MobMcpState::new(svc));
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
            meerkat_mob::ProfileBinding::Inline(meerkat_mob::profile::Profile {
                model: "claude-sonnet-4-5".to_string(),
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
            }),
        );
        let mut definition = MobDefinition::explicit(MobId::from(mob_id));
        definition.profiles = explicit_profiles;
        definition
    }

    fn sample_realm_profile(model: &str) -> meerkat_mob::Profile {
        meerkat_mob::profile::Profile {
            model: model.to_string(),
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
    async fn test_persistent_root_restores_explicit_mob_member_status() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(
            MobMcpState::new(svc.clone())
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
            )
            .await
            .expect("spawn worker");

        let restored = Arc::new(
            MobMcpState::new(svc.clone())
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

        let mobs = restored.mob_list().await;
        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0].0, mob_id);
    }

    #[tokio::test]
    async fn test_mob_destroy_removes_persistent_store_file() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(
            MobMcpState::new(svc.clone())
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
            MobMcpState::new(svc).with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        assert!(
            restored.mob_list().await.is_empty(),
            "destroyed persistent mobs must not reappear after restart"
        );
    }

    #[tokio::test]
    async fn test_default_constructor_exposes_realm_profile_crud_with_in_memory_store() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));

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
    async fn test_persistent_root_upgrades_default_realm_profile_store_to_durable_sqlite() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");

        let state = Arc::new(
            MobMcpState::new(svc.clone())
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        state
            .realm_profile_create("worker", &sample_realm_profile("claude-opus-4-6"))
            .await
            .expect("create persistent realm profile");

        let restored = Arc::new(
            MobMcpState::new(svc).with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        let fetched = restored
            .realm_profile_get("worker")
            .await
            .expect("get restored realm profile")
            .expect("restored profile should exist");
        assert_eq!(fetched.profile.model, "claude-opus-4-6");
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
    async fn test_destroy_bridge_session_mobs_uses_cleanup_policy_not_owner_index() {
        let state = MobMcpState::new_in_memory();
        let sid = SessionId::new().to_string();

        let mut manual = explicit_definition("manual-owner-index");
        manual.set_owner_bridge_session_lookup_index(sid.clone());
        let manual_id = state
            .mob_create_definition(manual)
            .await
            .expect("create manual owner-indexed mob");

        let mut bridge_session_scoped = explicit_definition("bridge-session-scoped-owner");
        bridge_session_scoped.mark_owner_bridge_session_indexed(&sid);
        let bridge_session_scoped_id = state
            .mob_create_definition(bridge_session_scoped)
            .await
            .expect("create bridge-session-scoped mob");

        state.destroy_bridge_session_mobs(&sid).await.unwrap();

        assert!(
            state.handle_for(&manual_id).await.is_ok(),
            "owner bridge-session indexing alone must not make a mob eligible for cleanup"
        );
        assert!(
            state.handle_for(&bridge_session_scoped_id).await.is_err(),
            "DestroyOnOwnerArchive must remain the cleanup truth"
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
        let mobs = state.mob_list().await;
        let implicit_count = mobs.len();
        assert_eq!(implicit_count, 1, "only one implicit mob should exist");
    }

    #[tokio::test]
    async fn test_scavenge_orphaned_bridge_session_scoped_mobs() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc.clone()));

        // Create a session and its implicit mob
        let result = svc
            .create_session(CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: ContentInput::from("test"),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
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

        // Also create a manual owner-indexed explicit mob that should survive scavenging.
        let mut manual = explicit_definition("manual-owner-index");
        manual.set_owner_bridge_session_lookup_index(sid.clone());
        let manual_id = state
            .mob_create_definition(manual)
            .await
            .expect("create manual owner-indexed mob");

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
            "manual owner-indexed mob must survive orphan scavenging"
        );
    }

    #[tokio::test]
    async fn test_scavenge_orphaned_bridge_session_scoped_mobs_honors_bridge_owner_index() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc.clone()));

        let result = svc
            .create_session(CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: ContentInput::from("test"),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .unwrap();
        let sid = result.session_id.to_string();

        let mut bridge_owned = explicit_definition("bridge-owned-orphan");
        bridge_owned.mark_owner_bridge_session_indexed(&sid);
        let bridge_owned_id = state
            .mob_create_definition(bridge_owned)
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
    async fn test_implicit_mob_has_auto_wire_orchestrator() {
        let state = MobMcpState::new_in_memory();
        let sid = SessionId::new().to_string();

        let mob_id = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();
        let handle = state.handle_for(&mob_id).await.unwrap();
        assert!(
            handle.definition().wiring.auto_wire_orchestrator,
            "implicit mob must have auto_wire_orchestrator set"
        );
        assert_eq!(
            handle.definition().owner_bridge_session_index(),
            Some(sid.as_str()),
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
            model: "claude-sonnet-4-5".to_string(),
            prompt: "test".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            skill_references: None,
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
                prompt: "test".into(),
                system_prompt: None,
                render_metadata: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                event_tx: None,
                skill_references: None,
                flow_tool_overlay: None,
                pre_turn_context_appends: Vec::new(),
                turn_metadata: None,
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
}
