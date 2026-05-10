//! Surface-agnostic session runtime — the canonical orchestrator that
//! glues `PersistentSessionService`, `StagedSessionRegistry`,
//! `MeerkatMachine`, and the optional `LiveAdapterHost` together.
//!
//! Every Meerkat surface composes this crate (Rust SDK first, then
//! `rkat-rpc` for the Python/TS SDKs, then CLI / REST / MCP-server /
//! web SDK / embedded `examples/*`). Nothing here imports `RpcError`,
//! `RpcResponse`, axum types, or any other wire type — surfaces map
//! the typed errors in [`errors`] onto their own wire shapes.
//!
//! See `SESSION_RUNTIME_SPLIT_TODO.md` for the migration plan.

// Most of the session runtime requires async-sync primitives (`tokio::sync::Mutex`,
// `Notify`, `broadcast`) and `tokio::spawn` for Drop guards — none of which
// `tokio_with_wasm` exposes. The WASM SDK reaches the runtime via a different
// surface (`meerkat-web-runtime`), so gating these modules off wasm32 keeps the
// facade compiling for both targets without leaking unused async primitives
// into the wasm bundle. `errors` and the small `SessionState`/`SessionInfo`
// shells stay available everywhere because they're surface-agnostic data.
#[cfg(not(target_arch = "wasm32"))]
pub mod admission;
pub mod errors;
#[cfg(not(target_arch = "wasm32"))]
pub mod live_orchestration;
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub mod llm_reconfigure;
#[cfg(not(target_arch = "wasm32"))]
pub mod recovery;
#[cfg(not(target_arch = "wasm32"))]
pub mod runtime_state;
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub mod staged_promotion;

pub use errors::LiveOpenPrecheckError;
#[cfg(not(target_arch = "wasm32"))]
pub use runtime_state::{SessionInfo, SessionState};

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod inner {
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock as StdRwLock};

    use meerkat_client::LlmClient;
    use meerkat_core::ConfigRuntime;
    use meerkat_core::connection::RealmId;
    use meerkat_runtime::MeerkatMachine;

    use crate::service_factory::FactoryAgentBuilder;
    use crate::session_runtime::admission::StagedCapacityAdmissions;
    use crate::session_runtime::runtime_state::SkillIdentityRegistryState;
    use crate::{PersistentSessionService, StagedSessionRegistry};

    /// Surface-agnostic session runtime.
    ///
    /// Owns the shared session-orchestration state every Meerkat
    /// surface composes against. RPC's `SessionRuntime` wraps an
    /// `Arc<MeerkatSessionRuntime>` and continues to host RPC-specific
    /// glue (callback dispatcher, RpcMobSessionService, mob/MCP
    /// sessions); accessor parity is enforced by the W3-B move and the
    /// V5 surface-symmetry audit at the end of the wave.
    pub struct MeerkatSessionRuntime {
        /// Persistent session service.
        pub service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        /// Canonical staged-session authority.
        pub staged_sessions: Arc<StagedSessionRegistry>,
        /// Service-owned capacity guards for staged sessions.
        pub staged_capacity_admissions: StagedCapacityAdmissions,
        /// Runtime adapter (`MeerkatMachine`).
        pub runtime_adapter: Arc<MeerkatMachine>,
        /// Optional live-adapter host (gated on the `live` feature at
        /// the surface level; the type itself crosses every surface).
        #[cfg(feature = "live")]
        pub live_adapter_host: Arc<StdRwLock<Option<Arc<meerkat_live::LiveAdapterHost>>>>,
        /// Shared config runtime (config_runtime accessor).
        pub config_runtime: Arc<StdRwLock<Option<Arc<ConfigRuntime>>>>,
        /// Override LLM client used as the default for new sessions.
        pub default_llm_client: Arc<StdRwLock<Option<Arc<dyn LlmClient>>>>,
        /// Active realm id, if configured.
        pub realm_id: Arc<StdRwLock<Option<RealmId>>>,
        /// Active instance id, if configured.
        pub instance_id: Arc<StdRwLock<Option<String>>>,
        /// Active backend label, if configured.
        pub backend: Arc<StdRwLock<Option<String>>>,
        /// Skill-identity registry slot.
        pub skill_identity_registry: Arc<StdRwLock<SkillIdentityRegistryState>>,
        /// Skill-identity context root used by skill loaders.
        pub skill_identity_context_root: Arc<StdRwLock<Option<PathBuf>>>,
        /// Skill-identity user root used by skill loaders.
        pub skill_identity_user_root: Arc<StdRwLock<Option<PathBuf>>>,
    }

    impl MeerkatSessionRuntime {
        /// Shared config runtime.
        pub fn config_runtime(&self) -> Option<Arc<ConfigRuntime>> {
            self.config_runtime
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        /// Update the shared config runtime.
        pub fn set_config_runtime(&self, runtime: Arc<ConfigRuntime>) {
            *self
                .config_runtime
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(runtime);
        }

        /// Active realm id, if configured.
        pub fn realm_id(&self) -> Option<RealmId> {
            self.realm_id
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        /// Active instance id, if configured.
        pub fn instance_id(&self) -> Option<String> {
            self.instance_id
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        /// Active backend label, if configured.
        pub fn backend(&self) -> Option<String> {
            self.backend
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        /// Set realm/instance/backend metadata in one call. RPC's
        /// `set_realm_context` delegates here.
        pub fn set_realm_context(
            &self,
            realm_id: Option<RealmId>,
            instance_id: Option<String>,
            backend: Option<String>,
        ) {
            *self
                .realm_id
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = realm_id;
            *self
                .instance_id
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = instance_id;
            *self
                .backend
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = backend;
        }

        /// Override the shared default LLM client used by this runtime.
        pub fn set_default_llm_client(&self, client: Option<Arc<dyn LlmClient>>) {
            *self
                .default_llm_client
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = client;
        }

        /// Read the shared default LLM client.
        pub fn default_llm_client(&self) -> Option<Arc<dyn LlmClient>> {
            self.default_llm_client
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        /// Attach a live-adapter host. Subsequent `live/open` calls
        /// resolve it through [`live_adapter_host`].
        #[cfg(feature = "live")]
        pub fn set_live_adapter_host(&self, host: Arc<meerkat_live::LiveAdapterHost>) {
            if let Ok(mut slot) = self.live_adapter_host.write() {
                *slot = Some(host);
            }
        }

        /// Read the current live-adapter host, if attached.
        #[cfg(feature = "live")]
        pub fn live_adapter_host(&self) -> Option<Arc<meerkat_live::LiveAdapterHost>> {
            self.live_adapter_host
                .read()
                .ok()
                .and_then(|guard| guard.clone())
        }

        /// Snapshot the active skill-identity registry.
        pub fn skill_identity_registry(&self) -> meerkat_core::skills::SourceIdentityRegistry {
            self.skill_identity_registry
                .read()
                .map(|state| state.registry.clone())
                .unwrap_or_default()
        }

        /// Replace the skill-identity registry unconditionally. Most
        /// callers should prefer
        /// [`set_skill_identity_registry_for_generation`] which guards
        /// against stale-write races.
        pub fn set_skill_identity_registry(
            &self,
            registry: meerkat_core::skills::SourceIdentityRegistry,
        ) {
            if let Ok(mut slot) = self.skill_identity_registry.write() {
                slot.registry = registry;
            }
        }

        /// Replace the skill-identity registry only when `generation`
        /// is `>=` the currently-recorded generation. The runtime
        /// uses this to drop stale writes after a config-reload race.
        pub fn set_skill_identity_registry_for_generation(
            &self,
            generation: u64,
            registry: meerkat_core::skills::SourceIdentityRegistry,
        ) {
            if let Ok(mut slot) = self.skill_identity_registry.write()
                && generation >= slot.generation
            {
                slot.generation = generation;
                slot.registry = registry;
            }
        }
    }
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub use inner::MeerkatSessionRuntime;

/// Constructor builder for [`MeerkatSessionRuntime`].
///
/// W3-C populates this with `with_live_adapter_host`,
/// `with_config_runtime`, `with_default_llm_client`, `with_realm_id`,
/// `with_instance_id`, `with_backend`, `with_skill_identity_registry`.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub use builder::SessionRuntimeBuilder;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod builder {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};

    use meerkat_client::LlmClient;
    use meerkat_core::ConfigRuntime;
    use meerkat_core::connection::RealmId;
    use meerkat_core::skills::SourceIdentityRegistry;
    use meerkat_runtime::MeerkatMachine;

    use crate::service_factory::FactoryAgentBuilder;
    use crate::session_runtime::admission::StagedCapacityAdmissions;
    use crate::session_runtime::inner::MeerkatSessionRuntime;
    use crate::session_runtime::runtime_state::SkillIdentityRegistryState;
    use crate::{PersistentSessionService, StagedSessionRegistry};

    /// Constructor builder for [`MeerkatSessionRuntime`].
    pub struct SessionRuntimeBuilder {
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        staged_sessions: Arc<StagedSessionRegistry>,
        staged_capacity_admissions: StagedCapacityAdmissions,
        runtime_adapter: Arc<MeerkatMachine>,
        #[cfg(feature = "live")]
        live_adapter_host: Arc<StdRwLock<Option<Arc<meerkat_live::LiveAdapterHost>>>>,
        config_runtime: Arc<StdRwLock<Option<Arc<ConfigRuntime>>>>,
        default_llm_client: Arc<StdRwLock<Option<Arc<dyn LlmClient>>>>,
        realm_id: Arc<StdRwLock<Option<RealmId>>>,
        instance_id: Arc<StdRwLock<Option<String>>>,
        backend: Arc<StdRwLock<Option<String>>>,
        skill_identity_registry: Arc<StdRwLock<SkillIdentityRegistryState>>,
        skill_identity_context_root: Arc<StdRwLock<Option<PathBuf>>>,
        skill_identity_user_root: Arc<StdRwLock<Option<PathBuf>>>,
    }

    impl SessionRuntimeBuilder {
        /// Start a new builder. The three required arguments are the
        /// pieces every surface must own up-front; everything else is
        /// optional and added via `with_*` methods.
        pub fn new(
            service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
            staged_sessions: Arc<StagedSessionRegistry>,
            runtime_adapter: Arc<MeerkatMachine>,
        ) -> Self {
            Self {
                service,
                staged_sessions,
                staged_capacity_admissions: Arc::new(StdMutex::new(HashMap::new())),
                runtime_adapter,
                #[cfg(feature = "live")]
                live_adapter_host: Arc::new(StdRwLock::new(None)),
                config_runtime: Arc::new(StdRwLock::new(None)),
                default_llm_client: Arc::new(StdRwLock::new(None)),
                realm_id: Arc::new(StdRwLock::new(None)),
                instance_id: Arc::new(StdRwLock::new(None)),
                backend: Arc::new(StdRwLock::new(None)),
                skill_identity_registry: Arc::new(StdRwLock::new(
                    SkillIdentityRegistryState::default(),
                )),
                skill_identity_context_root: Arc::new(StdRwLock::new(None)),
                skill_identity_user_root: Arc::new(StdRwLock::new(None)),
            }
        }

        /// Reuse an existing staged-capacity ledger (e.g. when the
        /// surface owns one for its own RAII guards).
        #[must_use]
        pub fn with_staged_capacity_admissions(
            mut self,
            admissions: StagedCapacityAdmissions,
        ) -> Self {
            self.staged_capacity_admissions = admissions;
            self
        }

        /// Reuse an existing live-adapter host slot (RPC pre-builds
        /// this as `Arc<RwLock<None>>` so a later `set_*` can update
        /// it).
        #[cfg(feature = "live")]
        #[must_use]
        pub fn with_live_adapter_host_slot(
            mut self,
            slot: Arc<StdRwLock<Option<Arc<meerkat_live::LiveAdapterHost>>>>,
        ) -> Self {
            self.live_adapter_host = slot;
            self
        }

        /// Pre-populate the live-adapter host.
        #[cfg(feature = "live")]
        #[must_use]
        pub fn with_live_adapter_host(self, host: Arc<meerkat_live::LiveAdapterHost>) -> Self {
            if let Ok(mut slot) = self.live_adapter_host.write() {
                *slot = Some(host);
            }
            self
        }

        /// Reuse an existing config-runtime slot.
        #[must_use]
        pub fn with_config_runtime_slot(
            mut self,
            slot: Arc<StdRwLock<Option<Arc<ConfigRuntime>>>>,
        ) -> Self {
            self.config_runtime = slot;
            self
        }

        /// Pre-populate the config runtime.
        #[must_use]
        pub fn with_config_runtime(self, runtime: Arc<ConfigRuntime>) -> Self {
            if let Ok(mut slot) = self.config_runtime.write() {
                *slot = Some(runtime);
            }
            self
        }

        /// Reuse an existing default-llm-client slot.
        #[must_use]
        pub fn with_default_llm_client_slot(
            mut self,
            slot: Arc<StdRwLock<Option<Arc<dyn LlmClient>>>>,
        ) -> Self {
            self.default_llm_client = slot;
            self
        }

        /// Pre-populate the default LLM client.
        #[must_use]
        pub fn with_default_llm_client(self, client: Arc<dyn LlmClient>) -> Self {
            if let Ok(mut slot) = self.default_llm_client.write() {
                *slot = Some(client);
            }
            self
        }

        /// Active realm id.
        #[must_use]
        pub fn with_realm_id(self, realm_id: RealmId) -> Self {
            if let Ok(mut slot) = self.realm_id.write() {
                *slot = Some(realm_id);
            }
            self
        }

        /// Active instance id.
        #[must_use]
        pub fn with_instance_id(self, instance_id: String) -> Self {
            if let Ok(mut slot) = self.instance_id.write() {
                *slot = Some(instance_id);
            }
            self
        }

        /// Active backend label.
        #[must_use]
        pub fn with_backend(self, backend: String) -> Self {
            if let Ok(mut slot) = self.backend.write() {
                *slot = Some(backend);
            }
            self
        }

        /// Reuse an existing realm-id slot (lets a surface share the
        /// slot with its own RPC `SessionRuntime`).
        #[must_use]
        pub fn with_realm_id_slot(mut self, slot: Arc<StdRwLock<Option<RealmId>>>) -> Self {
            self.realm_id = slot;
            self
        }

        /// Reuse an existing instance-id slot.
        #[must_use]
        pub fn with_instance_id_slot(mut self, slot: Arc<StdRwLock<Option<String>>>) -> Self {
            self.instance_id = slot;
            self
        }

        /// Reuse an existing backend slot.
        #[must_use]
        pub fn with_backend_slot(mut self, slot: Arc<StdRwLock<Option<String>>>) -> Self {
            self.backend = slot;
            self
        }

        /// Pre-populate the skill-identity registry.
        #[must_use]
        pub fn with_skill_identity_registry(self, registry: SourceIdentityRegistry) -> Self {
            if let Ok(mut slot) = self.skill_identity_registry.write() {
                slot.registry = registry;
            }
            self
        }

        /// Reuse an existing skill-identity registry slot.
        #[must_use]
        pub fn with_skill_identity_registry_slot(
            mut self,
            slot: Arc<StdRwLock<SkillIdentityRegistryState>>,
        ) -> Self {
            self.skill_identity_registry = slot;
            self
        }

        /// Reuse an existing skill-identity context-root slot.
        #[must_use]
        pub fn with_skill_identity_context_root_slot(
            mut self,
            slot: Arc<StdRwLock<Option<PathBuf>>>,
        ) -> Self {
            self.skill_identity_context_root = slot;
            self
        }

        /// Pre-populate the skill-identity context root.
        #[must_use]
        pub fn with_skill_identity_context_root(self, path: PathBuf) -> Self {
            if let Ok(mut slot) = self.skill_identity_context_root.write() {
                *slot = Some(path);
            }
            self
        }

        /// Reuse an existing skill-identity user-root slot.
        #[must_use]
        pub fn with_skill_identity_user_root_slot(
            mut self,
            slot: Arc<StdRwLock<Option<PathBuf>>>,
        ) -> Self {
            self.skill_identity_user_root = slot;
            self
        }

        /// Pre-populate the skill-identity user root.
        #[must_use]
        pub fn with_skill_identity_user_root(self, path: PathBuf) -> Self {
            if let Ok(mut slot) = self.skill_identity_user_root.write() {
                *slot = Some(path);
            }
            self
        }

        /// Finalize the builder.
        #[must_use]
        pub fn build(self) -> MeerkatSessionRuntime {
            MeerkatSessionRuntime {
                service: self.service,
                staged_sessions: self.staged_sessions,
                staged_capacity_admissions: self.staged_capacity_admissions,
                runtime_adapter: self.runtime_adapter,
                #[cfg(feature = "live")]
                live_adapter_host: self.live_adapter_host,
                config_runtime: self.config_runtime,
                default_llm_client: self.default_llm_client,
                realm_id: self.realm_id,
                instance_id: self.instance_id,
                backend: self.backend,
                skill_identity_registry: self.skill_identity_registry,
                skill_identity_context_root: self.skill_identity_context_root,
                skill_identity_user_root: self.skill_identity_user_root,
            }
        }
    }
}
